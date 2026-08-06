//! MVCC Version Manager
//!
//! Provides timestamp management for MVCC (Multi-Version Concurrency Control)
//! based transaction isolation.
//!
//! ## Concurrency Model
//!
//! All write transactions are "insert" transactions that run concurrently.
//! Conflicts are detected by WriteSet at commit time, not at start time.
//! No write transaction ever blocks readers.
//!
//! This module uses `parking_lot::Condvar` for efficient waiting instead of
//! spin-wait loops. This reduces CPU usage during contention and provides
//! proper timeout support.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use super::snapshot_tracker::SnapshotTracker;
use crate::core::types::Timestamp;

/// Released timestamp sentinel value (0 means timestamp has been released)
/// Note: distinct from Timestamp::MAX which may be used as a sentinel elsewhere
pub const RELEASED_TIMESTAMP: Timestamp = 0;

#[derive(Debug, Clone, thiserror::Error)]
pub enum VersionManagerError {
    #[error("Too many concurrent transactions")]
    TooManyTransactions,

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(Timestamp),

    #[error("Timeout waiting for transaction")]
    Timeout,

    #[error("Failed to track snapshot for timestamp")]
    SnapshotTrackingFailed,

    #[error("Timestamp space exhausted")]
    TimestampExhausted,

    #[error("Timestamp {timestamp} is older than retention frontier {frontier}")]
    TimestampBeforeRetention {
        timestamp: Timestamp,
        frontier: Timestamp,
    },
}

pub type VersionManagerResult<T> = Result<T, VersionManagerError>;

#[derive(Debug, Clone)]
pub struct VersionManagerConfig {
    pub max_concurrent_reads: u32,
    pub wait_timeout: Duration,
    /// The oldest timestamp that may be opened as a historical snapshot.
    /// Zero disables the retention check.
    pub retention_frontier: Timestamp,
    /// Minimum age of a `Pending` write timestamp before
    /// [`VersionManager::reap_expired_write_timestamps`] aborts it as stale.
    ///
    /// This replaces the former force-advance (`max_frontier_stall`) which was
    /// unreachable and, if configured, could publish uncommitted writes.
    pub write_reap_timeout: Duration,
}

impl Default for VersionManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_reads: 1000,
            wait_timeout: Duration::from_secs(5),
            retention_frontier: 0,
            write_reap_timeout: Duration::from_secs(60),
        }
    }
}

impl VersionManagerConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_concurrent_reads(mut self, max: u32) -> Self {
        self.max_concurrent_reads = max;
        self
    }

    pub fn with_retention_frontier(mut self, timestamp: Timestamp) -> Self {
        self.retention_frontier = timestamp;
        self
    }

    pub fn with_write_reap_timeout(mut self, timeout: Duration) -> Self {
        self.write_reap_timeout = timeout;
        self
    }
}

pub struct VersionManager {
    write_ts: AtomicU64,
    read_ts: AtomicU64,

    // Read admission channel — independent from writes
    read_pending: AtomicI32,
    read_lock: Mutex<()>,
    read_condvar: Condvar,

    // Write admission channel — independent from reads
    write_pending: AtomicI32,
    write_lock: Mutex<()>,
    write_condvar: Condvar,

    config: VersionManagerConfig,
    snapshot_tracker: Arc<SnapshotTracker>,
    write_states: Mutex<BTreeMap<Timestamp, (Instant, WriteTimestampState)>>,
    retention_frontier: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteTimestampState {
    Pending,
    Committed,
    Aborted,
}

impl VersionManager {
    pub fn new() -> Self {
        Self::with_config(VersionManagerConfig::default())
    }

    pub fn with_config(config: VersionManagerConfig) -> Self {
        let retention_frontier = config.retention_frontier;
        Self {
            write_ts: AtomicU64::new(1),
            read_ts: AtomicU64::new(1),
            read_pending: AtomicI32::new(0),
            read_lock: Mutex::new(()),
            read_condvar: Condvar::new(),
            write_pending: AtomicI32::new(0),
            write_lock: Mutex::new(()),
            write_condvar: Condvar::new(),
            config,
            snapshot_tracker: Arc::new(SnapshotTracker::new()),
            write_states: Mutex::new(BTreeMap::new()),
            retention_frontier: AtomicU64::new(retention_frontier),
        }
    }

    pub fn init_ts(&self, ts: Timestamp) {
        // `write_ts` is the last allocated timestamp. Keeping the baseline at
        // the recovered timestamp makes the next allocation checked and
        // contiguous, including at the u64 boundary.
        self.write_ts.store(ts, Ordering::SeqCst);
        self.read_ts.store(ts, Ordering::SeqCst);
        self.write_states.lock().clear();
    }

    /// Set the oldest timestamp that can be retained as a historical snapshot.
    /// The frontier only moves forward.
    pub fn set_retention_frontier(&self, timestamp: Timestamp) {
        self.retention_frontier
            .fetch_max(timestamp, Ordering::SeqCst);
    }

    pub fn retention_frontier(&self) -> Timestamp {
        self.retention_frontier.load(Ordering::SeqCst)
    }

    pub fn clear(&self) {
        // Preserve write_ts so that subsequent writes and checkpoints
        // use timestamps >= the compact timestamp, ensuring persisted
        // data remains visible after reload.
        self.read_ts.store(0, Ordering::SeqCst);
        self.read_pending.store(0, Ordering::SeqCst);
        self.write_pending.store(0, Ordering::SeqCst);
        self.write_states.lock().clear();
    }

    pub fn write_timestamp(&self) -> Timestamp {
        self.write_ts.load(Ordering::SeqCst)
    }

    /// Allocate the next write timestamp.
    pub fn next_write_timestamp(&self) -> VersionManagerResult<Timestamp> {
        self.try_next_write_timestamp()
    }

    pub fn try_next_write_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let ts = self.reserve_timestamp()?;
        self.write_pending.fetch_add(1, Ordering::SeqCst);
        self.snapshot_tracker
            .add_snapshot(ts)
            .map_err(|_| VersionManagerError::SnapshotTrackingFailed)?;
        self.write_states
            .lock()
            .insert(ts, (Instant::now(), WriteTimestampState::Pending));
        Ok(ts)
    }

    fn reserve_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut current = self.write_ts.load(Ordering::SeqCst);
        loop {
            let next = current
                .checked_add(1)
                .ok_or(VersionManagerError::TimestampExhausted)?;
            match self
                .write_ts
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Ok(next),
                Err(observed) => current = observed,
            }
        }
    }

    pub fn read_timestamp(&self) -> Timestamp {
        self.read_ts.load(Ordering::SeqCst)
    }

    pub fn acquire_read_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut guard = self.read_lock.lock();
        loop {
            let pr = self.read_pending.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_reads as i32 {
                    log::warn!(
                        "Too many pending read requests: {}. Max concurrent reads: {}. \
                        Consider increasing max_concurrent_reads or reducing read intensity.",
                        pr,
                        self.config.max_concurrent_reads,
                    );
                    self.read_condvar.wait(&mut guard);
                    continue;
                }
                self.read_pending.fetch_add(1, Ordering::SeqCst);
                let ts = self.read_ts.load(Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.read_pending.fetch_sub(1, Ordering::SeqCst);
                    self.read_condvar.notify_all();
                    return Err(VersionManagerError::SnapshotTrackingFailed);
                }
                return Ok(ts);
            }
            self.read_condvar.wait(&mut guard);
        }
    }

    pub fn acquire_read_timestamp_with_timeout(&self, timeout: Duration) -> Option<Timestamp> {
        let start = Instant::now();
        let mut guard = self.read_lock.lock();
        loop {
            let pr = self.read_pending.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_reads as i32 {
                    log::warn!(
                        "Too many pending read requests: {}. Max concurrent reads: {}.",
                        pr,
                        self.config.max_concurrent_reads,
                    );
                    let elapsed = start.elapsed();
                    if elapsed >= timeout {
                        return None;
                    }
                    let remaining = timeout - elapsed;
                    let result = self.read_condvar.wait_for(&mut guard, remaining);
                    if result.timed_out() {
                        return None;
                    }
                    continue;
                }
                self.read_pending.fetch_add(1, Ordering::SeqCst);
                let ts = self.read_ts.load(Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.read_pending.fetch_sub(1, Ordering::SeqCst);
                    return None;
                }
                return Some(ts);
            }

            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return None;
            }

            let remaining = timeout - elapsed;
            let result = self.read_condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                return None;
            }
        }
    }

    /// Register a read at an already committed historical timestamp.
    pub fn acquire_read_timestamp_at(
        &self,
        timestamp: Timestamp,
    ) -> VersionManagerResult<Timestamp> {
        if timestamp > self.read_timestamp() {
            return Err(VersionManagerError::InvalidTimestamp(timestamp));
        }
        let retention_frontier = self.retention_frontier();
        if retention_frontier != 0 && timestamp < retention_frontier {
            return Err(VersionManagerError::TimestampBeforeRetention {
                timestamp,
                frontier: retention_frontier,
            });
        }
        let guard = self.read_lock.lock();
        let pending = self.read_pending.load(Ordering::SeqCst);
        if pending < 0 || pending >= self.config.max_concurrent_reads as i32 {
            drop(guard);
            return Err(VersionManagerError::TooManyTransactions);
        }
        self.read_pending.fetch_add(1, Ordering::SeqCst);
        if self.snapshot_tracker.add_snapshot(timestamp).is_err() {
            self.read_pending.fetch_sub(1, Ordering::SeqCst);
            self.read_condvar.notify_all();
            drop(guard);
            return Err(VersionManagerError::SnapshotTrackingFailed);
        }
        drop(guard);
        Ok(timestamp)
    }

    pub fn release_read_timestamp(&self) {
        let ts = self.read_ts.load(Ordering::SeqCst);
        self.release_read_timestamp_at(ts);
    }

    pub fn release_read_timestamp_at(&self, ts: Timestamp) {
        if let Err(e) = self.snapshot_tracker.release_snapshot(ts) {
            log::error!("Failed to release snapshot {}: {}", ts, e);
            // Continue anyway - we still need to decrement read_pending
        }
        self.read_pending.fetch_sub(1, Ordering::SeqCst);
        self.read_condvar.notify_all();
    }

    pub fn acquire_insert_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let _guard = self.write_lock.lock();
        let ts = self.reserve_timestamp()?;
        if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
            log::error!("Failed to pre-reserve snapshot {}: {}", ts, e);
            return Err(VersionManagerError::SnapshotTrackingFailed);
        }
        self.write_states
            .lock()
            .insert(ts, (Instant::now(), WriteTimestampState::Pending));
        self.write_pending.fetch_add(1, Ordering::SeqCst);
        drop(_guard);
        Ok(ts)
    }

    pub fn acquire_insert_timestamp_with_timeout(&self, _timeout: Duration) -> Option<Timestamp> {
        self.acquire_insert_timestamp().ok()
    }

    pub fn commit_write_timestamp(&self, ts: Timestamp) {
        self.finish_write_timestamp(ts, WriteTimestampState::Committed);
    }

    pub fn abort_write_timestamp(&self, ts: Timestamp) {
        self.finish_write_timestamp(ts, WriteTimestampState::Aborted);
    }

    fn finish_write_timestamp(&self, ts: Timestamp, state: WriteTimestampState) {
        let mut states = self.write_states.lock();
        if let Some((_, entry)) = states.get_mut(&ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = state;
                let _ = self.snapshot_tracker.release_snapshot(ts);
                self.write_pending.fetch_sub(1, Ordering::SeqCst);
            }
        }

        self.advance_read_frontier(&mut states);
        drop(states);
        self.write_condvar.notify_all();
    }

    /// Advance the read frontier over terminal (Committed/Aborted) timestamps.
    ///
    /// The frontier never crosses a live `Pending` write: a pending timestamp
    /// is an in-flight write whose data must not become visible to readers.
    /// Crossing it would publish the transaction's partial writes (dirty read).
    /// Long-lived pending writes are instead terminated by
    /// [`VersionManager::reap_expired_write_timestamps`] (driven by the
    /// transaction manager's periodic cleanup).
    fn advance_read_frontier(
        &self,
        states: &mut BTreeMap<Timestamp, (Instant, WriteTimestampState)>,
    ) {
        let mut frontier = self.read_ts.load(Ordering::SeqCst);
        loop {
            let next = frontier.saturating_add(1);
            match states.get(&next).map(|(_, state)| *state) {
                Some(WriteTimestampState::Committed | WriteTimestampState::Aborted) => {
                    frontier = next;
                    states.remove(&next);
                }
                _ => break,
            }
        }
        self.read_ts.store(frontier, Ordering::SeqCst);
    }

    /// Abort `Pending` write timestamps older than `write_reap_timeout`,
    /// advancing the read frontier so version GC can proceed.
    ///
    /// This is a safety net for write timestamps whose owning path vanished
    /// (orphaned write). Callers must pass the set of timestamps currently
    /// owned by live write transactions so those are never reaped; reaping a
    /// live transaction's timestamp would silently discard its writes.
    ///
    /// Returns the number of timestamps reaped.
    pub fn reap_expired_write_timestamps(
        &self,
        timeout: Duration,
        owned: &std::collections::HashSet<Timestamp>,
    ) -> usize {
        let now = Instant::now();
        let mut states = self.write_states.lock();
        let expired: Vec<Timestamp> = states
            .iter()
            .filter(|(ts, (acquired, state))| {
                *state == WriteTimestampState::Pending
                    && !owned.contains(ts)
                    && now.duration_since(*acquired) > timeout
            })
            .map(|(ts, _)| *ts)
            .collect();

        let mut reaped = 0;
        for ts in expired {
            if let Some((_, entry)) = states.get_mut(&ts) {
                if *entry == WriteTimestampState::Pending {
                    *entry = WriteTimestampState::Aborted;
                    let _ = self.snapshot_tracker.release_snapshot(ts);
                    self.write_pending.fetch_sub(1, Ordering::SeqCst);
                    reaped += 1;
                }
            }
        }

        if reaped > 0 {
            self.advance_read_frontier(&mut states);
        }
        drop(states);
        self.write_condvar.notify_all();
        reaped
    }

    pub fn pending_count(&self) -> i32 {
        self.read_pending.load(Ordering::SeqCst) + self.write_pending.load(Ordering::SeqCst)
    }

    pub fn get_safe_gc_timestamp(&self) -> Timestamp {
        let active = self.snapshot_tracker.min_active_snapshot();
        let retention = self.retention_frontier();
        if retention == 0 {
            active
        } else {
            active.min(retention)
        }
    }

    pub fn get_safe_gc_timestamp_with_margin(&self, margin: Timestamp) -> Timestamp {
        let safe_ts = self.get_safe_gc_timestamp();
        safe_ts.saturating_sub(margin)
    }

    /// Get the snapshot tracker for explicit snapshot management
    pub fn snapshot_tracker(&self) -> &SnapshotTracker {
        &self.snapshot_tracker
    }

    #[cfg(test)]
    fn backdate_write_timestamp(&self, ts: Timestamp, elapsed: Duration) {
        if let Some((acquired, _)) = self.write_states.lock().get_mut(&ts) {
            *acquired = Instant::now() - elapsed;
        }
    }
}

impl Default for VersionManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ReadTimestampGuard {
    version_manager: Arc<VersionManager>,
    timestamp: Timestamp,
}

impl ReadTimestampGuard {
    pub fn new(version_manager: Arc<VersionManager>) -> VersionManagerResult<Self> {
        let timestamp = version_manager.acquire_read_timestamp()?;
        Ok(Self {
            version_manager,
            timestamp,
        })
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

impl Drop for ReadTimestampGuard {
    fn drop(&mut self) {
        self.version_manager
            .release_read_timestamp_at(self.timestamp);
    }
}

pub struct InsertTimestampGuard {
    version_manager: Arc<VersionManager>,
    timestamp: Option<Timestamp>,
}

impl InsertTimestampGuard {
    pub fn new(version_manager: Arc<VersionManager>) -> VersionManagerResult<Self> {
        let timestamp = version_manager.acquire_insert_timestamp()?;
        Ok(Self {
            version_manager,
            timestamp: Some(timestamp),
        })
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp.unwrap_or(0)
    }

    pub fn commit(mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.commit_write_timestamp(ts);
        }
    }

    pub fn abort(mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.abort_write_timestamp(ts);
        }
    }
}

impl Drop for InsertTimestampGuard {
    fn drop(&mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.abort_write_timestamp(ts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_version_manager_basic() {
        let vm = VersionManager::new();

        let ts1 = vm.acquire_read_timestamp().expect("acquire read");
        assert_eq!(ts1, 1);
        vm.release_read_timestamp();

        let ts2 = vm.acquire_insert_timestamp().expect("acquire insert");
        assert!(ts2 >= 1);
        vm.commit_write_timestamp(ts2);
    }

    #[test]
    fn test_read_timestamp_guard() {
        let vm = Arc::new(VersionManager::new());

        {
            let guard = ReadTimestampGuard::new(vm.clone()).expect("guard should be created");
            assert_eq!(guard.timestamp(), 1);
        }

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_insert_timestamp_guard() {
        let vm = Arc::new(VersionManager::new());

        {
            let guard = InsertTimestampGuard::new(vm.clone()).expect("guard should be created");
            let ts = guard.timestamp();
            assert!(ts >= 1);
        }

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_concurrent_reads() {
        let vm = Arc::new(VersionManager::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let vm_clone = vm.clone();
            handles.push(thread::spawn(move || {
                let guard = ReadTimestampGuard::new(vm_clone).expect("guard should be created");
                thread::sleep(Duration::from_millis(10));
                guard.timestamp()
            }));
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|&ts| ts == 1));
    }

    #[test]
    fn test_concurrent_inserts() {
        let vm = Arc::new(VersionManager::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let vm_clone = vm.clone();
            handles.push(thread::spawn(move || {
                let guard = InsertTimestampGuard::new(vm_clone).expect("guard should be created");
                let ts = guard.timestamp();
                thread::sleep(Duration::from_millis(10));
                ts
            }));
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let unique: HashSet<_> = results.into_iter().collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn test_snapshot_tracker_cleanup_threshold() {
        let vm = Arc::new(VersionManager::new());
        let tracker = vm.snapshot_tracker();

        // Add multiple snapshots via insert timestamps
        let ts1 = vm.acquire_insert_timestamp().expect("acquire insert");
        let ts2 = vm.acquire_insert_timestamp().expect("acquire insert");
        let ts3 = vm.acquire_insert_timestamp().expect("acquire insert");

        // Cleanup threshold should be minimum active
        assert_eq!(tracker.cleanup_threshold(), ts1);

        // Release first
        vm.commit_write_timestamp(ts1);
        assert_eq!(tracker.cleanup_threshold(), ts2);

        // Release second
        vm.commit_write_timestamp(ts2);
        assert_eq!(tracker.cleanup_threshold(), ts3);

        // Release last
        vm.commit_write_timestamp(ts3);
        assert_eq!(tracker.cleanup_threshold(), u64::MAX); // No active snapshots
    }

    #[test]
    fn test_out_of_order_commit_does_not_advance_frontier() {
        let vm = VersionManager::new();
        let first = vm
            .acquire_insert_timestamp()
            .expect("first write timestamp");
        let second = vm
            .acquire_insert_timestamp()
            .expect("second write timestamp");

        vm.commit_write_timestamp(second);
        assert_eq!(vm.read_timestamp(), first - 1);
        assert_eq!(vm.pending_count(), 1);

        vm.commit_write_timestamp(first);
        assert_eq!(vm.read_timestamp(), second);
        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_abort_does_not_publish_frontier_as_a_commit() {
        let vm = VersionManager::new();
        let timestamp = vm.acquire_insert_timestamp().expect("write timestamp");

        vm.abort_write_timestamp(timestamp);

        assert_eq!(vm.read_timestamp(), timestamp);
        assert_eq!(vm.pending_count(), 0);
        assert_eq!(vm.snapshot_tracker().active_count(), 0);
    }

    #[test]
    fn test_read_guard_releases_original_timestamp() {
        let vm = Arc::new(VersionManager::new());
        let guard = ReadTimestampGuard::new(vm.clone()).expect("read timestamp");
        let timestamp = guard.timestamp();
        let write_timestamp = vm.acquire_insert_timestamp().expect("write timestamp");
        vm.commit_write_timestamp(write_timestamp);

        assert_eq!(vm.read_timestamp(), write_timestamp);
        drop(guard);
        assert_eq!(vm.snapshot_tracker().ref_count(timestamp), None);
        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_historical_read_tracks_requested_timestamp() {
        let vm = Arc::new(VersionManager::new());
        let write_timestamp = vm.acquire_insert_timestamp().expect("write timestamp");
        vm.commit_write_timestamp(write_timestamp);

        let timestamp = vm
            .acquire_read_timestamp_at(write_timestamp)
            .expect("historical timestamp");
        assert_eq!(timestamp, write_timestamp);
        assert_eq!(vm.snapshot_tracker().ref_count(timestamp), Some(1));

        vm.release_read_timestamp_at(timestamp);
        assert_eq!(vm.snapshot_tracker().ref_count(timestamp), None);
    }

    #[test]
    fn test_historical_read_before_retention_is_rejected() {
        let vm =
            VersionManager::with_config(VersionManagerConfig::default().with_retention_frontier(3));
        vm.init_ts(3);

        let error = vm
            .acquire_read_timestamp_at(2)
            .expect_err("historical snapshots before retention must be rejected");
        assert!(matches!(
            error,
            VersionManagerError::TimestampBeforeRetention {
                timestamp: 2,
                frontier: 3
            }
        ));
    }

    #[test]
    fn test_timestamp_exhaustion_is_reported() {
        let vm = VersionManager::new();
        vm.init_ts(Timestamp::MAX);

        assert!(matches!(
            vm.try_next_write_timestamp(),
            Err(VersionManagerError::TimestampExhausted)
        ));
        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_retention_frontier_limits_safe_gc_timestamp() {
        let vm = VersionManager::with_config(
            VersionManagerConfig::default().with_retention_frontier(10),
        );
        vm.init_ts(20);

        assert_eq!(vm.get_safe_gc_timestamp(), 10);

        let snapshot = vm
            .acquire_read_timestamp_at(15)
            .expect("snapshot inside retention should be accepted");
        assert_eq!(vm.get_safe_gc_timestamp(), 10);

        vm.release_read_timestamp_at(snapshot);
        assert_eq!(vm.get_safe_gc_timestamp(), 10);
    }

    #[test]
    fn test_frontier_never_crosses_pending_write() {
        let vm = VersionManager::new();
        let first = vm
            .acquire_insert_timestamp()
            .expect("first write timestamp");
        let second = vm
            .acquire_insert_timestamp()
            .expect("second write timestamp");

        // Committing out of order must not advance the frontier past the
        // still-pending `first` write: crossing it would publish `first`'s
        // partial writes to new readers (dirty read).
        vm.commit_write_timestamp(second);
        assert_eq!(vm.read_timestamp(), first - 1);
        assert_eq!(vm.pending_count(), 1);

        // Even repeated commit attempts must not skip the pending timestamp.
        vm.commit_write_timestamp(second);
        assert_eq!(vm.read_timestamp(), first - 1);

        vm.commit_write_timestamp(first);
        assert_eq!(vm.read_timestamp(), second);
        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_orphaned_write_timestamp_is_reaped() {
        let vm = VersionManager::new();
        let first = vm
            .acquire_insert_timestamp()
            .expect("first write timestamp");
        let second = vm
            .acquire_insert_timestamp()
            .expect("second write timestamp");

        // The owning transaction vanished without commit/abort: `first` stays
        // Pending and pins the frontier.
        vm.backdate_write_timestamp(first, Duration::from_secs(120));
        assert_eq!(vm.read_timestamp(), first - 1);

        let owned = std::collections::HashSet::new();
        let reaped = vm.reap_expired_write_timestamps(Duration::from_secs(60), &owned);
        assert_eq!(reaped, 1);
        // `first` is aborted; `second` is still Pending so the frontier stops
        // just before it.
        assert_eq!(vm.read_timestamp(), second - 1);
        assert_eq!(vm.pending_count(), 1);
    }

    #[test]
    fn test_reaper_skips_owned_timestamp() {
        let vm = VersionManager::new();
        let first = vm
            .acquire_insert_timestamp()
            .expect("first write timestamp");
        vm.backdate_write_timestamp(first, Duration::from_secs(120));

        // The timestamp is owned by a live transaction: it must not be reaped
        // even though it is older than the reap timeout.
        let owned = std::collections::HashSet::from([first]);
        let reaped = vm.reap_expired_write_timestamps(Duration::from_secs(60), &owned);
        assert_eq!(reaped, 0);
        assert_eq!(vm.read_timestamp(), first - 1);
        assert_eq!(vm.pending_count(), 1);

        // Once the owner releases it (no longer owned), the same entry is reaped.
        let reaped = vm.reap_expired_write_timestamps(Duration::from_secs(60), &HashSet::new());
        assert_eq!(reaped, 1);
        assert_eq!(vm.read_timestamp(), first);
        assert_eq!(vm.pending_count(), 0);
    }
}

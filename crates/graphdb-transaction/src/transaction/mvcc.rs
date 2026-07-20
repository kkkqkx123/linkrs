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
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
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
}

pub type VersionManagerResult<T> = Result<T, VersionManagerError>;

#[derive(Debug, Clone)]
pub struct VersionManagerConfig {
    pub max_concurrent_reads: u32,
    pub max_concurrent_inserts: u32,
    pub wait_timeout: Duration,
}

impl Default for VersionManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_reads: 1000,
            max_concurrent_inserts: 100,
            wait_timeout: Duration::from_secs(5),
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

    pub fn with_max_concurrent_inserts(mut self, max: u32) -> Self {
        self.max_concurrent_inserts = max;
        self
    }
}

pub struct VersionManager {
    write_ts: AtomicU32,
    read_ts: AtomicU32,
    pending_reqs: AtomicI32,
    lock: Mutex<()>,
    condvar: Condvar,
    config: VersionManagerConfig,
    snapshot_tracker: Arc<SnapshotTracker>,
    write_states: Mutex<BTreeMap<Timestamp, WriteTimestampState>>,
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
        Self {
            write_ts: AtomicU32::new(1),
            read_ts: AtomicU32::new(1),
            pending_reqs: AtomicI32::new(0),
            lock: Mutex::new(()),
            condvar: Condvar::new(),
            config,
            snapshot_tracker: Arc::new(SnapshotTracker::new()),
            write_states: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn init_ts(&self, ts: Timestamp) {
        self.write_ts.store(ts + 1, Ordering::SeqCst);
        self.read_ts.store(ts, Ordering::SeqCst);
        self.write_states.lock().clear();
    }

    pub fn clear(&self) {
        // Preserve write_ts so that subsequent writes and checkpoints
        // use timestamps >= the compact timestamp, ensuring persisted
        // data remains visible after reload.
        self.read_ts.store(0, Ordering::SeqCst);
        self.pending_reqs.store(0, Ordering::SeqCst);
        self.write_states.lock().clear();
    }

    pub fn write_timestamp(&self) -> Timestamp {
        self.write_ts.load(Ordering::SeqCst)
    }

    pub fn next_write_timestamp(&self) -> Timestamp {
        let ts = self.write_ts.fetch_add(1, Ordering::SeqCst) + 1;
        self.pending_reqs.fetch_add(1, Ordering::SeqCst);
        self.snapshot_tracker
            .add_snapshot(ts)
            .expect("write timestamp snapshot tracking must succeed");
        self.write_states
            .lock()
            .insert(ts, WriteTimestampState::Pending);
        ts
    }

    pub fn read_timestamp(&self) -> Timestamp {
        self.read_ts.load(Ordering::SeqCst)
    }

    pub fn acquire_read_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_reads as i32 {
                    log::warn!(
                        "Too many pending read requests: {}. Max concurrent reads: {}. \
                        Consider increasing max_concurrent_reads or reducing read intensity.",
                        pr,
                        self.config.max_concurrent_reads,
                    );
                    self.condvar.wait(&mut guard);
                    continue;
                }
                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                let ts = self.read_ts.load(Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
                    self.condvar.notify_all();
                    return Err(VersionManagerError::SnapshotTrackingFailed);
                }
                return Ok(ts);
            }
            self.condvar.wait(&mut guard);
        }
    }

    pub fn acquire_read_timestamp_with_timeout(&self, timeout: Duration) -> Option<Timestamp> {
        let start = Instant::now();
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
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
                    let result = self.condvar.wait_for(&mut guard, remaining);
                    if result.timed_out() {
                        return None;
                    }
                    continue;
                }
                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                let ts = self.read_ts.load(Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
                    return None;
                }
                return Some(ts);
            }

            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return None;
            }

            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
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
        let guard = self.lock.lock();
        let pending = self.pending_reqs.load(Ordering::SeqCst);
        if pending < 0 || pending >= self.config.max_concurrent_reads as i32 {
            drop(guard);
            return Err(VersionManagerError::TooManyTransactions);
        }
        self.pending_reqs.fetch_add(1, Ordering::SeqCst);
        if self.snapshot_tracker.add_snapshot(timestamp).is_err() {
            self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
            self.condvar.notify_all();
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
            // Continue anyway - we still need to decrement pending_reqs
        }
        self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    pub fn acquire_insert_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_inserts as i32 {
                    log::warn!(
                        "Too many pending insert requests: {}. Max concurrent inserts: {}. \
                        Consider increasing max_concurrent_inserts or reducing write intensity.",
                        pr,
                        self.config.max_concurrent_inserts,
                    );
                    self.condvar.wait(&mut guard);
                    continue;
                }

                let ts = self.write_ts.fetch_add(1, Ordering::SeqCst) + 1;
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to pre-reserve snapshot {}: {}", ts, e);
                    return Err(VersionManagerError::SnapshotTrackingFailed);
                }
                self.write_states
                    .lock()
                    .insert(ts, WriteTimestampState::Pending);
                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                drop(guard);
                return Ok(ts);
            }
            self.condvar.wait(&mut guard);
        }
    }

    pub fn acquire_insert_timestamp_with_timeout(&self, timeout: Duration) -> Option<Timestamp> {
        let start = Instant::now();
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_inserts as i32 {
                    log::warn!(
                        "Too many pending insert requests: {}. Max concurrent inserts: {}.",
                        pr,
                        self.config.max_concurrent_inserts,
                    );
                    let elapsed = start.elapsed();
                    if elapsed >= timeout {
                        return None;
                    }
                    let remaining = timeout - elapsed;
                    let result = self.condvar.wait_for(&mut guard, remaining);
                    if result.timed_out() {
                        return None;
                    }
                    continue;
                }

                let ts = self.write_ts.fetch_add(1, Ordering::SeqCst) + 1;
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to pre-reserve snapshot {}: {}", ts, e);
                    return None;
                }
                self.write_states
                    .lock()
                    .insert(ts, WriteTimestampState::Pending);
                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                drop(guard);
                return Some(ts);
            }

            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return None;
            }

            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                return None;
            }
        }
    }

    pub fn release_write_timestamp(&self, ts: Timestamp) {
        self.commit_write_timestamp(ts);
    }

    pub fn commit_write_timestamp(&self, ts: Timestamp) {
        self.finish_write_timestamp(ts, WriteTimestampState::Committed);
    }

    pub fn abort_write_timestamp(&self, ts: Timestamp) {
        self.finish_write_timestamp(ts, WriteTimestampState::Aborted);
    }

    fn finish_write_timestamp(&self, ts: Timestamp, state: WriteTimestampState) {
        let _guard = self.lock.lock();
        let mut states = self.write_states.lock();
        if let Some(entry) = states.get_mut(&ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = state;
                let _ = self.snapshot_tracker.release_snapshot(ts);
                self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
            }
        }

        let mut frontier = self.read_ts.load(Ordering::SeqCst);
        loop {
            let next = frontier.saturating_add(1);
            match states.get(&next).copied() {
                Some(WriteTimestampState::Committed | WriteTimestampState::Aborted) => {
                    frontier = next;
                    states.remove(&next);
                }
                _ => break,
            }
        }
        self.read_ts.store(frontier, Ordering::SeqCst);
        drop(states);
        drop(_guard);
        self.condvar.notify_all();
    }

    pub fn pending_count(&self) -> i32 {
        self.pending_reqs.load(Ordering::SeqCst)
    }

    pub fn get_safe_gc_timestamp(&self) -> Timestamp {
        self.snapshot_tracker.min_active_snapshot()
    }

    pub fn get_safe_gc_timestamp_with_margin(&self, margin: Timestamp) -> Timestamp {
        let safe_ts = self.snapshot_tracker.min_active_snapshot();
        safe_ts.saturating_sub(margin)
    }

    /// Get the snapshot tracker for explicit snapshot management
    pub fn snapshot_tracker(&self) -> &SnapshotTracker {
        &self.snapshot_tracker
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
        vm.release_write_timestamp(ts2);
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
        vm.release_write_timestamp(ts1);
        assert_eq!(tracker.cleanup_threshold(), ts2);

        // Release second
        vm.release_write_timestamp(ts2);
        assert_eq!(tracker.cleanup_threshold(), ts3);

        // Release last
        vm.release_write_timestamp(ts3);
        assert_eq!(tracker.cleanup_threshold(), u32::MAX); // No active snapshots
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
}

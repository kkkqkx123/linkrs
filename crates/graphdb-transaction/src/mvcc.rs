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
use graphdb_core::types::Timestamp;

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
}

pub type VersionManagerResult<T> = Result<T, VersionManagerError>;

#[derive(Debug, Clone)]
pub struct VersionManagerConfig {
    pub max_concurrent_reads: u32,
    pub wait_timeout: Duration,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteTimestampState {
    Pending,
    Committed,
    Aborted,
}

/// Observable state of one write-timestamp slot.
///
/// `Vanished` covers every timestamp absent from the slot map: slots of
/// long-committed writes are removed when the read frontier advances over a
/// run of terminal slots, and read-only snapshots never own a write slot at
/// all. Vanished slots carry no pending write, so visibility predicates can
/// trust the plain timestamp comparison for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampSlot {
    Pending,
    Committed,
    Aborted,
    Vanished,
}

impl VersionManager {
    pub fn new() -> Self {
        Self::with_config(VersionManagerConfig::default())
    }

    pub fn with_config(config: VersionManagerConfig) -> Self {
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
        }
    }

    pub fn init_ts(&self, ts: Timestamp) {
        // `write_ts` is the last allocated timestamp. Keeping the baseline at
        // the recovered timestamp makes the next allocation checked and
        // contiguous, including at the u64 boundary.
        self.write_ts.store(ts, Ordering::Release);
        self.read_ts.store(ts, Ordering::Release);
        self.write_states.lock().clear();
        // Restart rebuild: pins held by the previous process are gone with
        // it, so the tracker must restart empty. Otherwise the minimum
        // stays behind and GC stalls forever.
        self.snapshot_tracker.clear();
    }

    pub fn clear(&self) {
        // Preserve write_ts so that subsequent writes and checkpoints
        // use timestamps >= the compact timestamp, ensuring persisted
        // data remains visible after reload.
        self.read_ts.store(0, Ordering::Release);
        self.read_pending.store(0, Ordering::Relaxed);
        self.write_pending.store(0, Ordering::Relaxed);
        self.write_states.lock().clear();
        // The counters above are meaningless while stale pins survive:
        // drain the tracker as well so the safe-GC waterfront is not
        // pinned by snapshots that no longer exist.
        self.snapshot_tracker.clear();
    }

    pub fn write_timestamp(&self) -> Timestamp {
        self.write_ts.load(Ordering::Acquire)
    }

    /// Allocate the next write timestamp.
    pub fn next_write_timestamp(&self) -> VersionManagerResult<Timestamp> {
        self.try_next_write_timestamp()
    }

    pub fn try_next_write_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let ts = self.reserve_timestamp()?;
        self.write_pending.fetch_add(1, Ordering::Relaxed);
        self.snapshot_tracker
            .add_snapshot(ts)
            .map_err(|_| VersionManagerError::SnapshotTrackingFailed)?;
        self.write_states
            .lock()
            .insert(ts, (Instant::now(), WriteTimestampState::Pending));
        Ok(ts)
    }

    fn reserve_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut current = self.write_ts.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(1)
                .ok_or(VersionManagerError::TimestampExhausted)?;
            // The allocator shares the u64 domain with sentinel values, so it
            // must stop before either sentinel instead of handing one out as a
            // transaction timestamp.
            debug_assert!(
                graphdb_core::types::is_allocatable_timestamp(next),
                "timestamp allocator reached reserved sentinel"
            );
            if !graphdb_core::types::is_allocatable_timestamp(next) {
                return Err(VersionManagerError::TimestampExhausted);
            }
            match self
                .write_ts
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(next),
                Err(observed) => current = observed,
            }
        }
    }

    pub fn read_timestamp(&self) -> Timestamp {
        self.read_ts.load(Ordering::Acquire)
    }

    pub fn acquire_read_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut guard = self.read_lock.lock();
        loop {
            let pr = self.read_pending.load(Ordering::Relaxed);
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
                self.read_pending.fetch_add(1, Ordering::Relaxed);
                let ts = self.read_ts.load(Ordering::Acquire);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.read_pending.fetch_sub(1, Ordering::Relaxed);
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
            let pr = self.read_pending.load(Ordering::Relaxed);
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
                self.read_pending.fetch_add(1, Ordering::Relaxed);
                let ts = self.read_ts.load(Ordering::Acquire);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.read_pending.fetch_sub(1, Ordering::Relaxed);
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

    pub fn release_read_timestamp(&self) {
        let ts = self.read_ts.load(Ordering::Acquire);
        self.release_read_timestamp_at(ts);
    }

    pub fn release_read_timestamp_at(&self, ts: Timestamp) {
        if let Err(e) = self.snapshot_tracker.release_snapshot(ts) {
            log::error!("Failed to release snapshot {}: {}", ts, e);
            // Continue anyway - we still need to decrement read_pending
        }
        self.read_pending.fetch_sub(1, Ordering::Relaxed);
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
        self.write_pending.fetch_add(1, Ordering::Relaxed);
        drop(_guard);
        Ok(ts)
    }

    pub fn abort_write_timestamp(&self, ts: Timestamp) {
        // Retires a timestamp that will never become visible. Aborting a
        // user transaction must go through the manager abort protocol so
        // SSI locks, leases and undo logs are released together with the
        // timestamp.
        self.finish_write_timestamp(ts, WriteTimestampState::Aborted);
    }

    /// Settle a system write timestamp in commit order.
    ///
    /// Reserves a commit timestamp for `start` and publishes visibility
    /// over both slots, so system commits share the commit-ordered
    /// coordinate with explicit transactions. Fails closed when the start
    /// slot is not a live pending write; callers must propagate the error
    /// instead of falling back to start-ordered publishing.
    pub fn commit_ordered(&self, start: Timestamp) -> VersionManagerResult<Timestamp> {
        let commit_ts = self.reserve_commit_timestamp(start)?;
        self.publish_reserved_commit(start, commit_ts);
        Ok(commit_ts)
    }

    /// Allocate a commit timestamp at commit time and retire the start slot.
    ///
    /// This restores Ladybug semantics (`commitTS = ++lastTimestamp`): read
    /// visibility is ordered by commit time, not by transaction start time.
    /// The start slot is retired as `Committed` (releasing its snapshot and
    /// pending count) and the freshly allocated commit timestamp is inserted
    /// as `Committed`, so the read frontier advances over both in one step.
    /// Out-of-order commits therefore never pin the frontier behind a
    /// still-pending start timestamp: every allocated timestamp reaches a
    /// terminal state exactly once, at commit time.
    ///
    /// Must only be called after the commit is durable (WAL) and storage
    /// finalization succeeded; otherwise unfinalized writes would become
    /// visible to new readers.
    ///
    /// Prefer the split `reserve_commit_timestamp` /
    /// `publish_reserved_commit` pair for new commit paths: reserving
    /// before finalization lets conflict certification index the exact
    /// commit timestamp while keeping visibility gated on finalization.
    /// This combined helper stays for recovery re-drives and tests.
    pub fn allocate_commit_timestamp(
        &self,
        start_ts: Timestamp,
    ) -> VersionManagerResult<Timestamp> {
        let _guard = self.write_lock.lock();
        let commit_ts = self.reserve_timestamp()?;
        let mut states = self.write_states.lock();
        if let Some((_, entry)) = states.get_mut(&start_ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = WriteTimestampState::Committed;
                let _ = self.snapshot_tracker.release_snapshot(start_ts);
                self.write_pending.fetch_sub(1, Ordering::Relaxed);
            }
        }
        states.insert(commit_ts, (Instant::now(), WriteTimestampState::Committed));
        self.advance_read_frontier(&mut states);
        drop(states);
        self.write_condvar.notify_all();
        Ok(commit_ts)
    }

    /// Reserve a commit timestamp before storage finalization.
    ///
    /// The reserved timestamp is `Pending`, so the read frontier cannot
    /// cross it: visibility stays gated even though the exact commit
    /// timestamp is already known to conflict certification. The caller
    /// must settle the reservation exactly once, either with
    /// `publish_reserved_commit` on success or with
    /// `abort_write_timestamp` on failure (both are idempotent by slot
    /// state, so double-settling is safe).
    ///
    /// Refuses when the start slot is not a live `Pending` write: ordering
    /// visibility against a vanished owner would publish ownerless writes.
    pub fn reserve_commit_timestamp(&self, start_ts: Timestamp) -> VersionManagerResult<Timestamp> {
        let _guard = self.write_lock.lock();
        let mut states = self.write_states.lock();
        match states.get(&start_ts).map(|(_, state)| *state) {
            Some(WriteTimestampState::Pending) => {}
            _ => return Err(VersionManagerError::InvalidTimestamp(start_ts)),
        }
        let commit_ts = self.reserve_timestamp()?;
        states.insert(commit_ts, (Instant::now(), WriteTimestampState::Pending));
        Ok(commit_ts)
    }

    /// Settle a reservation made by `reserve_commit_timestamp`.
    ///
    /// Retires the start slot and marks the reserved commit timestamp
    /// `Committed`, then advances the read frontier over both. Must only
    /// be called after WAL durability and storage finalization succeeded.
    /// Tolerates missing slots (restart re-drives) by treating them as
    /// already settled instead of failing the commit.
    pub fn publish_reserved_commit(&self, start_ts: Timestamp, commit_ts: Timestamp) {
        let _guard = self.write_lock.lock();
        let mut states = self.write_states.lock();
        if let Some((_, entry)) = states.get_mut(&start_ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = WriteTimestampState::Committed;
                let _ = self.snapshot_tracker.release_snapshot(start_ts);
                self.write_pending.fetch_sub(1, Ordering::Relaxed);
            }
        }
        if let Some((_, entry)) = states.get_mut(&commit_ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = WriteTimestampState::Committed;
            }
        } else {
            states.insert(commit_ts, (Instant::now(), WriteTimestampState::Committed));
        }
        self.advance_read_frontier(&mut states);
        drop(states);
        self.write_condvar.notify_all();
    }

    fn finish_write_timestamp(&self, ts: Timestamp, state: WriteTimestampState) {
        let mut states = self.write_states.lock();
        if let Some((_, entry)) = states.get_mut(&ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = state;
                let _ = self.snapshot_tracker.release_snapshot(ts);
                self.write_pending.fetch_sub(1, Ordering::Relaxed);
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
        let mut frontier = self.read_ts.load(Ordering::Acquire);
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
        self.read_ts.store(frontier, Ordering::Release);
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
                    self.write_pending.fetch_sub(1, Ordering::Relaxed);
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
        self.read_pending.load(Ordering::Relaxed) + self.write_pending.load(Ordering::Relaxed)
    }

    /// Return the observable state of one write-timestamp slot.
    ///
    /// Read-only probe for pending-aware visibility: storage asks about the
    /// creation/deletion stamp it just read and hides stamps owned by foreign
    /// pending transactions. Absent slots report `Vanished`; they were either
    /// reclaimed after the read frontier swallowed a run of terminal slots
    /// (their data is undone or committed and the plain predicate applies) or
    /// never owned a write slot at all.
    pub fn timestamp_slot(&self, ts: Timestamp) -> TimestampSlot {
        match self.write_states.lock().get(&ts).map(|(_, state)| *state) {
            Some(WriteTimestampState::Pending) => TimestampSlot::Pending,
            Some(WriteTimestampState::Committed) => TimestampSlot::Committed,
            Some(WriteTimestampState::Aborted) => TimestampSlot::Aborted,
            None => TimestampSlot::Vanished,
        }
    }

    /// Ages of all currently `Pending` write timestamps.
    ///
    /// Lets the cleanup path tell a live long-running transaction (its
    /// timestamp appears in the caller-supplied `owned` set and must never be
    /// reaped, but it still pins the read frontier) apart from a crash
    /// orphan (unowned and eligible for reaping). Sorted oldest-first so the
    /// worst frontier blocker is reported first.
    pub fn pending_write_ages(&self) -> Vec<(Timestamp, Duration)> {
        let now = Instant::now();
        let states = self.write_states.lock();
        let mut ages: Vec<(Timestamp, Duration)> = states
            .iter()
            .filter(|(_, (_, state))| *state == WriteTimestampState::Pending)
            .map(|(ts, (acquired, _))| (*ts, now.duration_since(*acquired)))
            .collect();
        ages.sort_by_key(|(_, age)| *age);
        ages.reverse();
        ages
    }

    /// Timeout after which a `Pending` write timestamp is reaped by
    /// [`VersionManager::reap_expired_write_timestamps`] as stale.
    pub fn write_reap_timeout(&self) -> Duration {
        self.config.write_reap_timeout
    }

    pub fn get_safe_gc_timestamp(&self) -> Timestamp {
        self.snapshot_tracker.min_active_snapshot()
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
        vm.commit_ordered(ts2).expect("ordered commit");
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
    fn test_insert_timestamp_acquire_commit_abort() {
        let vm = Arc::new(VersionManager::new());

        // Committed system write leaves no pending slot.
        let ts = vm
            .acquire_insert_timestamp()
            .expect("acquire should succeed");
        assert!(ts >= 1);
        vm.commit_ordered(ts).expect("ordered commit");
        assert_eq!(vm.pending_count(), 0);

        // Aborted system write (drop without commit) leaves no pending slot.
        let ts = vm
            .acquire_insert_timestamp()
            .expect("acquire should succeed");
        vm.abort_write_timestamp(ts);
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
                let ts = vm_clone
                    .acquire_insert_timestamp()
                    .expect("acquire should succeed");
                thread::sleep(Duration::from_millis(10));
                vm_clone.commit_ordered(ts).expect("ordered commit");
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
        vm.commit_ordered(ts1).expect("ordered commit");
        assert_eq!(tracker.cleanup_threshold(), ts2);

        // Release second
        vm.commit_ordered(ts2).expect("ordered commit");
        assert_eq!(tracker.cleanup_threshold(), ts3);

        // Release last
        vm.commit_ordered(ts3).expect("ordered commit");
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

        vm.commit_ordered(second).expect("ordered commit");
        assert_eq!(vm.read_timestamp(), first - 1);
        assert_eq!(vm.pending_count(), 1);

        let first_commit = vm.commit_ordered(first).expect("ordered commit");
        assert_eq!(vm.read_timestamp(), first_commit);
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
        let commit_ts = vm.commit_ordered(write_timestamp).expect("ordered commit");

        assert_eq!(vm.read_timestamp(), commit_ts);
        drop(guard);
        assert_eq!(vm.snapshot_tracker().ref_count(timestamp), None);
        assert_eq!(vm.pending_count(), 0);
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
        vm.commit_ordered(second).expect("ordered commit");
        assert_eq!(vm.read_timestamp(), first - 1);
        assert_eq!(vm.pending_count(), 1);

        // Settling an already-settled slot fails closed instead of silently
        // re-publishing: the caller must not retry a finished commit.
        assert!(vm.commit_ordered(second).is_err());
        assert_eq!(vm.read_timestamp(), first - 1);

        let first_commit = vm.commit_ordered(first).expect("ordered commit");
        assert_eq!(vm.read_timestamp(), first_commit);
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

    #[test]
    fn test_pending_write_ages_reports_oldest_first() {
        let vm = VersionManager::new();
        let first = vm
            .acquire_insert_timestamp()
            .expect("first write timestamp");
        let second = vm
            .acquire_insert_timestamp()
            .expect("second write timestamp");
        vm.backdate_write_timestamp(first, Duration::from_secs(120));
        vm.backdate_write_timestamp(second, Duration::from_secs(10));

        let ages = vm.pending_write_ages();
        assert_eq!(ages.len(), 2);
        // Oldest blocker first so cleanup logs the frontier pin immediately.
        assert_eq!(ages[0].0, first);
        assert!(ages[0].1 >= Duration::from_secs(120));
        assert_eq!(ages[1].0, second);

        vm.commit_ordered(first).expect("ordered commit");
        let ages = vm.pending_write_ages();
        assert_eq!(ages.len(), 1);
        assert_eq!(ages[0].0, second);
    }

    #[test]
    fn test_reserve_publish_split_keeps_frontier_gated() {
        let vm = VersionManager::new();
        let first = vm
            .acquire_insert_timestamp()
            .expect("first write timestamp");
        let second = vm
            .acquire_insert_timestamp()
            .expect("second write timestamp");

        // Reserving exposes the exact commit timestamp to certification
        // while the frontier stays gated behind both pending slots.
        let commit_ts = vm.reserve_commit_timestamp(second).expect("reserve");
        assert!(commit_ts > second);
        assert_eq!(vm.read_timestamp(), first - 1);

        // Publishing settles both slots at once, ordered by commit time.
        // The frontier still waits behind the first pending write.
        vm.publish_reserved_commit(second, commit_ts);
        assert_eq!(vm.read_timestamp(), first - 1);

        let first_commit = vm.commit_ordered(first).expect("ordered commit");
        assert!(first_commit > commit_ts);
        assert_eq!(vm.read_timestamp(), first_commit);
        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_reserve_refuses_dead_start_slot() {
        let vm = VersionManager::new();
        let start = vm.acquire_insert_timestamp().expect("write timestamp");
        vm.abort_write_timestamp(start);
        assert!(matches!(
            vm.reserve_commit_timestamp(start),
            Err(VersionManagerError::InvalidTimestamp(_))
        ));
    }

    #[test]
    fn test_clear_drains_snapshot_tracker() {
        let vm = VersionManager::new();
        let _ = vm.acquire_read_timestamp().expect("read timestamp");
        assert_eq!(vm.snapshot_tracker().active_count(), 1);

        vm.clear();
        assert_eq!(vm.snapshot_tracker().active_count(), 0);
        assert_eq!(
            vm.snapshot_tracker().min_active_snapshot(),
            crate::mvcc_watermarks::NO_ACTIVE_SNAPSHOT
        );
        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_init_ts_drains_snapshot_tracker() {
        let vm = VersionManager::new();
        let _ = vm.acquire_read_timestamp().expect("read timestamp");
        assert_ne!(
            vm.snapshot_tracker().min_active_snapshot(),
            crate::mvcc_watermarks::NO_ACTIVE_SNAPSHOT
        );
        vm.init_ts(100);
        assert_eq!(
            vm.snapshot_tracker().min_active_snapshot(),
            crate::mvcc_watermarks::NO_ACTIVE_SNAPSHOT
        );
        assert_eq!(vm.read_timestamp(), 100);
    }

    #[test]
    fn test_timestamp_slot_lifecycle() {
        use super::TimestampSlot;
        let vm = VersionManager::new();
        let first = vm.acquire_insert_timestamp().expect("first write");
        let second = vm.acquire_insert_timestamp().expect("second write");
        assert_eq!(vm.timestamp_slot(first), TimestampSlot::Pending);
        assert_eq!(vm.timestamp_slot(second), TimestampSlot::Pending);

        vm.abort_write_timestamp(second);
        assert_eq!(vm.timestamp_slot(second), TimestampSlot::Aborted);

        // Committing out of order cannot cross the still-pending first slot,
        // so the aborted second slot stays observable until the frontier
        // swallows the whole run.
        vm.commit_ordered(first).expect("ordered commit");
        assert_eq!(vm.timestamp_slot(first), TimestampSlot::Vanished);
        assert_eq!(vm.timestamp_slot(second), TimestampSlot::Vanished);

        // Never-allocated stamps carry no pending write either.
        assert_eq!(vm.timestamp_slot(u64::MAX - 1), TimestampSlot::Vanished);
    }
}

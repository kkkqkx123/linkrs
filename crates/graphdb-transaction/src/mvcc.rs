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
//!
//! ## Layout
//!
//! Pure types live in [`types`], timestamp allocation in [`allocator`], read
//! admission and the RAII guard in [`read`], the write slot state machine in
//! [`write`], and frontier advancement, reaping and resets in [`frontier`].

mod allocator;
mod frontier;
mod read;
mod types;
mod write;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use super::snapshot_tracker::SnapshotTracker;
use graphdb_core::types::Timestamp;

use types::WriteTimestampState;

pub use read::ReadTimestampGuard;
pub use types::{
    TimestampSlot, VersionManagerConfig, VersionManagerError, VersionManagerResult,
    RELEASED_TIMESTAMP,
};

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

    pub fn pending_count(&self) -> i32 {
        self.read_pending.load(Ordering::Relaxed) + self.write_pending.load(Ordering::Relaxed)
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
}

impl Default for VersionManager {
    fn default() -> Self {
        Self::new()
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

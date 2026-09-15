//! Unified MVCC watermarks
//!
//! Provides a single snapshot of GC-related frontiers:
//! `oldest_active_snapshot`, `last_published_commit`,
//! `checkpoint_snapshot` and `wal_reclaim_lsn`. All GC call sites derive
//! their cutoff from this structure instead of interpreting sentinel values
//! independently.

use std::sync::Arc;

use graphdb_core::types::{CommitLsn, Timestamp};

use crate::mvcc::VersionManager;

/// Sentinel value returned by `SnapshotTracker::cleanup_threshold` when no
/// snapshot is active.  Callers must not forward this value directly to GC;
/// it must be resolved through `MvccWatermarks`.
pub const NO_ACTIVE_SNAPSHOT: Timestamp = u64::MAX;

/// Unified view over MVCC frontiers. Immutable once computed; callers fix
/// watermarks at GC-pass start and reuse them for all table types in that
/// pass so a prefix reclaim cannot change the cutoff for a later type in the
/// same pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MvccWatermarks {
    /// Minimum active snapshot timestamp, or `NO_ACTIVE_SNAPSHOT` when no
    /// transaction or read holds a snapshot. See `VersionManager`.
    pub oldest_active_snapshot: Timestamp,
    /// Highest timestamp that has been published as committed and whose
    /// writes are visible to new readers (`read_ts` frontier).  Used as an
    /// upper bound when no snapshot is active.
    pub last_published_commit: Timestamp,
    /// Snapshot timestamp captured at the start of the current checkpoint, if
    /// any.  Checkpoint output includes exactly the data visible at this
    /// timestamp.  `None` when no checkpoint is active.
    pub checkpoint_snapshot: Option<Timestamp>,
    /// WAL reclaim LSN derived from the last published checkpoint manifest
    /// and the sync outbox safe LSN.  WAL segments with LSN <= this value
    /// have been incorporated into a checkpoint and can be reclaimed.
    /// `CommitLsn::ZERO` disables WAL reclaim until a checkpoint publishes.
    pub wal_reclaim_lsn: CommitLsn,
}

impl MvccWatermarks {
    /// Compute watermarks from the live `VersionManager` and optional
    /// checkpoint / WAL frontier inputs. Must be called after the read side
    /// has registered any new statement snapshot so the snapshot set is
    /// consistent with the frontier the statement will observe.
    pub fn capture(
        version_manager: &VersionManager,
        checkpoint_snapshot: Option<Timestamp>,
        wal_reclaim_lsn: Option<CommitLsn>,
    ) -> Self {
        let oldest_active_snapshot = version_manager.snapshot_tracker().cleanup_threshold();
        let last_published_commit = version_manager.read_timestamp();
        Self {
            oldest_active_snapshot,
            last_published_commit,
            checkpoint_snapshot,
            wal_reclaim_lsn: wal_reclaim_lsn.unwrap_or(CommitLsn::ZERO),
        }
    }

    pub fn from_parts(
        oldest_active_snapshot: Timestamp,
        last_published_commit: Timestamp,
        checkpoint_snapshot: Option<Timestamp>,
        wal_reclaim_lsn: CommitLsn,
    ) -> Self {
        Self {
            oldest_active_snapshot,
            last_published_commit,
            checkpoint_snapshot,
            wal_reclaim_lsn,
        }
    }

    /// Safe GC timestamp for version chains and tombstones.
    ///
    /// The returned value is exclusive: versions with `end_ts <= safe_gc_ts`
    /// are reclaimable (see `Visibility::is_gc_eligible` in graphdb-storage).
    /// Callers must apply any configured margin themselves so the policy is
    /// uniform across table types. All layers share this bound; the margin in
    /// `safe_gc_timestamp_with_margin` absorbs the race between watermark
    /// capture and GC execution.
    pub fn safe_gc_timestamp(&self) -> Timestamp {
        if self.oldest_active_snapshot == NO_ACTIVE_SNAPSHOT {
            self.last_published_commit
        } else {
            self.oldest_active_snapshot
        }
    }

    pub fn safe_gc_timestamp_with_margin(&self, margin: Timestamp) -> Timestamp {
        self.safe_gc_timestamp().saturating_sub(margin)
    }

    /// Whether there is at least one active snapshot pinning history.
    pub fn has_active_snapshot(&self) -> bool {
        self.oldest_active_snapshot != NO_ACTIVE_SNAPSHOT
    }

    /// Whether the checkpoint frontier is available (a checkpoint is in
    /// progress and has captured its snapshot).  Callers that advance the
    /// retention frontier or WAL reclaim LSN must verify this is `Some`
    /// before reclaiming files that depend on the published manifest.
    pub fn has_checkpoint_snapshot(&self) -> bool {
        self.checkpoint_snapshot.is_some()
    }

    /// Whether WAL reclaim can proceed (valid checkpoint + non-zero LSN).
    pub fn can_reclaim_wal(&self) -> bool {
        self.checkpoint_snapshot.is_some() && self.wal_reclaim_lsn != CommitLsn::ZERO
    }

    /// Diagnostic age of the oldest active snapshot, if any.
    pub fn oldest_age(&self, vm: &VersionManager) -> Option<std::time::Duration> {
        vm.snapshot_tracker().oldest_age()
    }
}

/// Helper that captures watermarks via an `Arc<VersionManager>`.
pub fn capture_watermarks(
    version_manager: &Arc<VersionManager>,
    checkpoint_snapshot: Option<Timestamp>,
    wal_reclaim_lsn: Option<CommitLsn>,
) -> MvccWatermarks {
    MvccWatermarks::capture(version_manager, checkpoint_snapshot, wal_reclaim_lsn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvcc::VersionManager;

    #[test]
    fn safe_gc_with_no_active_snapshot_uses_last_published() {
        let vm = VersionManager::new();
        let ts = vm.acquire_insert_timestamp().unwrap();
        vm.commit_ordered(ts).expect("ordered commit");
        let wm = MvccWatermarks::capture(&vm, None, None);
        assert_eq!(wm.oldest_active_snapshot, NO_ACTIVE_SNAPSHOT);
        assert_eq!(wm.safe_gc_timestamp(), vm.read_timestamp());
    }

    #[test]
    fn safe_gc_with_active_snapshot_uses_min() {
        let vm = VersionManager::new();
        let ts = vm.acquire_insert_timestamp().unwrap();
        // pending write pins oldest_active
        let wm = MvccWatermarks::capture(&vm, None, None);
        assert_eq!(wm.oldest_active_snapshot, ts);
        assert_eq!(wm.safe_gc_timestamp(), ts);
        vm.commit_ordered(ts).expect("ordered commit");
    }

    #[test]
    fn watermark_margin_applied() {
        let wm = MvccWatermarks::from_parts(100, 100, None, CommitLsn::ZERO);
        assert_eq!(wm.safe_gc_timestamp_with_margin(1), 99);
        assert_eq!(wm.safe_gc_timestamp_with_margin(200), 0);
    }

    #[test]
    fn wal_reclaim_requires_checkpoint() {
        let wm_no_cp =
            MvccWatermarks::from_parts(NO_ACTIVE_SNAPSHOT, 10, None, CommitLsn::new(100));
        assert!(!wm_no_cp.can_reclaim_wal());
        let wm_with_cp =
            MvccWatermarks::from_parts(NO_ACTIVE_SNAPSHOT, 10, Some(10), CommitLsn::new(100));
        assert!(wm_with_cp.can_reclaim_wal());
    }
}

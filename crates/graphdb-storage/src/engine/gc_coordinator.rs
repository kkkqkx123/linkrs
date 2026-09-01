use std::sync::Arc;

use graphdb_core::types::CommitLsn;
use graphdb_core::types::Timestamp;
use graphdb_transaction::{MvccWatermarks, VersionManager};

/// Per-pass GC statistics for a single storage sub-system.
#[derive(Debug, Clone, Default)]
pub struct GcPassStats {
    pub safe_gc_timestamp: Timestamp,
    pub vertex_entries_removed: usize,
    pub edge_tombstones_removed: usize,
    pub column_versions_removed: usize,
    pub index_entries_removed: usize,
    pub wal_segments_reclaimable: usize,
}

impl GcPassStats {
    pub fn total_removed(&self) -> usize {
        self.vertex_entries_removed
            + self.edge_tombstones_removed
            + self.column_versions_removed
            + self.index_entries_removed
    }

    pub fn is_empty(&self) -> bool {
        self.total_removed() == 0 && self.wal_segments_reclaimable == 0
    }
}

/// Diagnostic view over the current MVCC GC state.
#[derive(Debug, Clone)]
pub struct GcDiagnostics {
    pub watermarks: MvccWatermarks,
    pub safe_gc_timestamp: Timestamp,
    pub has_active_snapshot: bool,
    pub oldest_snapshot_age: Option<std::time::Duration>,
    pub active_snapshot_count: usize,
    pub oldest_snapshot_ts: Timestamp,
    pub pending_writes: i32,
    pub version_chain_bytes: u64,
    pub index_tombstone_count: usize,
    pub edge_tombstone_count: usize,
    pub blocked_bytes_estimate: u64,
    pub long_transaction_warning: Option<String>,
}

impl GcDiagnostics {
    pub fn is_blocked(&self) -> bool {
        self.has_active_snapshot && self.oldest_snapshot_age.is_some_and(|d| d.as_secs() > 30)
    }
}

pub struct GcCoordinator {
    version_manager: Arc<VersionManager>,
    config_margin: Timestamp,
    checkpoint_snapshot: Option<Timestamp>,
    wal_reclaim_lsn: Option<CommitLsn>,
}

impl GcCoordinator {
    pub fn new(version_manager: Arc<VersionManager>) -> Self {
        Self {
            version_manager,
            config_margin: 1,
            checkpoint_snapshot: None,
            wal_reclaim_lsn: None,
        }
    }

    pub fn with_margin(mut self, margin: Timestamp) -> Self {
        self.config_margin = margin;
        self
    }

    pub fn with_checkpoint_snapshot(mut self, ts: Option<Timestamp>) -> Self {
        self.checkpoint_snapshot = ts;
        self
    }

    pub fn with_wal_reclaim_lsn(mut self, lsn: Option<CommitLsn>) -> Self {
        self.wal_reclaim_lsn = lsn;
        self
    }

    pub fn capture_watermarks(&self) -> MvccWatermarks {
        MvccWatermarks::capture(
            &self.version_manager,
            self.checkpoint_snapshot,
            self.wal_reclaim_lsn,
        )
    }

    /// Safe GC timestamp for this pass, applying the configured margin.
    pub fn safe_gc_timestamp(&self) -> Timestamp {
        self.capture_watermarks()
            .safe_gc_timestamp_with_margin(self.config_margin)
    }

    /// Capture diagnostics for observability without performing GC.
    pub fn diagnostics(&self) -> GcDiagnostics {
        let watermarks = self.capture_watermarks();
        let safe_gc_timestamp = watermarks.safe_gc_timestamp_with_margin(self.config_margin);
        let has_active_snapshot = watermarks.has_active_snapshot();
        let oldest_snapshot_age = watermarks.oldest_age(&self.version_manager);
        let active_snapshot_count = self.version_manager.snapshot_tracker().active_count() as usize;
        let oldest_snapshot_ts = watermarks.oldest_active_snapshot;
        let pending_writes = self.version_manager.pending_count();
        let long_transaction_warning = if has_active_snapshot {
            oldest_snapshot_age.and_then(|age| {
                if age.as_secs() > 30 {
                    Some(format!(
                        "long-lived snapshot age={:?} oldest_ts={} safe_gc={} pending_writes={}",
                        age, oldest_snapshot_ts, safe_gc_timestamp, pending_writes
                    ))
                } else {
                    None
                }
            })
        } else {
            None
        };
        if let Some(ref warn) = long_transaction_warning {
            log::warn!("GC diagnostics: {}", warn);
        }
        GcDiagnostics {
            watermarks,
            safe_gc_timestamp,
            has_active_snapshot,
            oldest_snapshot_age,
            active_snapshot_count,
            oldest_snapshot_ts,
            pending_writes,
            version_chain_bytes: 0,
            index_tombstone_count: 0,
            edge_tombstone_count: 0,
            blocked_bytes_estimate: 0,
            long_transaction_warning,
        }
    }

    /// Whether WAL / old checkpoint files can be reclaimed at the current watermarks.
    pub fn can_reclaim_wal(&self) -> bool {
        self.capture_watermarks().can_reclaim_wal()
    }
}

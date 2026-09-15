use std::sync::Arc;

use graphdb_core::types::CommitLsn;
use graphdb_core::types::Timestamp;
use graphdb_transaction::{MvccWatermarks, VersionManager};

/// Diagnostic view over the current MVCC GC state.
#[derive(Debug, Clone)]
pub struct GcDiagnostics {
    pub watermarks: MvccWatermarks,
    pub safe_gc_timestamp: Timestamp,
    pub has_active_snapshot: bool,
    pub oldest_snapshot_age: Option<std::time::Duration>,
    pub active_snapshot_count: usize,
    pub long_transaction_warning: Option<String>,
}

impl GcDiagnostics {
    pub fn is_blocked(&self) -> bool {
        self.has_active_snapshot && self.oldest_snapshot_age.is_some_and(|d| d.as_secs() > 30)
    }
}

/// Coordinates MVCC garbage collection across table types.
///
/// The coordinator fixes one watermark per pass (`capture_watermarks`) and
/// derives every table cutoff from it, so reclaiming vertices cannot move the
/// cutoff used for edges later in the same pass. `config_margin` (default 1)
/// is subtracted from the waterfront: GC then lags one timestamp behind the
/// oldest active snapshot, absorbing the race between watermark capture and
/// GC execution at the cost of keeping one extra round of history. Passes run
/// on the storage background pool (5s period, 500ms minimum); a pass that
/// finds `safe == 0` is a no-op. Garbage that survives a pass is therefore
/// "retained by margin or by a live snapshot", never a leak — check
/// `diagnostics()` before investigating.
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
        let active_snapshot_count = self.version_manager.snapshot_tracker().active_count();
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
            long_transaction_warning,
        }
    }
}

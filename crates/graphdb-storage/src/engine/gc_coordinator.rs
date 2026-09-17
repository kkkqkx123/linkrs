use std::path::PathBuf;
use std::sync::Arc;

use graphdb_core::types::CommitLsn;
use graphdb_core::types::Timestamp;
use graphdb_sync::checkpoint_manifest::CheckpointManifestManager;
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
    /// Manifest directory for self-healing lazy loads. When the explicit
    /// checkpoint fields above are empty (e.g. first pass after a restart),
    /// the watermark is read once from the latest published manifest instead
    /// of adding a cross-module call chain into the checkpoint publisher.
    manifest_dir: Option<PathBuf>,
}

impl GcCoordinator {
    pub fn new(version_manager: Arc<VersionManager>) -> Self {
        Self {
            version_manager,
            config_margin: 1,
            checkpoint_snapshot: None,
            wal_reclaim_lsn: None,
            manifest_dir: None,
        }
    }

    pub fn with_margin(mut self, margin: Timestamp) -> Self {
        self.config_margin = margin;
        self
    }

    /// Builder wiring the manifest directory for self-healing lazy loads.
    pub fn with_manifest_dir(mut self, dir: PathBuf) -> Self {
        self.manifest_dir = Some(dir);
        self
    }

    /// Explicit refresh entry for the checkpoint watermark.
    ///
    /// Checkpoint completion (via the persistence watermark cell) and GC
    /// construction sites feed fresh values here; when never refreshed, the
    /// capture below falls back to one lazy manifest load.
    pub fn refresh_checkpoint_watermark(&mut self, snapshot: Timestamp, reclaim_lsn: CommitLsn) {
        self.checkpoint_snapshot = Some(snapshot);
        self.wal_reclaim_lsn = Some(reclaim_lsn);
    }

    pub fn capture_watermarks(&self) -> MvccWatermarks {
        if self.checkpoint_snapshot.is_some() {
            return MvccWatermarks::capture(
                &self.version_manager,
                self.checkpoint_snapshot,
                self.wal_reclaim_lsn,
            );
        }
        let (snapshot, reclaim) = self.load_checkpoint_watermark().unwrap_or((None, None));
        MvccWatermarks::capture(&self.version_manager, snapshot, reclaim)
    }

    /// Best-effort lazy load of the checkpoint watermark from the latest
    /// published manifest. Old manifests without a snapshot field yield
    /// `None` (their LSN-scale commit stamp must never be mistaken for a
    /// timestamp); any IO or parse failure degrades to no watermark with a
    /// debug log, never an error.
    fn load_checkpoint_watermark(&self) -> Option<(Option<Timestamp>, Option<CommitLsn>)> {
        let dir = self.manifest_dir.as_ref()?;
        let manifest = CheckpointManifestManager::new(dir).load_latest().ok()??;
        let snapshot = manifest.snapshot_timestamp;
        snapshot?;
        Some((snapshot, Some(manifest.safe_lsn)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_sync::checkpoint_manifest::CheckpointManifest;

    #[test]
    fn test_wal_reclaim_gated_on_checkpoint_bounds() {
        let vm = Arc::new(VersionManager::new());
        let coordinator = GcCoordinator::new(vm);
        // No checkpoint bounds flow into routine passes: version-GC
        // cutoffs never depend on them, and WAL reclaim stays disabled.
        let wm = coordinator.capture_watermarks();
        assert!(!wm.can_reclaim_wal());
        assert!(!wm.has_checkpoint_snapshot());
    }

    #[test]
    fn test_checkpoint_refresh_enables_wal_reclaim() {
        let vm = Arc::new(VersionManager::new());
        let mut coordinator = GcCoordinator::new(vm);
        assert!(!coordinator.capture_watermarks().can_reclaim_wal());
        coordinator.refresh_checkpoint_watermark(42, CommitLsn::new(100));
        let wm = coordinator.capture_watermarks();
        assert!(wm.has_checkpoint_snapshot());
        assert!(wm.can_reclaim_wal());
        assert_eq!(wm.checkpoint_snapshot, Some(42));
        assert_eq!(wm.wal_reclaim_lsn, CommitLsn::new(100));
    }

    #[test]
    fn test_first_capture_after_restart_lazy_loads_manifest() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let manifest_dir = dir.path().join("manifests");
        let manager = CheckpointManifestManager::new(&manifest_dir);
        manager.init().expect("manifest dir init");
        let storage_path = dir.path().join("checkpoint_1");
        std::fs::create_dir_all(&storage_path).expect("snapshot dir");
        let storage_ref =
            CheckpointManifest::storage_snapshot_from_directory(&storage_path, 1, 0, 0)
                .expect("storage snapshot ref");
        let manifest =
            CheckpointManifest::new(1, CommitLsn::new(500), storage_ref, None, Vec::new())
                .expect("manifest")
                .with_snapshot_timestamp(40)
                .expect("snapshot stamp");
        manager.publish(&manifest).expect("publish");

        // A fresh coordinator (restart: watermark cell empty) heals from the
        // manifest alone.
        let vm = Arc::new(VersionManager::new());
        let coordinator = GcCoordinator::new(vm).with_manifest_dir(manifest_dir);
        let wm = coordinator.capture_watermarks();
        assert_eq!(wm.checkpoint_snapshot, Some(40));
        assert!(wm.can_reclaim_wal());
    }
}

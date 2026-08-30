use graphdb_core::{StorageError, StorageResult};
use graphdb_transaction::wal::Lsn;

use crate::engine::persistence_coordinator::PersistenceState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotStats {
    pub snapshot_count: usize,
    pub total_size_bytes: u64,
    pub latest_snapshot_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistenceDiagnostics {
    pub state: PersistenceState,
    pub checkpoint_sequence: u64,
    pub safe_lsn: Lsn,
    pub last_checkpoint_error: Option<String>,
    pub last_snapshot_error: Option<String>,
    pub temporary_checkpoint_count: usize,
    /// Number of catalog lock acquisitions observed by the storage engine.
    pub catalog_lock_acquisitions: u64,
    /// Total time spent waiting for catalog locks, in nanoseconds.
    pub catalog_lock_wait_nanos: u64,
    /// Total time catalog guards were held, in nanoseconds.
    pub catalog_lock_hold_nanos: u64,
    /// Number of catalog acquisitions that observed measurable contention.
    pub catalog_lock_contentions: u64,
    /// Lock metrics split by catalog operation type.
    pub catalog_lock_by_operation: Vec<CatalogLockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLockDiagnostic {
    pub operation: String,
    pub acquisitions: u64,
    pub wait_nanos: u64,
    pub hold_nanos: u64,
    pub contentions: u64,
}

impl crate::engine::persistence_coordinator::PersistenceCoordinator {
    pub fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool> {
        let snapshot_manager = self
            .snapshot_manager
            .as_ref()
            .ok_or_else(|| StorageError::not_supported("Snapshots are not enabled"))?;

        snapshot_manager.verify_snapshot(snapshot_id)
    }

    pub fn cleanup_old_snapshots(&self) -> StorageResult<usize> {
        let snapshot_manager = self
            .snapshot_manager
            .as_ref()
            .ok_or_else(|| StorageError::not_supported("Snapshots are not enabled"))?;

        snapshot_manager.cleanup_old_snapshots()
    }

    /// Remove published checkpoints older than the retention limit while
    /// keeping the newest valid recovery points.
    pub fn cleanup_old_checkpoints(&self, max_checkpoints: usize) -> StorageResult<usize> {
        let keep = max_checkpoints.max(1);
        let current_sequence = self.checkpoint_manager.read().current_seq();
        let retained_by_snapshot = self
            .snapshot_manager
            .as_ref()
            .map(|manager| manager.retained_checkpoint_sequences())
            .unwrap_or_default();
        let mut checkpoints: Vec<(u64, std::path::PathBuf)> =
            std::fs::read_dir(&self.config.checkpoint_dir)?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    let name = path.file_name()?.to_str()?;
                    let sequence = name.strip_prefix("checkpoint_")?.parse::<u64>().ok()?;
                    path.is_dir().then_some((sequence, path))
                })
                .collect();
        checkpoints.sort_by_key(|(sequence, _)| *sequence);
        let remove_count = checkpoints.len().saturating_sub(keep);
        let mut removed = 0;
        for (_sequence, path) in checkpoints.into_iter().filter(|(sequence, _)| {
            *sequence != current_sequence && !retained_by_snapshot.contains(sequence)
        }) {
            if removed >= remove_count {
                break;
            }
            std::fs::remove_dir_all(path)?;
            removed += 1;
        }
        if removed > 0 {
            Self::sync_directory(&self.config.checkpoint_dir)?;
        }
        Ok(removed)
    }

    pub fn snapshot_stats(&self) -> SnapshotStats {
        if let Some(snapshot_manager) = self.snapshot_manager.as_ref() {
            SnapshotStats {
                snapshot_count: snapshot_manager.snapshot_count(),
                total_size_bytes: snapshot_manager.total_snapshot_size(),
                latest_snapshot_id: snapshot_manager.get_latest_snapshot().map(|info| info.id),
            }
        } else {
            SnapshotStats::default()
        }
    }

    pub fn diagnostics(&self) -> PersistenceDiagnostics {
        let temporary_checkpoint_count = std::fs::read_dir(&self.config.checkpoint_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.ends_with(".tmp"))
                    })
                    .count()
            })
            .unwrap_or(0);
        let manifest_safe_lsn = self
            .manifest_manager
            .latest_safe_lsn()
            .map(|lsn| Lsn::new(lsn.get()))
            .unwrap_or_else(|_| *self.last_checkpoint_lsn.read());
        PersistenceDiagnostics {
            state: *self.state.read(),
            checkpoint_sequence: self.checkpoint_manager.read().current_seq(),
            safe_lsn: manifest_safe_lsn,
            last_checkpoint_error: self.last_checkpoint_error.read().clone(),
            last_snapshot_error: self.last_snapshot_error.read().clone(),
            temporary_checkpoint_count,
            catalog_lock_acquisitions: 0,
            catalog_lock_wait_nanos: 0,
            catalog_lock_hold_nanos: 0,
            catalog_lock_contentions: 0,
            catalog_lock_by_operation: Vec::new(),
        }
    }

    pub fn mark_flushed(&self, lsn: Lsn) {
        *self.last_flush_lsn.write() = lsn;
        *self.last_flush_time.write() = std::time::Instant::now();
    }

    pub fn mark_checkpointed(&self, lsn: Lsn) {
        *self.last_checkpoint_lsn.write() = lsn;
        *self.last_checkpoint_time.write() = std::time::Instant::now();
        *self.last_flush_lsn.write() = lsn;
        *self.last_flush_time.write() = std::time::Instant::now();
    }
}

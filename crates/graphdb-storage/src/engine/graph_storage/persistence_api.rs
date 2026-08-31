use std::sync::Arc;

use graphdb_core::types::{CommitLsn, CompactConfig};
use graphdb_core::StorageResult;

use crate::StoragePersistenceOps;

use super::persistence;
use super::GraphStorage;

impl StoragePersistenceOps for GraphStorage {
    fn flush(&self) -> StorageResult<()> {
        persistence::flush(&self.ctx)
    }

    fn create_checkpoint(&self) -> StorageResult<Option<crate::CheckpointStats>> {
        persistence::create_checkpoint(&self.ctx)
    }

    fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool> {
        persistence::verify_snapshot(&self.ctx, snapshot_id)
    }

    fn cleanup_snapshots(&self) -> StorageResult<usize> {
        persistence::cleanup_snapshots(&self.ctx)
    }

    fn snapshot_stats(&self) -> crate::SnapshotStats {
        persistence::snapshot_stats(&self.ctx)
    }

    fn persistence_diagnostics(&self) -> Option<crate::PersistenceDiagnostics> {
        persistence::persistence_diagnostics(&self.ctx)
    }

    fn compact(&self, config: &CompactConfig) -> StorageResult<()> {
        persistence::compact_transactional(&self.ctx, config)
    }

    fn save_data(&self) -> StorageResult<()> {
        persistence::save_data(&self.ctx)
    }

    fn save_data_to_dir(&self, dir: &std::path::Path) -> StorageResult<()> {
        persistence::save_data_to_dir(&self.ctx, dir)
    }

    fn auto_flush_if_needed(&self) -> StorageResult<bool> {
        persistence::auto_flush_if_needed(&self.ctx)
    }

    fn auto_checkpoint_if_needed(&self) -> StorageResult<Option<crate::CheckpointStats>> {
        persistence::auto_checkpoint_if_needed(&self.ctx)
    }

    fn should_flush(&self) -> bool {
        persistence::should_flush(&self.ctx)
    }

    fn should_checkpoint(&self) -> bool {
        persistence::should_checkpoint(&self.ctx)
    }

    fn set_outbox_materialized_lsn_provider(
        &self,
        provider: Arc<dyn Fn() -> StorageResult<Option<CommitLsn>> + Send + Sync>,
    ) {
        if let Some(persistence) = self.ctx.persistence() {
            persistence
                .read()
                .set_outbox_materialized_lsn_provider(provider);
        }
    }
}

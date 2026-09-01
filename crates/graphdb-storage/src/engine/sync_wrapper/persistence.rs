use super::SyncWrapper;
use crate::macros::forward_methods;
use crate::{StorageClient, StoragePersistenceOps};
use graphdb_core::StorageError;

impl<S: StorageClient + 'static> StoragePersistenceOps for SyncWrapper<S> {
    forward_methods!(inner;
        fn flush(&self) -> Result<(), StorageError>;
        fn save_data(&self) -> graphdb_core::StorageResult<()>;
        fn save_data_to_dir(&self, dir: &std::path::Path) -> graphdb_core::StorageResult<()>;
        fn verify_snapshot(&self, snapshot_id: u64) -> graphdb_core::StorageResult<bool>;
        fn cleanup_snapshots(&self) -> graphdb_core::StorageResult<usize>;
        fn snapshot_stats(&self) -> crate::SnapshotStats;
        fn persistence_diagnostics(&self) -> Option<crate::PersistenceDiagnostics>;
        fn compact(&self, config: &graphdb_core::types::CompactConfig) -> graphdb_core::StorageResult<()>;
        fn auto_flush_if_needed(&self) -> graphdb_core::StorageResult<bool>;
        fn should_flush(&self) -> bool;
        fn should_checkpoint(&self) -> bool;
    );

    fn create_checkpoint(&self) -> graphdb_core::StorageResult<Option<crate::CheckpointStats>> {
        if self.enabled {
            let manager = self.sync_manager.as_ref().ok_or_else(|| {
                StorageError::invalid_operation(
                    "Synchronization is enabled without an outbox manager",
                )
            })?;
            manager
                .create_checkpoint_outbox_snapshot()
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to create checkpoint outbox snapshot: {error}"
                    ))
                })?;
        }
        self.inner.create_checkpoint()
    }

    fn auto_checkpoint_if_needed(
        &self,
    ) -> graphdb_core::StorageResult<Option<crate::CheckpointStats>> {
        if !self.should_checkpoint() {
            return Ok(None);
        }
        if self.enabled {
            let manager = self.sync_manager.as_ref().ok_or_else(|| {
                StorageError::invalid_operation(
                    "Synchronization is enabled without an outbox manager",
                )
            })?;
            manager
                .create_checkpoint_outbox_snapshot()
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to create checkpoint outbox snapshot: {error}"
                    ))
                })?;
        }
        self.inner.auto_checkpoint_if_needed()
    }
}

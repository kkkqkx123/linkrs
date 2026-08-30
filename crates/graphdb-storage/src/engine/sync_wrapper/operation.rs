use super::SyncWrapper;
use crate::macros::forward_methods;
use crate::{
    StorageClient, StorageCommitOps, StorageGcOps, StorageOperationContext,
    StorageOperationContextOps, StorageRecoveryOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use graphdb_core::metadata::{IndexMetadataManager, SchemaManager};
use graphdb_core::StorageResult;
use std::sync::Arc;

impl<S: StorageClient + 'static> crate::stats_reader::ColumnStatsReader for SyncWrapper<S> {
    fn vertex_column_stats(
        &self,
        space: &str,
        tag: &str,
        column: &str,
    ) -> Option<std::sync::Arc<crate::stats_reader::ColumnStatsSnapshot>> {
        self.inner.vertex_column_stats(space, tag, column)
    }

    fn edge_column_stats(
        &self,
        space: &str,
        edge_type: &str,
        column: &str,
    ) -> Option<std::sync::Arc<crate::stats_reader::ColumnStatsSnapshot>> {
        self.inner.edge_column_stats(space, edge_type, column)
    }

    fn stats_epoch(&self) -> u64 {
        self.inner.stats_epoch()
    }
}

impl<S: StorageClient + StorageSchemaContextOps + 'static> StorageSchemaContextOps
    for SyncWrapper<S>
{
    forward_methods!(inner;
        fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
        fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>>;
    );
}

impl<S: StorageClient + 'static> StorageOperationContextOps for SyncWrapper<S> {
    fn bind_auto_commit_context(&self) -> StorageResult<Self> {
        let inner = self.inner.bind_auto_commit_context()?;
        let inner = match inner.operation_context() {
            Some(context) => inner.bind_operation_context((*context).clone()),
            None => inner,
        };
        Ok(Self {
            inner,
            sync_manager: self.sync_manager.clone(),
            enabled: self.enabled,
            auto_commit_owner: true,
        })
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self {
        Self {
            inner: self.inner.bind_operation_context(context),
            sync_manager: self.sync_manager.clone(),
            enabled: self.enabled,
            auto_commit_owner: false,
        }
    }

    fn bind_read_operation_context(&self) -> StorageResult<Self> {
        let inner = self.inner.bind_read_operation_context()?;
        let inner = match inner.operation_context() {
            Some(context) => inner.bind_operation_context((*context).clone()),
            None => inner,
        };
        Ok(Self {
            inner,
            sync_manager: self.sync_manager.clone(),
            enabled: self.enabled,
            auto_commit_owner: true,
        })
    }

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.inner.operation_context()
    }

    fn finalize_operation(&self, committed: bool) -> StorageResult<()> {
        self.inner.finalize_operation(committed)
    }
}

impl<S: StorageClient + 'static> StorageCommitOps for SyncWrapper<S> {
    fn commit_staged_writes(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> StorageResult<graphdb_core::types::CommitLsn> {
        self.inner.commit_staged_writes(transaction_id, intents)
    }

    fn abort_staged_writes(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
    ) -> StorageResult<()> {
        self.inner.abort_staged_writes(transaction_id)
    }

    fn commit_staged_writes_with_durability(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
        durability: graphdb_core::types::DurabilityLevel,
    ) -> StorageResult<graphdb_core::types::CommitLsn> {
        self.inner
            .commit_staged_writes_with_durability(transaction_id, intents, durability)
    }

    fn recover_outbox_projection(
        &self,
        sync_manager: &graphdb_sync::SyncManager,
    ) -> StorageResult<usize> {
        self.inner.recover_outbox_projection(sync_manager)
    }
}

impl<S: StorageClient + 'static> StorageSyncContextOps for SyncWrapper<S> {
    fn get_sync_manager(&self) -> Option<Arc<graphdb_sync::SyncManager>> {
        self.sync_manager.clone()
    }
}

impl<S: StorageClient + 'static> StorageRecoveryOps for SyncWrapper<S> {
    forward_methods!(inner;
        fn needs_recovery(&self) -> bool;
        fn recover_from_wal(&self) -> graphdb_core::StorageResult<graphdb_transaction::wal::recovery::RecoveryStats>;
        fn recover_from_wal_with_config(
            &self,
            config: graphdb_transaction::wal::recovery::RecoveryConfig,
        ) -> graphdb_core::StorageResult<graphdb_transaction::wal::recovery::RecoveryStats>;
        fn init_with_recovery(&self) -> graphdb_core::StorageResult<Option<graphdb_transaction::wal::recovery::RecoveryStats>>;
    );
}

impl<S: StorageClient + 'static> StorageGcOps for SyncWrapper<S> {
    forward_methods!(inner;
        fn is_index_gc_running(&self) -> bool;
        fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle>;
    );

    forward_methods!(inner;
        fn stop_index_gc(&self);
    );
}

impl<S: crate::client::StorageClient + crate::client::StorageSnapshotOps + 'static>
    crate::client::StorageSnapshotOps for SyncWrapper<S>
{
    forward_methods!(inner;
        fn export_snapshot(&self, ts: graphdb_core::types::Timestamp) -> graphdb_core::StorageResult<Vec<crate::engine::graph_storage::context::ExportedEdgeSnapshotRecord>>;
        fn get_freeze_stats(&self) -> Option<crate::engine::background_freeze::FreezeStats>;
    );

    forward_methods!(inner;
        fn trigger_background_freeze(&self) -> graphdb_core::StorageResult<()>;
    );

    forward_methods!(inner;
        fn list_cold_snapshots(&self) -> graphdb_core::StorageResult<Vec<crate::client::ColdSnapshotInfo>>;
        fn load_cold_snapshot(&self, path: &std::path::Path) -> graphdb_core::StorageResult<crate::client::ColdSnapshotInfo>;
        fn remove_cold_snapshot(&self, label: graphdb_core::types::LabelId) -> graphdb_core::StorageResult<()>;
        fn export_cold_snapshot(&self, label: graphdb_core::types::LabelId, path: &std::path::Path) -> graphdb_core::StorageResult<crate::client::ColdSnapshotInfo>;
        fn merge_cold_snapshots(&self, labels: &[graphdb_core::types::LabelId]) -> graphdb_core::StorageResult<Vec<crate::client::ColdSnapshotInfo>>;
        fn cold_snapshot_dir(&self) -> Option<std::path::PathBuf>;
    );
}

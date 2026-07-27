use std::sync::Arc;

use crate::core::metadata::{IndexMetadataManager, SchemaManager};
use crate::core::types::{
    EdgeTypeInfo, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo, PropertyDef,
    SpaceInfo, TagInfo, UpdateInfo, UserAlterInfo, UserInfo, VertexId,
};
use crate::core::{Edge, EdgeDirection, RoleType, StorageError, StorageResult, Value, Vertex};
use crate::storage::{
    StorageAdmin, StorageAuthOps, StorageClient, StorageCommitOps, StorageGcOps,
    StorageOperationContext, StorageOperationContextOps, StoragePersistenceOps, StorageReader,
    StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps, StorageSnapshotOps,
    StorageStats, StorageSyncContextOps, StorageWriter,
};
use crate::storage::macros::forward_methods;
use crate::sync::SyncManager;

pub struct MetricsStorage<S: StorageClient> {
    inner: S,
}

impl<S: StorageClient> MetricsStorage<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: StorageClient> StorageReader for MetricsStorage<S> {
    forward_methods!(inner;
        fn get_vertex(&self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError>;
        fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError>;
        fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError>;
        fn scan_vertices_by_prop(&self, space: &str, tag: &str, prop: &str, value: &Value) -> Result<Vec<Vertex>, StorageError>;
        fn get_edge(&self, space: &str, src: &VertexId, dst: &VertexId, edge_type: &str, rank: i64) -> Result<Option<Edge>, StorageError>;
        fn get_node_edges(&self, space: &str, node_id: &VertexId, direction: EdgeDirection) -> Result<Vec<Edge>, StorageError>;
        fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError>;
        fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
        fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError>;
        fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError>;
        fn lookup_index(&self, space: &str, index: &str, value: &Value) -> Result<Vec<Value>, StorageError>;
        fn get_vertex_with_schema(&self, space: &str, tag: &str, id: &Value) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError>;
        fn get_edge_with_schema(&self, space: &str, edge_type: &str, src: &Value, dst: &Value) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError>;
        fn scan_vertices_with_schema(&self, space: &str, tag: &str) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError>;
        fn scan_edges_with_schema(&self, space: &str, edge_type: &str) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError>;
        fn get_space(&self, space: &str) -> Result<Option<SpaceInfo>, StorageError>;
        fn get_space_by_id(&self, space_id: u64) -> Result<Option<SpaceInfo>, StorageError>;
        fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError>;
        fn get_space_id(&self, space: &str) -> Result<u64, StorageError>;
        fn space_exists(&self, space: &str) -> bool;
        fn get_tag(&self, space: &str, tag: &str) -> Result<Option<TagInfo>, StorageError>;
        fn list_tags(&self, space: &str) -> Result<Vec<TagInfo>, StorageError>;
        fn get_edge_type(&self, space: &str, edge_type: &str) -> Result<Option<EdgeTypeInfo>, StorageError>;
        fn list_edge_types(&self, space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError>;
        fn get_tag_index(&self, space: &str, index: &str) -> Result<Option<Index>, StorageError>;
        fn list_tag_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError>;
        fn get_edge_index(&self, space: &str, index: &str) -> Result<Option<Index>, StorageError>;
        fn list_edge_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError>;
        fn get_vertex_version_history(&self, space: &str, tag: &str) -> Result<Option<crate::storage::LabelVersionHistory>, StorageError>;
        fn get_edge_version_history(&self, space: &str, edge_type: &str) -> Result<Option<crate::storage::LabelVersionHistory>, StorageError>;
        fn get_vertex_schema_changes(&self, space: &str, tag: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::storage::PropertyChange>, StorageError>;
        fn get_edge_schema_changes(&self, space: &str, edge_type: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::storage::PropertyChange>, StorageError>;
        fn detect_vertex_breaking_changes(&self, space: &str, tag: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::storage::PropertyChange>, StorageError>;
        fn detect_edge_breaking_changes(&self, space: &str, edge_type: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::storage::PropertyChange>, StorageError>;
    );
}

impl<S: StorageClient> StorageWriter for MetricsStorage<S> {
    forward_methods!(inner;
        fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError>;
        fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError>;
        fn delete_vertex_with_edges(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError>;
        fn batch_insert_vertices(&mut self, space: &str, vertices: Vec<Vertex>) -> Result<Vec<VertexId>, StorageError>;
        fn delete_tags(&mut self, space: &str, vertex_id: &VertexId, tag_names: &[String]) -> Result<usize, StorageError>;
        fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
        fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
        fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError>;
        fn insert_vertex_data(&mut self, space: &str, info: &InsertVertexInfo) -> Result<bool, StorageError>;
        fn insert_edge_data(&mut self, space: &str, info: &InsertEdgeInfo) -> Result<bool, StorageError>;
        fn delete_vertex_data(&mut self, space: &str, vertex_id: &str) -> Result<bool, StorageError>;
        fn delete_edge_data(&mut self, space: &str, src: &str, dst: &str, rank: i64) -> Result<bool, StorageError>;
        fn update_data(&mut self, space: &str, space_id: u64, info: &UpdateInfo) -> Result<bool, StorageError>;
    );

    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        StorageWriter::delete_vertex(&mut self.inner, space, id)
    }

    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError> {
        StorageWriter::delete_edge(&mut self.inner, space, src, dst, edge_type, rank)
    }
}

impl<S: StorageClient> StorageSchemaOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn create_space(&mut self, space: &mut SpaceInfo) -> Result<bool, StorageError>;
        fn drop_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn clear_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn alter_space_comment(&mut self, space_id: u64, comment: String) -> Result<bool, StorageError>;
        fn create_tag(&mut self, space: &str, tag: &TagInfo) -> Result<u32, StorageError>;
        fn alter_tag(&mut self, space: &str, tag: &str, additions: Vec<PropertyDef>, deletions: Vec<String>) -> Result<bool, StorageError>;
        fn rename_vertex_property(&mut self, label: LabelId, old_name: &str, new_name: &str) -> Result<(), StorageError>;
        fn rename_tag_property(&mut self, space: &str, tag: &str, old_name: &str, new_name: &str) -> Result<bool, StorageError>;
        fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError>;
        fn create_edge_type(&mut self, space: &str, edge: &EdgeTypeInfo) -> Result<u32, StorageError>;
        fn alter_edge_type(&mut self, space: &str, edge_type: &str, additions: Vec<PropertyDef>, deletions: Vec<String>) -> Result<bool, StorageError>;
        fn drop_edge_type(&mut self, space: &str, edge_type: &str) -> Result<bool, StorageError>;
        fn create_tag_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
        fn drop_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn rebuild_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn create_edge_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
        fn drop_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn rebuild_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    );
}

impl<S: StorageClient> StorageAuthOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn change_password(&mut self, info: &PasswordInfo) -> Result<bool, StorageError>;
        fn create_user(&mut self, info: &UserInfo) -> Result<bool, StorageError>;
        fn alter_user(&mut self, info: &UserAlterInfo) -> Result<bool, StorageError>;
        fn drop_user(&mut self, username: &str) -> Result<bool, StorageError>;
        fn grant_role(&mut self, username: &str, space_id: u64, role: RoleType) -> Result<bool, StorageError>;
        fn revoke_role(&mut self, username: &str, space_id: u64) -> Result<bool, StorageError>;
    );

    forward_methods!(inner;
        fn user_exists(&self, username: &str) -> bool;
    );
}

impl<S: StorageClient> StorageAdmin for MetricsStorage<S> {
    forward_methods!(inner;
        fn load_from_disk(&mut self) -> Result<(), StorageError>;
        fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError>;
    );

    forward_methods!(inner;
        fn save_to_disk(&self) -> Result<(), StorageError>;
        fn get_storage_stats(&self) -> StorageStats;
        fn get_db_path(&self) -> &str;
        fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
    );
}

impl<S: StorageClient> StoragePersistenceOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn flush(&self) -> Result<(), StorageError>;
        fn save_data(&self) -> crate::core::StorageResult<()>;
        fn save_data_to_dir(&self, dir: &std::path::Path) -> crate::core::StorageResult<()>;
        fn create_checkpoint(&self) -> crate::core::StorageResult<Option<crate::storage::CheckpointStats>>;
        fn verify_snapshot(&self, snapshot_id: u64) -> crate::core::StorageResult<bool>;
        fn cleanup_snapshots(&self) -> crate::core::StorageResult<usize>;
        fn snapshot_stats(&self) -> crate::storage::SnapshotStats;
        fn persistence_diagnostics(&self) -> Option<crate::storage::PersistenceDiagnostics>;
        fn compact(&self, config: &crate::core::types::CompactConfig) -> crate::core::StorageResult<()>;
        fn auto_flush_if_needed(&self) -> crate::core::StorageResult<bool>;
        fn auto_checkpoint_if_needed(&self) -> crate::core::StorageResult<Option<crate::storage::CheckpointStats>>;
        fn should_flush(&self) -> bool;
        fn should_checkpoint(&self) -> bool;
    );
}

impl<S: StorageClient + StorageSchemaContextOps> StorageSchemaContextOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
        fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>>;
    );
}

impl<S: StorageClient> StorageOperationContextOps for MetricsStorage<S> {
    fn bind_auto_commit_context(&self) -> StorageResult<Self> {
        Ok(Self {
            inner: self.inner.bind_auto_commit_context()?,
        })
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self {
        Self {
            inner: self.inner.bind_operation_context(context),
        }
    }

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.inner.operation_context()
    }

    fn finalize_operation(&self, committed: bool) -> crate::core::StorageResult<()> {
        self.inner.finalize_operation(committed)
    }
}

impl<S: StorageClient> StorageCommitOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn commit_staged_writes(&self, transaction_id: crate::core::types::TransactionId, intents: &[crate::core::wal::OutboxIntent]) -> crate::core::StorageResult<crate::core::types::CommitLsn>;
        fn abort_staged_writes(&self, transaction_id: crate::core::types::TransactionId) -> crate::core::StorageResult<()>;
        fn recover_outbox_projection(&self, sync_manager: &crate::sync::SyncManager) -> crate::core::StorageResult<usize>;
    );
}

impl<S: StorageClient + StorageSyncContextOps> StorageSyncContextOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn get_sync_manager(&self) -> Option<Arc<SyncManager>>;
    );
}

impl<S: StorageClient> StorageRecoveryOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn needs_recovery(&self) -> bool;
        fn recover_from_wal(&self) -> crate::core::StorageResult<crate::transaction::wal::recovery::RecoveryStats>;
        fn recover_from_wal_with_config(
            &self,
            config: crate::transaction::wal::recovery::RecoveryConfig,
        ) -> crate::core::StorageResult<crate::transaction::wal::recovery::RecoveryStats>;
        fn init_with_recovery(&self) -> crate::core::StorageResult<Option<crate::transaction::wal::recovery::RecoveryStats>>;
    );
}

impl<S: StorageClient> StorageGcOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn is_index_gc_running(&self) -> bool;
        fn start_index_gc(&self) -> Option<std::thread::JoinHandle<()>>;
    );

    forward_methods!(inner;
        fn stop_index_gc(&self);
    );
}

impl<S: StorageClient> std::fmt::Debug for MetricsStorage<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsStorage")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<S: StorageClient> Clone for MetricsStorage<S>
where
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S: crate::storage::client::StorageClient + StorageSnapshotOps + 'static>
    crate::storage::client::StorageSnapshotOps for MetricsStorage<S>
{
    fn export_snapshot(
        &self,
        ts: crate::core::types::Timestamp,
    ) -> crate::core::StorageResult<
        Vec<crate::storage::engine::graph_storage::context::ExportedEdgeSnapshotRecord>,
    > {
        self.inner.export_snapshot(ts)
    }

    fn get_freeze_stats(&self) -> Option<crate::storage::engine::background_freeze::FreezeStats> {
        self.inner.get_freeze_stats()
    }

    fn trigger_background_freeze(&self) -> crate::core::StorageResult<()> {
        self.inner.trigger_background_freeze()
    }
}

impl<S: crate::transaction::UndoTarget + StorageClient> crate::transaction::UndoTarget
    for MetricsStorage<S>
{
    fn delete_vertex_type(
        &self,
        label: crate::core::types::LabelId,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::delete_vertex_type(&self.inner, label)
    }

    fn delete_edge_type(
        &self,
        edge_key: crate::core::types::EdgeKey,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::delete_edge_type(&self.inner, edge_key)
    }

    fn delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::delete_vertex(&self.inner, vertex, ts)
    }

    fn delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::delete_edge(&self.inner, edge_ctx)
    }

    fn restore_edge(
        &self,
        edge: crate::core::types::EdgeIdentifier,
        properties: Vec<(String, crate::core::Value)>,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::restore_edge(&self.inner, edge, properties, ts)
    }

    fn undo_update_vertex_property(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        col_id: crate::core::types::ColumnId,
        value: crate::transaction::undo_log::PropertyValue,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::undo_update_vertex_property(
            &self.inner,
            vertex,
            col_id,
            value,
            ts,
        )
    }

    fn undo_update_edge_property(
        &self,
        edge_id: crate::core::types::EdgeIdentifier,
        oe_offset: i32,
        ie_offset: i32,
        col_id: crate::core::types::ColumnId,
        value: crate::transaction::undo_log::PropertyValue,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::undo_update_edge_property(
            &self.inner,
            edge_id,
            oe_offset,
            ie_offset,
            col_id,
            value,
            ts,
        )
    }

    fn revert_delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_delete_vertex(&self.inner, vertex, ts)
    }

    fn revert_delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_delete_edge(&self.inner, edge_ctx)
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_delete_vertex_properties(
            &self.inner,
            label_name,
            prop_names,
        )
    }

    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_delete_edge_properties(
            &self.inner,
            src_label,
            dst_label,
            edge_label,
            prop_names,
        )
    }

    fn revert_delete_vertex_label(
        &self,
        label_name: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_delete_vertex_label(&self.inner, label_name)
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_delete_edge_label(
            &self.inner,
            src_label,
            dst_label,
            edge_label,
        )
    }

    fn revert_rename_vertex_properties(
        &self,
        label_name: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_rename_vertex_properties(
            &self.inner,
            label_name,
            current_names,
            original_names,
        )
    }

    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::transaction::UndoTarget::revert_rename_edge_properties(
            &self.inner,
            src_label,
            dst_label,
            edge_label,
            current_names,
            original_names,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::{GraphStorage, MetricsStorage, StoragePersistenceOps};

    #[test]
    fn delegates_admin_checkpoint_operations() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let inner = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
            .expect("Failed to create GraphStorage");
        let storage = MetricsStorage::new(inner);

        let checkpoint = storage
            .create_checkpoint()
            .expect("checkpoint should succeed");

        assert!(checkpoint.is_some());
    }
}

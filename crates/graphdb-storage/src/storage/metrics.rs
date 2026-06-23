use std::sync::Arc;
use std::time::Instant;

use crate::core::metadata::SchemaManager;
use crate::core::stats::StatsManager;
use crate::core::types::{
    EdgeTypeInfo, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo, PropertyDef,
    SpaceInfo, TagInfo, TransactionContextInfo, UpdateInfo, UserAlterInfo, UserInfo, VertexId,
};
use crate::core::{Edge, EdgeDirection, RoleType, StorageError, Value, Vertex};
use crate::storage::{
    StorageAdmin, StorageAuthOps, StorageClient, StorageGcOps, StoragePersistenceOps,
    StorageReader, StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps, StorageSnapshotOps,
    StorageStats, StorageSyncContextOps, StorageTransactionContextOps, StorageWriter,
};
use crate::sync::SyncManager;

macro_rules! wrap_read {
    ($fn:ident($self:ident $(, $arg:ident: $ty:ty)*) -> $ret:ty) => {
        fn $fn(&$self, $($arg: $ty),*) -> $ret {
            let start = Instant::now();
            let result = $self.inner.$fn($($arg),*);
            $self.record_read(start.elapsed().as_micros() as u64, result.is_ok());
            result
        }
    };
}

macro_rules! wrap_write {
    ($fn:ident($self:ident $(, $arg:ident: $ty:ty)*) -> $ret:ty) => {
        fn $fn(&mut $self, $($arg: $ty),*) -> $ret {
            let start = Instant::now();
            let result = $self.inner.$fn($($arg),*);
            $self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
            result
        }
    };
}

macro_rules! forward_methods {
    ($field:ident; $(fn $fn:ident(&self $(, $arg:ident : $ty:ty)* $(,)?);)+) => {
        $(
            fn $fn(&self, $($arg: $ty),*) {
                self.$field.$fn($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $fn:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?);)+) => {
        $(
            fn $fn(&mut self, $($arg: $ty),*) {
                self.$field.$fn($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $fn:ident(&self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $fn(&self, $($arg: $ty),*) -> $ret {
                self.$field.$fn($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $fn:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $fn(&mut self, $($arg: $ty),*) -> $ret {
                self.$field.$fn($($arg),*)
            }
        )+
    };
}

pub struct MetricsStorage<S: StorageClient> {
    inner: S,
    stats_manager: Arc<StatsManager>,
}

impl<S: StorageClient> MetricsStorage<S> {
    pub fn new(inner: S, stats_manager: Arc<StatsManager>) -> Self {
        Self {
            inner,
            stats_manager,
        }
    }

    pub fn into_inner(self) -> S {
        self.inner
    }

    pub fn stats_manager(&self) -> &Arc<StatsManager> {
        &self.stats_manager
    }

    fn record_read(&self, latency_us: u64, success: bool) {
        self.stats_manager.record_storage_read(latency_us, success);
    }

    fn record_write(&self, latency_us: u64, success: bool) {
        self.stats_manager.record_storage_write(latency_us, success);
    }
}

impl<S: StorageClient> StorageReader for MetricsStorage<S> {
    wrap_read!(get_vertex(self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError>);
    wrap_read!(scan_vertices(self, space: &str) -> Result<Vec<Vertex>, StorageError>);
    wrap_read!(scan_vertices_by_tag(self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError>);
    wrap_read!(scan_vertices_by_prop(self, space: &str, tag: &str, prop: &str, value: &Value) -> Result<Vec<Vertex>, StorageError>);
    wrap_read!(get_edge(self, space: &str, src: &VertexId, dst: &VertexId, edge_type: &str, rank: i64) -> Result<Option<Edge>, StorageError>);
    wrap_read!(get_node_edges(self, space: &str, node_id: &VertexId, direction: EdgeDirection) -> Result<Vec<Edge>, StorageError>);
    wrap_read!(scan_edges_by_type(self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError>);
    wrap_read!(scan_all_edges(self, space: &str) -> Result<Vec<Edge>, StorageError>);
    wrap_read!(lookup_index(self, space: &str, index: &str, value: &Value) -> Result<Vec<Value>, StorageError>);
    wrap_read!(get_vertex_with_schema(self, space: &str, tag: &str, id: &Value) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError>);
    wrap_read!(get_edge_with_schema(self, space: &str, edge_type: &str, src: &Value, dst: &Value) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError>);
    wrap_read!(scan_vertices_with_schema(self, space: &str, tag: &str) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError>);
    wrap_read!(scan_edges_with_schema(self, space: &str, edge_type: &str) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError>);
    wrap_read!(get_space(self, space: &str) -> Result<Option<SpaceInfo>, StorageError>);
    wrap_read!(get_space_by_id(self, space_id: u64) -> Result<Option<SpaceInfo>, StorageError>);
    wrap_read!(list_spaces(self) -> Result<Vec<SpaceInfo>, StorageError>);
    wrap_read!(get_space_id(self, space: &str) -> Result<u64, StorageError>);

    fn space_exists(&self, space: &str) -> bool {
        self.inner.space_exists(space)
    }

    wrap_read!(get_tag(self, space: &str, tag: &str) -> Result<Option<TagInfo>, StorageError>);
    wrap_read!(list_tags(self, space: &str) -> Result<Vec<TagInfo>, StorageError>);
    wrap_read!(get_edge_type(self, space: &str, edge_type: &str) -> Result<Option<EdgeTypeInfo>, StorageError>);
    wrap_read!(list_edge_types(self, space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError>);
    wrap_read!(get_tag_index(self, space: &str, index: &str) -> Result<Option<Index>, StorageError>);
    wrap_read!(list_tag_indexes(self, space: &str) -> Result<Vec<Index>, StorageError>);
}

impl<S: StorageClient> StorageWriter for MetricsStorage<S> {
    wrap_write!(insert_vertex(self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError>);
    wrap_write!(update_vertex(self, space: &str, vertex: Vertex) -> Result<(), StorageError>);
    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        let start = Instant::now();
        let result = StorageWriter::delete_vertex(&mut self.inner, space, id);
        self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }
    wrap_write!(delete_vertex_with_edges(self, space: &str, id: &VertexId) -> Result<(), StorageError>);
    wrap_write!(batch_insert_vertices(self, space: &str, vertices: Vec<Vertex>) -> Result<Vec<VertexId>, StorageError>);
    wrap_write!(delete_tags(self, space: &str, vertex_id: &VertexId, tag_names: &[String]) -> Result<usize, StorageError>);
    wrap_write!(insert_edge(self, space: &str, edge: Edge) -> Result<(), StorageError>);
    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError> {
        let start = Instant::now();
        let result = StorageWriter::delete_edge(&mut self.inner, space, src, dst, edge_type, rank);
        self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }
    wrap_write!(batch_insert_edges(self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError>);
    wrap_write!(insert_vertex_data(self, space: &str, info: &InsertVertexInfo) -> Result<bool, StorageError>);
    wrap_write!(insert_edge_data(self, space: &str, info: &InsertEdgeInfo) -> Result<bool, StorageError>);
    wrap_write!(delete_vertex_data(self, space: &str, vertex_id: &str) -> Result<bool, StorageError>);
    wrap_write!(delete_edge_data(self, space: &str, src: &str, dst: &str, rank: i64) -> Result<bool, StorageError>);
    wrap_write!(update_data(self, space: &str, space_id: u64, info: &UpdateInfo) -> Result<bool, StorageError>);
}

impl<S: StorageClient> StorageSchemaOps for MetricsStorage<S> {
    wrap_write!(create_space(self, space: &mut SpaceInfo) -> Result<bool, StorageError>);
    wrap_write!(drop_space(self, space: &str) -> Result<bool, StorageError>);
    wrap_write!(clear_space(self, space: &str) -> Result<bool, StorageError>);
    wrap_write!(alter_space_comment(self, space_id: u64, comment: String) -> Result<bool, StorageError>);
    wrap_write!(create_tag(self, space: &str, tag: &TagInfo) -> Result<u32, StorageError>);
    wrap_write!(alter_tag(self, space: &str, tag: &str, additions: Vec<PropertyDef>, deletions: Vec<String>) -> Result<bool, StorageError>);
    wrap_write!(rename_vertex_property(self, label: LabelId, old_name: &str, new_name: &str) -> Result<(), StorageError>);
    wrap_write!(rename_tag_property(self, space: &str, tag: &str, old_name: &str, new_name: &str) -> Result<bool, StorageError>);
    wrap_write!(drop_tag(self, space: &str, tag: &str) -> Result<bool, StorageError>);
    wrap_write!(create_edge_type(self, space: &str, edge: &EdgeTypeInfo) -> Result<u32, StorageError>);
    wrap_write!(alter_edge_type(self, space: &str, edge_type: &str, additions: Vec<PropertyDef>, deletions: Vec<String>) -> Result<bool, StorageError>);
    wrap_write!(drop_edge_type(self, space: &str, edge_type: &str) -> Result<bool, StorageError>);
    wrap_write!(create_tag_index(self, space: &str, info: &Index) -> Result<bool, StorageError>);
    wrap_write!(drop_tag_index(self, space: &str, index: &str) -> Result<bool, StorageError>);
    wrap_write!(rebuild_tag_index(self, space: &str, index: &str) -> Result<bool, StorageError>);
}

impl<S: StorageClient> StorageAuthOps for MetricsStorage<S> {
    wrap_write!(change_password(self, info: &PasswordInfo) -> Result<bool, StorageError>);
    wrap_write!(create_user(self, info: &UserInfo) -> Result<bool, StorageError>);
    wrap_write!(alter_user(self, info: &UserAlterInfo) -> Result<bool, StorageError>);
    wrap_write!(drop_user(self, username: &str) -> Result<bool, StorageError>);
    fn user_exists(&self, username: &str) -> bool {
        let start = Instant::now();
        let result = self.inner.user_exists(username);
        self.record_read(start.elapsed().as_micros() as u64, true);
        result
    }
    wrap_write!(grant_role(self, username: &str, space_id: u64, role: RoleType) -> Result<bool, StorageError>);
    wrap_write!(revoke_role(self, username: &str, space_id: u64) -> Result<bool, StorageError>);
}

impl<S: StorageClient> StorageAdmin for MetricsStorage<S> {
    fn load_from_disk(&mut self) -> Result<(), StorageError> {
        let start = Instant::now();
        let result = self.inner.load_from_disk();
        self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }

    fn save_to_disk(&self) -> Result<(), StorageError> {
        let start = Instant::now();
        let result = self.inner.save_to_disk();
        self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }

    fn get_storage_stats(&self) -> StorageStats {
        self.inner.get_storage_stats()
    }

    fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError> {
        let start = Instant::now();
        let result = self.inner.find_dangling_edges(space);
        self.record_read(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }

    fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError> {
        let start = Instant::now();
        let result = self.inner.repair_dangling_edges(space);
        self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }

    fn get_db_path(&self) -> &str {
        self.inner.get_db_path()
    }
}

impl<S: StorageClient> StoragePersistenceOps for MetricsStorage<S> {
    fn flush(&self) -> Result<(), StorageError> {
        let start = Instant::now();
        let result = self.inner.flush();
        self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }

    fn save_data(&self) -> crate::core::StorageResult<()> {
        let start = Instant::now();
        let result = self.inner.save_data();
        self.record_write(start.elapsed().as_micros() as u64, result.is_ok());
        result
    }

    fn save_data_to_dir(&self, dir: &std::path::Path) -> crate::core::StorageResult<()> {
        self.inner.save_data_to_dir(dir)
    }

    fn create_checkpoint(
        &self,
    ) -> crate::core::StorageResult<Option<crate::storage::CheckpointStats>> {
        self.inner.create_checkpoint()
    }

    fn verify_snapshot(&self, snapshot_id: u64) -> crate::core::StorageResult<bool> {
        self.inner.verify_snapshot(snapshot_id)
    }

    fn cleanup_snapshots(&self) -> crate::core::StorageResult<usize> {
        self.inner.cleanup_snapshots()
    }

    fn snapshot_stats(&self) -> crate::storage::SnapshotStats {
        self.inner.snapshot_stats()
    }

    fn compact(&self, config: &crate::core::types::CompactConfig) -> crate::core::StorageResult<()> {
        self.inner.compact(config)
    }

    fn auto_flush_if_needed(&self) -> crate::core::StorageResult<bool> {
        self.inner.auto_flush_if_needed()
    }

    fn auto_checkpoint_if_needed(
        &self,
    ) -> crate::core::StorageResult<Option<crate::storage::CheckpointStats>> {
        self.inner.auto_checkpoint_if_needed()
    }

    fn should_flush(&self) -> bool {
        self.inner.should_flush()
    }

    fn should_checkpoint(&self) -> bool {
        self.inner.should_checkpoint()
    }
}

impl<S: StorageClient + StorageSchemaContextOps> StorageSchemaContextOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
    );
}

impl<S: StorageClient + StorageTransactionContextOps> StorageTransactionContextOps
    for MetricsStorage<S>
{
    forward_methods!(inner;
        fn get_transaction_context(&self) -> Option<Arc<TransactionContextInfo>>;
    );

    forward_methods!(inner;
        fn set_transaction_context(&self, ctx: Option<Arc<TransactionContextInfo>>);
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
            stats_manager: self.stats_manager.clone(),
        }
    }
}

impl<S: crate::storage::client::StorageClient + StorageSnapshotOps + 'static> crate::storage::client::StorageSnapshotOps for MetricsStorage<S> {
    fn export_snapshot(&self, ts: crate::core::types::Timestamp) -> crate::core::StorageResult<Vec<crate::storage::engine::graph_storage::context::ExportedEdgeSnapshotRecord>> {
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
    use std::sync::Arc;

    use crate::core::stats::{MetricType, StatsManager};
    use crate::core::types::VertexId;
    use crate::storage::{
        GraphStorage, MetricsStorage, MockStorage, StoragePersistenceOps, StorageReader,
        StorageWriter,
    };

    #[test]
    fn records_read_and_write_metrics() {
        let stats_manager = Arc::new(StatsManager::new());
        let inner = MockStorage::new().expect("Failed to create MockStorage");
        let mut storage = MetricsStorage::new(inner, stats_manager.clone());

        storage
            .get_vertex("test", &VertexId::from_int64(1))
            .expect("Failed to read vertex");
        storage
            .batch_insert_edges("test", Vec::new())
            .expect("Failed to write edges");

        assert_eq!(stats_manager.get_value(MetricType::StorageReadOps), Some(1));
        assert_eq!(
            stats_manager.get_value(MetricType::StorageWriteOps),
            Some(1)
        );
    }

    #[test]
    fn delegates_admin_checkpoint_operations() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let inner = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
            .expect("Failed to create GraphStorage");
        let stats_manager = Arc::new(StatsManager::new());
        let storage = MetricsStorage::new(inner, stats_manager);

        let checkpoint = storage
            .create_checkpoint()
            .expect("checkpoint should succeed");

        assert!(checkpoint.is_some());
    }
}

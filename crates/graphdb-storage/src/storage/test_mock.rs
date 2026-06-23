use crate::core::error::StorageError;
use crate::core::types::{
    EdgeTypeInfo, EdgeTypeSchema, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo,
    PropertyDef, SpaceInfo, TagInfo, TransactionContextInfo, UpdateInfo, UserAlterInfo, UserInfo,
    VertexId,
};
use crate::core::{Edge, EdgeDirection, RoleType, Value, Vertex};
use crate::storage::engine::graph_storage::GraphStorageContext;
use crate::storage::{
    StorageAdmin, StorageAuthOps, StorageGcOps, StoragePersistenceOps, StorageReader,
    StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps, StorageStats,
    StorageSyncContextOps, StorageTransactionContextOps, StorageWriter,
};
use crate::transaction::UndoTarget;
use parking_lot::RwLock;
use std::sync::Arc;

macro_rules! mock_stub {
    (&self, $fn:ident($($arg:ident: $ty:ty),*) -> $ret:ty, $val:expr) => {
        fn $fn(&self, $($arg: $ty),*) -> $ret { $val }
    };
    (&mut self, $fn:ident($($arg:ident: $ty:ty),*) -> $ret:ty, $val:expr) => {
        fn $fn(&mut self, $($arg: $ty),*) -> $ret { $val }
    };
}

#[derive(Debug, Clone)]
pub struct MockStorage {
    graph: GraphStorageContext,
    schema_manager: Arc<crate::core::metadata::SchemaManager>,
    transaction_context: Arc<RwLock<Option<Arc<TransactionContextInfo>>>>,
    fail_insert_edge: Arc<RwLock<bool>>,
    fail_delete_edge: Arc<RwLock<bool>>,
    fail_batch_insert_edges: Arc<RwLock<bool>>,
}

impl MockStorage {
    pub fn new() -> Result<Self, StorageError> {
        Ok(Self {
            graph: GraphStorageContext::new(),
            schema_manager: Arc::new(crate::core::metadata::SchemaManager::new()),
            transaction_context: Arc::new(RwLock::new(None)),
            fail_insert_edge: Arc::new(RwLock::new(false)),
            fail_delete_edge: Arc::new(RwLock::new(false)),
            fail_batch_insert_edges: Arc::new(RwLock::new(false)),
        })
    }

    pub fn get_graph(&self) -> &GraphStorageContext {
        &self.graph
    }

    pub fn set_fail_insert_edge(&self, enabled: bool) {
        *self.fail_insert_edge.write() = enabled;
    }

    pub fn set_fail_delete_edge(&self, enabled: bool) {
        *self.fail_delete_edge.write() = enabled;
    }

    pub fn set_fail_batch_insert_edges(&self, enabled: bool) {
        *self.fail_batch_insert_edges.write() = enabled;
    }
}

impl Default for MockStorage {
    fn default() -> Self {
        Self::new().expect("Failed to create MockStorage")
    }
}

impl StorageReader for MockStorage {
    mock_stub!(&self, get_vertex(_space: &str, _id: &VertexId) -> Result<Option<Vertex>, StorageError>, Ok(None));
    mock_stub!(&self, scan_vertices(_space: &str) -> Result<Vec<Vertex>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_vertices_by_tag(_space: &str, _tag: &str) -> Result<Vec<Vertex>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_vertices_by_prop(_space: &str, _tag: &str, _prop: &str, _value: &Value) -> Result<Vec<Vertex>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_edge(_space: &str, _src: &VertexId, _dst: &VertexId, _edge_type: &str, _rank: i64) -> Result<Option<Edge>, StorageError>, Ok(None));
    mock_stub!(&self, get_node_edges(_space: &str, _node_id: &VertexId, _direction: EdgeDirection) -> Result<Vec<Edge>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_edges_by_type(_space: &str, _edge_type: &str) -> Result<Vec<Edge>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_all_edges(_space: &str) -> Result<Vec<Edge>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, lookup_index(_space: &str, _index: &str, _value: &Value) -> Result<Vec<Value>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_vertex_with_schema(_space: &str, _tag: &str, _id: &Value) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError>, Ok(None));
    mock_stub!(&self, get_edge_with_schema(_space: &str, _edge_type: &str, _src: &Value, _dst: &Value) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError>, Ok(None));
    mock_stub!(&self, scan_vertices_with_schema(_space: &str, _tag: &str) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_edges_with_schema(_space: &str, _edge_type: &str) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_space(_space: &str) -> Result<Option<SpaceInfo>, StorageError>, Ok(None));
    mock_stub!(&self, get_space_by_id(_space_id: u64) -> Result<Option<SpaceInfo>, StorageError>, Ok(None));
    mock_stub!(&self, list_spaces() -> Result<Vec<SpaceInfo>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_space_id(_space: &str) -> Result<u64, StorageError>, Ok(1));
    mock_stub!(&self, space_exists(_space: &str) -> bool, false);
    mock_stub!(&self, get_tag(_space: &str, _tag: &str) -> Result<Option<TagInfo>, StorageError>, Ok(None));
    mock_stub!(&self, list_tags(_space: &str) -> Result<Vec<TagInfo>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_edge_type(_space: &str, _edge_type: &str) -> Result<Option<EdgeTypeSchema>, StorageError>, Ok(None));
    mock_stub!(&self, list_edge_types(_space: &str) -> Result<Vec<EdgeTypeSchema>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_tag_index(_space: &str, _index: &str) -> Result<Option<Index>, StorageError>, Ok(None));
    mock_stub!(&self, list_tag_indexes(_space: &str) -> Result<Vec<Index>, StorageError>, Ok(Vec::new()));
}

impl StorageWriter for MockStorage {
    mock_stub!(&mut self, insert_vertex(_space: &str, _vertex: Vertex) -> Result<VertexId, StorageError>, Ok(VertexId::new()));
    mock_stub!(&mut self, update_vertex(_space: &str, _vertex: Vertex) -> Result<(), StorageError>, Ok(()));
    mock_stub!(&mut self, delete_vertex(_space: &str, _id: &VertexId) -> Result<(), StorageError>, Ok(()));
    mock_stub!(&mut self, delete_vertex_with_edges(_space: &str, _id: &VertexId) -> Result<(), StorageError>, Ok(()));
    mock_stub!(&mut self, batch_insert_vertices(_space: &str, _vertices: Vec<Vertex>) -> Result<Vec<VertexId>, StorageError>, Ok(Vec::new()));
    mock_stub!(&mut self, delete_tags(_space: &str, _vertex_id: &VertexId, _tag_names: &[String]) -> Result<usize, StorageError>, Ok(0));
    fn insert_edge(&mut self, _space: &str, _edge: Edge) -> Result<(), StorageError> {
        if *self.fail_insert_edge.read() {
            Err(StorageError::db_error("insert_edge failed".to_string()))
        } else {
            Ok(())
        }
    }
    fn delete_edge(
        &mut self,
        _space: &str,
        _src: &VertexId,
        _dst: &VertexId,
        _edge_type: &str,
        _rank: i64,
    ) -> Result<(), StorageError> {
        if *self.fail_delete_edge.read() {
            Err(StorageError::db_error("delete_edge failed".to_string()))
        } else {
            Ok(())
        }
    }
    fn batch_insert_edges(&mut self, _space: &str, _edges: Vec<Edge>) -> Result<(), StorageError> {
        if *self.fail_batch_insert_edges.read() {
            Err(StorageError::db_error(
                "batch_insert_edges failed".to_string(),
            ))
        } else {
            Ok(())
        }
    }
    mock_stub!(&mut self, insert_vertex_data(_space: &str, _info: &InsertVertexInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, insert_edge_data(_space: &str, _info: &InsertEdgeInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, delete_vertex_data(_space: &str, _vertex_id: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, delete_edge_data(_space: &str, _src: &str, _dst: &str, _rank: i64) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, update_data(_space: &str, _space_id: u64, _info: &UpdateInfo) -> Result<bool, StorageError>, Ok(true));
}

impl StorageSchemaOps for MockStorage {
    mock_stub!(&mut self, create_space(_space: &mut SpaceInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, drop_space(_space: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, clear_space(_space: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, alter_space_comment(_space_id: u64, _comment: String) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, create_tag(_space: &str, _info: &TagInfo) -> Result<u32, StorageError>, Ok(1));
    mock_stub!(&mut self, alter_tag(_space: &str, _tag: &str, _additions: Vec<PropertyDef>, _deletions: Vec<String>) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, rename_vertex_property(_label: LabelId, _old_name: &str, _new_name: &str) -> Result<(), StorageError>, Ok(()));
    mock_stub!(&mut self, rename_tag_property(_space: &str, _tag: &str, _old_name: &str, _new_name: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, drop_tag(_space: &str, _tag: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, create_edge_type(_space: &str, _info: &EdgeTypeSchema) -> Result<u32, StorageError>, Ok(1));
    mock_stub!(&mut self, alter_edge_type(_space: &str, _edge_type: &str, _additions: Vec<PropertyDef>, _deletions: Vec<String>) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, drop_edge_type(_space: &str, _edge_type: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, create_tag_index(_space: &str, _info: &Index) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, drop_tag_index(_space: &str, _index: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, rebuild_tag_index(_space: &str, _index: &str) -> Result<bool, StorageError>, Ok(true));
}

impl StorageAuthOps for MockStorage {
    mock_stub!(&mut self, change_password(_info: &PasswordInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, create_user(_info: &UserInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, alter_user(_info: &UserAlterInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, drop_user(_username: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&self, user_exists(_username: &str) -> bool, false);
    mock_stub!(&mut self, grant_role(_username: &str, _space_id: u64, _role: RoleType) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, revoke_role(_username: &str, _space_id: u64) -> Result<bool, StorageError>, Ok(true));
}

impl StorageAdmin for MockStorage {
    mock_stub!(&mut self, load_from_disk() -> Result<(), StorageError>, Ok(()));
    mock_stub!(&self, save_to_disk() -> Result<(), StorageError>, Ok(()));

    fn get_storage_stats(&self) -> StorageStats {
        StorageStats {
            total_vertices: 0,
            total_edges: 0,
            total_spaces: 0,
            total_tags: 0,
            total_edge_types: 0,
            total_size_bytes: 0,
            data_size_bytes: 0,
            index_size_bytes: 0,
        }
    }

    mock_stub!(&self, find_dangling_edges(_space: &str) -> Result<Vec<Edge>, StorageError>, Ok(Vec::new()));
    mock_stub!(&mut self, repair_dangling_edges(_space: &str) -> Result<usize, StorageError>, Ok(0));
    mock_stub!(&self, get_db_path() -> &str, "");
}

impl StoragePersistenceOps for MockStorage {
    fn flush(&self) -> crate::core::StorageResult<()> {
        Ok(())
    }

    fn create_checkpoint(
        &self,
    ) -> crate::core::StorageResult<Option<crate::storage::CheckpointStats>> {
        Ok(None)
    }

    fn verify_snapshot(&self, _snapshot_id: u64) -> crate::core::StorageResult<bool> {
        Ok(false)
    }

    fn cleanup_snapshots(&self) -> crate::core::StorageResult<usize> {
        Ok(0)
    }

    fn snapshot_stats(&self) -> crate::storage::SnapshotStats {
        Default::default()
    }

    fn compact(&self, _config: &crate::core::types::CompactConfig) -> crate::core::StorageResult<()> {
        Ok(())
    }

    fn save_data_to_dir(&self, _dir: &std::path::Path) -> crate::core::StorageResult<()> {
        Ok(())
    }

    fn should_flush(&self) -> bool {
        false
    }

    fn should_checkpoint(&self) -> bool {
        false
    }
}

impl StorageSchemaContextOps for MockStorage {
    fn get_schema_manager(&self) -> Option<Arc<crate::core::metadata::SchemaManager>> {
        Some(self.schema_manager.clone())
    }
}

impl StorageTransactionContextOps for MockStorage {
    fn get_transaction_context(&self) -> Option<Arc<TransactionContextInfo>> {
        self.transaction_context.read().clone()
    }

    fn set_transaction_context(&self, context: Option<Arc<TransactionContextInfo>>) {
        *self.transaction_context.write() = context;
    }
}

impl StorageSyncContextOps for MockStorage {
    fn get_sync_manager(&self) -> Option<Arc<crate::sync::SyncManager>> {
        None
    }
}

impl UndoTarget for MockStorage {
    fn delete_vertex_type(
        &self,
        label: LabelId,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph.delete_vertex_type(label)
    }

    fn delete_edge_type(
        &self,
        edge_key: crate::core::types::EdgeKey,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph.delete_edge_type(edge_key)
    }

    fn delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph.delete_vertex(vertex.label, &vertex.vid.to_string(), ts)
            .map_err(|e| crate::transaction::undo_log::UndoLogError::UndoFailed(e.to_string()))
    }

    fn delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        let edge_id = &edge_ctx.edge_id;
        let params = crate::storage::engine::params::EdgeOperationParams {
            edge_label: edge_id.edge_label,
            src_label: edge_id.src_label,
            src_id: edge_id.src_vid,
            dst_label: edge_id.dst_label,
            dst_id: edge_id.dst_vid,
            rank: edge_id.rank,
        };
        self.graph.delete_edge(&params, edge_ctx.timestamp)
            .map(|_| ())
            .map_err(|e| crate::transaction::undo_log::UndoLogError::UndoFailed(e.to_string()))
    }

    fn undo_update_vertex_property(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        col_id: crate::core::types::ColumnId,
        value: crate::transaction::undo_log::PropertyValue,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph
            .undo_update_vertex_property(vertex, col_id, value, ts)
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
        self.graph
            .undo_update_edge_property(edge_id, oe_offset, ie_offset, col_id, value, ts)
    }

    fn revert_delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph.revert_delete_vertex(vertex, ts)
    }

    fn revert_delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph.revert_delete_edge(edge_ctx)
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph
            .revert_delete_vertex_properties(label_name, prop_names)
    }

    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph
            .revert_delete_edge_properties(src_label, dst_label, edge_label, prop_names)
    }

    fn revert_delete_vertex_label(
        &self,
        label_name: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph.revert_delete_vertex_label(label_name)
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph
            .revert_delete_edge_label(src_label, dst_label, edge_label)
    }

    fn revert_rename_vertex_properties(
        &self,
        label_name: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph
            .revert_rename_vertex_properties(label_name, current_names, original_names)
    }

    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph.revert_rename_edge_properties(
            src_label,
            dst_label,
            edge_label,
            current_names,
            original_names,
        )
    }
}

impl StorageRecoveryOps for MockStorage {
    fn needs_recovery(&self) -> bool {
        false
    }

    fn recover_from_wal(
        &self,
    ) -> crate::core::StorageResult<crate::transaction::wal::recovery::RecoveryStats> {
        Ok(Default::default())
    }

    fn recover_from_wal_with_config(
        &self,
        _config: crate::transaction::wal::recovery::RecoveryConfig,
    ) -> crate::core::StorageResult<crate::transaction::wal::recovery::RecoveryStats> {
        Ok(Default::default())
    }
}

impl StorageGcOps for MockStorage {
    fn is_index_gc_running(&self) -> bool {
        false
    }

    fn start_index_gc(&self) -> Option<std::thread::JoinHandle<()>> {
        None
    }

    fn stop_index_gc(&self) {}
}

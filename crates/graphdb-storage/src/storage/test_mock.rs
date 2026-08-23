use crate::core::error::StorageError;
use crate::core::metadata::IndexMetadataManager;
use crate::core::types::{
    EdgeTypeInfo, EdgeTypeSchema, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo,
    PropertyDef, SpaceInfo, TagInfo, UpdateInfo, UserAlterInfo, UserInfo, VertexId,
};
use crate::core::{Edge, EdgeDirection, RoleType, StorageResult, Value, Vertex};
use crate::storage::engine::graph_storage::GraphStorageContext;
use crate::storage::{
    LabelVersionHistory, PropertyChange, StorageAdmin, StorageAuthOps, StorageGcOps,
    StorageOperationContext, StorageOperationContextOps, StoragePersistenceOps, StorageReader,
    StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps, StorageStats,
    StorageSyncContextOps, StorageWriter,
};
use crate::transaction::UndoTarget;
use parking_lot::RwLock;
use std::collections::HashMap;
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
    operation_context: Option<Arc<StorageOperationContext>>,
    fail_insert_edge: Arc<RwLock<bool>>,
    fail_delete_edge: Arc<RwLock<bool>>,
    fail_batch_insert_edges: Arc<RwLock<bool>>,
    edge_types: Arc<RwLock<Vec<EdgeTypeInfo>>>,
    edges: Arc<RwLock<Vec<Edge>>>,
    /// Vertices keyed by space (mirrors the graph storage API used by
    /// executor-level tests for the point-lookup sources).
    vertices: Arc<RwLock<HashMap<String, Vec<Vertex>>>>,
}

impl MockStorage {
    pub fn new() -> Result<Self, StorageError> {
        Ok(Self {
            graph: GraphStorageContext::new(),
            schema_manager: Arc::new(crate::core::metadata::SchemaManager::new()),
            operation_context: None,
            fail_insert_edge: Arc::new(RwLock::new(false)),
            fail_delete_edge: Arc::new(RwLock::new(false)),
            fail_batch_insert_edges: Arc::new(RwLock::new(false)),
            edge_types: Arc::new(RwLock::new(Vec::new())),
            edges: Arc::new(RwLock::new(Vec::new())),
            vertices: Arc::new(RwLock::new(HashMap::new())),
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

    pub fn set_edge_types(&self, edge_types: Vec<EdgeTypeInfo>) {
        *self.edge_types.write() = edge_types;
    }

    pub fn set_edges(&self, edges: Vec<Edge>) {
        *self.edges.write() = edges;
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

/// The mock keeps its rows outside the engine's tables, so there is nothing
/// meaningful to report; the defaulted no-op snapshots make consumers fall
/// back to their sampling path.
impl crate::storage::stats_reader::ColumnStatsReader for MockStorage {}

impl StorageReader for MockStorage {
    fn get_vertex(&self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError> {
        Ok(self
            .vertices
            .read()
            .get(space)
            .and_then(|vertices| vertices.iter().find(|v| v.vid == *id).cloned()))
    }

    fn get_vertex_projected(
        &self,
        space: &str,
        id: &VertexId,
        projection: &[String],
    ) -> Result<Option<Vertex>, StorageError> {
        let vertex = self.get_vertex(space, id)?;
        if projection.is_empty() {
            return Ok(vertex);
        }
        Ok(vertex.map(|mut v| {
            v.properties.retain(|k, _| projection.contains(k));
            v
        }))
    }
    mock_stub!(&self, scan_vertices(_space: &str) -> Result<Vec<Vertex>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_vertices_by_tag(_space: &str, _tag: &str) -> Result<Vec<Vertex>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_vertices_by_prop(_space: &str, _tag: &str, _prop: &str, _value: &Value) -> Result<Vec<Vertex>, StorageError>, Ok(Vec::new()));
    fn get_edge(
        &self,
        _space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<Option<Edge>, StorageError> {
        Ok(self
            .edges
            .read()
            .iter()
            .find(|edge| {
                edge.src == *src
                    && edge.dst == *dst
                    && edge.edge_type == edge_type
                    && edge.ranking == rank
            })
            .cloned())
    }
    mock_stub!(&self, get_node_edges(_space: &str, _node_id: &VertexId, _direction: EdgeDirection) -> Result<Vec<Edge>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, neighbor_dst_ids_batch(_space: &str, _src_ids: &[VertexId], _direction: EdgeDirection, _edge_types: &[String]) -> Result<Vec<Vec<VertexId>>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, out_degree_batch(_space: &str, _src_ids: &[VertexId], _direction: EdgeDirection, _edge_types: &[String]) -> Result<Vec<usize>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_edges_by_type(_space: &str, _edge_type: &str) -> Result<Vec<Edge>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, scan_all_edges(_space: &str) -> Result<Vec<Edge>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, count_vertices_by_tag(_space: &str, _tag: &str) -> Result<u64, StorageError>, Ok(0));
    mock_stub!(&self, count_edges_by_type(_space: &str, _edge_type: &str) -> Result<u64, StorageError>, Ok(0));
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
    fn list_edge_types(&self, _space: &str) -> Result<Vec<EdgeTypeSchema>, StorageError> {
        Ok(self.edge_types.read().clone())
    }
    mock_stub!(&self, get_tag_index(_space: &str, _index: &str) -> Result<Option<Index>, StorageError>, Ok(None));
    mock_stub!(&self, list_tag_indexes(_space: &str) -> Result<Vec<Index>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_edge_index(_space: &str, _index: &str) -> Result<Option<Index>, StorageError>, Ok(None));
    mock_stub!(&self, list_edge_indexes(_space: &str) -> Result<Vec<Index>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_vertex_version_history(_space: &str, _tag: &str) -> Result<Option<LabelVersionHistory>, StorageError>, Ok(None));
    mock_stub!(&self, get_edge_version_history(_space: &str, _edge_type: &str) -> Result<Option<LabelVersionHistory>, StorageError>, Ok(None));
    mock_stub!(&self, get_vertex_schema_changes(_space: &str, _tag: &str, _from_version: u64, _to_version: u64) -> Result<Vec<PropertyChange>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, get_edge_schema_changes(_space: &str, _edge_type: &str, _from_version: u64, _to_version: u64) -> Result<Vec<PropertyChange>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, detect_vertex_breaking_changes(_space: &str, _tag: &str, _from_version: u64, _to_version: u64) -> Result<Vec<PropertyChange>, StorageError>, Ok(Vec::new()));
    mock_stub!(&self, detect_edge_breaking_changes(_space: &str, _edge_type: &str, _from_version: u64, _to_version: u64) -> Result<Vec<PropertyChange>, StorageError>, Ok(Vec::new()));
}

impl StorageWriter for MockStorage {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError> {
        self.vertices
            .write()
            .entry(space.to_string())
            .or_default()
            .push(vertex.clone());
        Ok(vertex.vid)
    }
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
    fn update_edge(&mut self, _space: &str, _edge: Edge) -> Result<(), StorageError> {
        Ok(())
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
    mock_stub!(&mut self, create_edge_index(_space: &str, _info: &Index) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, drop_edge_index(_space: &str, _index: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, rebuild_edge_index(_space: &str, _index: &str) -> Result<bool, StorageError>, Ok(true));
}

impl StorageAuthOps for MockStorage {
    mock_stub!(&mut self, change_password(_info: &PasswordInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, create_user(_info: &UserInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, alter_user(_info: &UserAlterInfo) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&mut self, drop_user(_username: &str) -> Result<bool, StorageError>, Ok(true));
    mock_stub!(&self, user_exists(_username: &str) -> bool, false);
    mock_stub!(&self, list_users() -> Vec<String>, Vec::new());
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

    fn persistence_diagnostics(&self) -> Option<crate::storage::PersistenceDiagnostics> {
        None
    }

    fn compact(
        &self,
        _config: &crate::core::types::CompactConfig,
    ) -> crate::core::StorageResult<()> {
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

    fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>> {
        None
    }
}

impl StorageOperationContextOps for MockStorage {
    fn bind_auto_commit_context(&self) -> StorageResult<Self> {
        Ok(self.bind_operation_context(StorageOperationContext {
            transaction_id: None,
            read_timestamp: 1,
            write_timestamp: Some(1),
            read_only: false,
            auto_commit: true,
            mutation_recorder: None,
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        }))
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self {
        let mut bound = self.clone();
        bound.operation_context = Some(Arc::new(context));
        bound
    }

    fn bind_read_operation_context(&self) -> StorageResult<Self> {
        Ok(self.bind_operation_context(StorageOperationContext {
            transaction_id: None,
            read_timestamp: 1,
            write_timestamp: None,
            read_only: true,
            auto_commit: true,
            mutation_recorder: None,
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        }))
    }

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.operation_context.clone()
    }

    fn finalize_operation(&self, _committed: bool) -> crate::core::StorageResult<()> {
        Ok(())
    }
}

impl crate::storage::StorageCommitOps for MockStorage {
    fn commit_staged_writes(
        &self,
        _transaction_id: crate::core::types::TransactionId,
        _intents: &[crate::core::wal::OutboxIntent],
    ) -> crate::core::StorageResult<crate::core::types::CommitLsn> {
        Ok(crate::core::types::CommitLsn::ZERO)
    }

    fn abort_staged_writes(
        &self,
        _transaction_id: crate::core::types::TransactionId,
    ) -> crate::core::StorageResult<()> {
        Ok(())
    }

    fn recover_outbox_projection(
        &self,
        _sync_manager: &crate::sync::SyncManager,
    ) -> crate::core::StorageResult<usize> {
        Ok(0)
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
        self.graph
            .delete_vertex(vertex.label, &vertex.vid.to_string(), ts)
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
        self.graph
            .delete_edge(&params, edge_ctx.timestamp)
            .map(|_| ())
            .map_err(|e| crate::transaction::undo_log::UndoLogError::UndoFailed(e.to_string()))
    }

    fn undo_update_vertex_property(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        col_id: crate::core::types::ColumnId,
        value: crate::core::Value,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph
            .undo_update_vertex_property(vertex, col_id, value, ts)
    }

    fn undo_update_edge_property(
        &self,
        edge_id: crate::core::types::EdgeIdentifier,
        col_id: crate::core::types::ColumnId,
        value: crate::core::Value,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.graph
            .undo_update_edge_property(edge_id, col_id, value, ts)
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

    fn start_index_gc(&self) -> Option<crate::storage::thread_pool::BackgroundTaskHandle> {
        None
    }

    fn stop_index_gc(&self) {}
}

// The mock has no auto-commit batch/group window; the stubs exist so the
// `QueryStorage` blanket impl (required by snapshot tests and the sync
// wrapper's generic bounds) applies to `MockStorage`.
impl crate::storage::AutoCommitBatchOps for MockStorage {
    fn begin_auto_commit_batch(
        &self,
    ) -> StorageResult<Arc<crate::storage::engine::graph_storage::AutoCommitBatchWindow>> {
        Err(StorageError::not_supported(
            "MockStorage does not support auto-commit batches",
        ))
    }

    fn bind_auto_commit_statement(
        &self,
        _window: &Arc<crate::storage::engine::graph_storage::AutoCommitBatchWindow>,
    ) -> StorageResult<Self>
    where
        Self: Sized,
    {
        Err(StorageError::not_supported(
            "MockStorage does not support auto-commit batches",
        ))
    }

    fn finalize_auto_commit_batch(
        &self,
        _window: &crate::storage::engine::graph_storage::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        Err(StorageError::not_supported(
            "MockStorage does not support auto-commit batches",
        ))
    }
}

impl crate::storage::AutoCommitGroupOps for MockStorage {
    fn begin_auto_commit_group(
        &self,
    ) -> StorageResult<Arc<crate::storage::engine::graph_storage::AutoCommitBatchWindow>> {
        Err(StorageError::not_supported(
            "MockStorage does not support group commit",
        ))
    }

    fn finalize_auto_commit_group(
        &self,
        _window: &crate::storage::engine::graph_storage::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        Err(StorageError::not_supported(
            "MockStorage does not support group commit",
        ))
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::storage::client::StorageOperationContextOps;
    use crate::storage::QueryStorage;

    #[test]
    fn unbound_storage_reports_no_snapshot() {
        let storage = MockStorage::new().expect("MockStorage should be created");
        assert!(storage.snapshot_handle().is_none());
    }

    #[test]
    fn read_bound_storage_pins_snapshot_handle() {
        let storage = MockStorage::new().expect("MockStorage should be created");
        let bound = storage
            .bind_read_operation_context()
            .expect("read binding should succeed");
        let handle = bound
            .snapshot_handle()
            .expect("bound storage has a snapshot");
        assert_eq!(handle.ts, 1);
    }

    #[test]
    fn auto_commit_bound_storage_pins_snapshot_handle() {
        let storage = MockStorage::new().expect("MockStorage should be created");
        let bound = storage
            .bind_auto_commit_context()
            .expect("auto-commit binding should succeed");
        let handle = bound
            .snapshot_handle()
            .expect("auto-commit storage has a snapshot");
        assert_eq!(handle.ts, 1);
    }
}

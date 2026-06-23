use crate::core::metadata::SchemaManager;
use crate::core::types::TransactionContextInfo;
use crate::core::types::{
    EdgeTypeInfo, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo, PropertyDef,
    SpaceInfo, TagInfo, Timestamp, UpdateInfo, UserAlterInfo, UserInfo, VertexId, CompactConfig,
};
use crate::core::{Edge, EdgeDirection, RoleType, StorageError, StorageResult, Value, Vertex};
use crate::storage::engine::background_freeze::FreezeStats;
use crate::storage::engine::graph_storage::context::ExportedEdgeSnapshotRecord;
use crate::transaction::wal::recovery::{RecoveryConfig, RecoveryStats};
use crate::transaction::UndoTarget;
use std::sync::Arc;

/// Read-only data and schema operations.
pub trait StorageReader: Send + Sync + std::fmt::Debug {
    fn get_vertex(&self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError>;
    fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError>;
    fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError>;
    fn scan_vertices_by_prop(
        &self,
        space: &str,
        tag: &str,
        prop: &str,
        value: &Value,
    ) -> Result<Vec<Vertex>, StorageError>;

    fn get_edge(
        &self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<Option<Edge>, StorageError>;
    fn get_node_edges(
        &self,
        space: &str,
        node_id: &VertexId,
        direction: EdgeDirection,
    ) -> Result<Vec<Edge>, StorageError>;
    fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError>;
    fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;

    fn lookup_index(
        &self,
        space: &str,
        index: &str,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError>;
    fn get_vertex_with_schema(
        &self,
        space: &str,
        tag: &str,
        id: &Value,
    ) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError>;
    fn get_edge_with_schema(
        &self,
        space: &str,
        edge_type: &str,
        src: &Value,
        dst: &Value,
    ) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError>;
    fn scan_vertices_with_schema(
        &self,
        space: &str,
        tag: &str,
    ) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError>;
    fn scan_edges_with_schema(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError>;

    fn get_space(&self, space: &str) -> Result<Option<SpaceInfo>, StorageError>;
    fn get_space_by_id(&self, space_id: u64) -> Result<Option<SpaceInfo>, StorageError>;
    fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError>;
    fn get_space_id(&self, space: &str) -> Result<u64, StorageError>;
    fn space_exists(&self, space: &str) -> bool;

    fn get_tag(&self, space: &str, tag: &str) -> Result<Option<TagInfo>, StorageError>;
    fn list_tags(&self, space: &str) -> Result<Vec<TagInfo>, StorageError>;

    fn get_edge_type(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Option<EdgeTypeInfo>, StorageError>;
    fn list_edge_types(&self, space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError>;

    fn get_tag_index(&self, space: &str, index: &str) -> Result<Option<Index>, StorageError>;
    fn list_tag_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError>;
}

/// Write operations for vertex and edge data.
pub trait StorageWriter: Send + Sync + std::fmt::Debug {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError>;
    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError>;
    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError>;
    fn delete_vertex_with_edges(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError>;
    fn batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError>;
    fn delete_tags(
        &mut self,
        space: &str,
        vertex_id: &VertexId,
        tag_names: &[String],
    ) -> Result<usize, StorageError>;

    fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError>;
    fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError>;

    fn insert_vertex_data(
        &mut self,
        space: &str,
        info: &InsertVertexInfo,
    ) -> Result<bool, StorageError>;
    fn insert_edge_data(
        &mut self,
        space: &str,
        info: &InsertEdgeInfo,
    ) -> Result<bool, StorageError>;
    fn delete_vertex_data(&mut self, space: &str, vertex_id: &str) -> Result<bool, StorageError>;
    fn delete_edge_data(
        &mut self,
        space: &str,
        src: &str,
        dst: &str,
        rank: i64,
    ) -> Result<bool, StorageError>;
    fn update_data(
        &mut self,
        space: &str,
        space_id: u64,
        info: &UpdateInfo,
    ) -> Result<bool, StorageError>;
}

/// Schema/space/tag/edge-type/index DDL operations.
pub trait StorageSchemaOps: Send + Sync + std::fmt::Debug {
    fn create_space(&mut self, space: &mut SpaceInfo) -> Result<bool, StorageError>;
    fn drop_space(&mut self, space: &str) -> Result<bool, StorageError>;
    fn clear_space(&mut self, space: &str) -> Result<bool, StorageError>;
    fn alter_space_comment(&mut self, space_id: u64, comment: String)
        -> Result<bool, StorageError>;

    fn create_tag(&mut self, space: &str, tag: &TagInfo) -> Result<u32, StorageError>;
    fn alter_tag(
        &mut self,
        space: &str,
        tag: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError>;
    fn rename_vertex_property(
        &mut self,
        label: LabelId,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), StorageError>;
    fn rename_tag_property(
        &mut self,
        space: &str,
        tag: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError>;
    fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError>;

    fn create_edge_type(&mut self, space: &str, edge: &EdgeTypeInfo) -> Result<u32, StorageError>;
    fn alter_edge_type(
        &mut self,
        space: &str,
        edge_type: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError>;
    fn drop_edge_type(&mut self, space: &str, edge_type: &str) -> Result<bool, StorageError>;

    fn create_tag_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
    fn drop_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    fn rebuild_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
}

/// Authentication and authorization operations.
pub trait StorageAuthOps: Send + Sync + std::fmt::Debug {
    fn change_password(&mut self, info: &PasswordInfo) -> Result<bool, StorageError>;
    fn create_user(&mut self, info: &UserInfo) -> Result<bool, StorageError>;
    fn alter_user(&mut self, info: &UserAlterInfo) -> Result<bool, StorageError>;
    fn drop_user(&mut self, username: &str) -> Result<bool, StorageError>;
    fn user_exists(&self, username: &str) -> bool;
    fn grant_role(
        &mut self,
        username: &str,
        space_id: u64,
        role: RoleType,
    ) -> Result<bool, StorageError>;
    fn revoke_role(&mut self, username: &str, space_id: u64) -> Result<bool, StorageError>;
}

/// Administrative operations: stats, maintenance, optional components.
pub trait StorageAdmin: Send + Sync + std::fmt::Debug {
    fn load_from_disk(&mut self) -> Result<(), StorageError>;
    fn save_to_disk(&self) -> Result<(), StorageError>;
    fn get_storage_stats(&self) -> StorageStats;

    fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
    fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError>;

    fn get_db_path(&self) -> &str;
}

/// Persistence operations for flushing, checkpointing, and compaction.
pub trait StoragePersistenceOps: Send + Sync + std::fmt::Debug {
    fn flush(&self) -> StorageResult<()>;

    fn create_checkpoint(&self) -> StorageResult<Option<crate::storage::CheckpointStats>>;

    fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool>;

    fn cleanup_snapshots(&self) -> StorageResult<usize>;

    fn snapshot_stats(&self) -> crate::storage::SnapshotStats;

    fn compact(&self, config: &CompactConfig) -> StorageResult<()>;

    fn save_data(&self) -> StorageResult<()> {
        self.flush()
    }

    fn save_data_to_dir(&self, dir: &std::path::Path) -> StorageResult<()>;

    fn auto_flush_if_needed(&self) -> StorageResult<bool> {
        if self.should_flush() {
            self.flush()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn auto_checkpoint_if_needed(&self) -> StorageResult<Option<crate::storage::CheckpointStats>> {
        if self.should_checkpoint() {
            self.create_checkpoint()
        } else {
            Ok(None)
        }
    }

    fn should_flush(&self) -> bool;

    fn should_checkpoint(&self) -> bool;
}

/// Access to persistent schema context shared with higher-level components.
pub trait StorageSchemaContextOps: Send + Sync + std::fmt::Debug {
    fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
}

/// Access to transaction runtime context shared with higher-level components.
pub trait StorageTransactionContextOps: Send + Sync + std::fmt::Debug {
    fn get_transaction_context(&self) -> Option<Arc<TransactionContextInfo>>;

    fn set_transaction_context(&self, context: Option<Arc<TransactionContextInfo>>);
}

/// Access to sync runtime context shared with higher-level components.
pub trait StorageSyncContextOps: Send + Sync + std::fmt::Debug {
    fn get_sync_manager(&self) -> Option<Arc<crate::sync::SyncManager>>;
}

/// WAL recovery operations.
pub trait StorageRecoveryOps: Send + Sync + std::fmt::Debug {
    fn needs_recovery(&self) -> bool;

    fn recover_from_wal(&self) -> StorageResult<RecoveryStats>;

    fn recover_from_wal_with_config(&self, config: RecoveryConfig) -> StorageResult<RecoveryStats>;

    fn init_with_recovery(&self) -> StorageResult<Option<RecoveryStats>> {
        if self.needs_recovery() {
            let stats = self.recover_from_wal()?;
            Ok(Some(stats))
        } else {
            Ok(None)
        }
    }
}

/// Index GC operations.
pub trait StorageGcOps: Send + Sync + std::fmt::Debug {
    fn is_index_gc_running(&self) -> bool;

    fn start_index_gc(&self) -> Option<std::thread::JoinHandle<()>>;

    fn stop_index_gc(&self);
}

/// Combined storage interface with full read/write/schema/auth/admin capabilities.
///
/// Runtime context accessors such as schema, transaction, and sync context are kept
/// as separate traits so higher-level components only depend on them when necessary.
pub trait StorageClient:
    StorageReader
    + StorageWriter
    + StorageSchemaOps
    + StorageSchemaContextOps
    + StorageAuthOps
    + StorageAdmin
    + StoragePersistenceOps
    + StorageRecoveryOps
    + StorageGcOps
    + UndoTarget
    + Send
    + Sync
    + std::fmt::Debug
{
}

impl<T> StorageClient for T where
    T: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + StorageSchemaContextOps
        + StorageAuthOps
        + StorageAdmin
        + StoragePersistenceOps
        + StorageRecoveryOps
        + StorageGcOps
        + UndoTarget
        + Send
        + Sync
        + std::fmt::Debug
{
}

/// Snapshot export and background freeze operations.
pub trait StorageSnapshotOps: Send + Sync + std::fmt::Debug {
    fn export_snapshot(&self, ts: Timestamp) -> StorageResult<Vec<ExportedEdgeSnapshotRecord>>;
    fn get_freeze_stats(&self) -> Option<FreezeStats>;
    fn trigger_background_freeze(&self) -> StorageResult<()>;
}

/// Storing statistical information
#[derive(Debug, Clone)]
pub struct StorageStats {
    pub total_vertices: usize,
    pub total_edges: usize,
    pub total_spaces: usize,
    pub total_tags: usize,
    pub total_edge_types: usize,
    /// Total allocated storage size in bytes (vertex tables + edge tables + indexes)
    pub total_size_bytes: u64,
    /// Data size in bytes (vertex + edge data, excluding index structures)
    pub data_size_bytes: u64,
    /// Property index structure size in bytes
    pub index_size_bytes: u64,
}

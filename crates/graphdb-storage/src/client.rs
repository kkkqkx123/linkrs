use crate::cursor::{EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, ScanOptions, VertexCursor};
use crate::engine::background_freeze::FreezeStats;
use crate::engine::graph_storage::context::ExportedEdgeSnapshotRecord;
use crate::mvcc::SnapshotHandle;
use crate::schema::{LabelVersionHistory, PropertyChange};
use graphdb_core::metadata::{IndexMetadataManager, SchemaManager};
use graphdb_core::types::TransactionId;
use graphdb_core::types::{
    CompactConfig, EdgeTypeInfo, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo,
    PropertyDef, SpaceInfo, TagInfo, Timestamp, UpdateInfo, UserAlterInfo, UserInfo, VertexId,
};
use graphdb_core::{Edge, EdgeDirection, RoleType, StorageError, StorageResult, Value, Vertex};
use graphdb_transaction::wal::recovery::{RecoveryConfig, RecoveryStats};
use graphdb_transaction::UndoTarget;
use std::path::Path;
use std::sync::Arc;

use graphdb_transaction::TransactionMutationRecorder;

/// Read-only data and schema operations.
pub trait StorageReader: Send + Sync + std::fmt::Debug {
    fn get_vertex(&self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError>;

    /// Monotonic physical layout version of the vertex/edge segment layout.
    ///
    /// Bumped on segment allocation, merge, compaction, eviction, restore,
    /// and cold-snapshot load/merge. `0` means the implementation does not
    /// track a layout version (default) — consumers then cannot use it to
    /// invalidate cached plans.
    fn layout_version(&self) -> u64 {
        0
    }

    /// Self-proven vertex-id domain covering a whole space.
    ///
    /// Returns `Some(min..max)` only when the storage can prove that every
    /// vertex id written to the space is a non-negative i64 within that
    /// range. `None` means no proof exists (mixed or string ids, or no
    /// writes) and partition planning must not guess a range.
    fn vertex_id_domain(&self, space: &str) -> Option<std::ops::Range<i64>> {
        let _ = space;
        None
    }

    /// Fetch a vertex with only the requested properties.
    ///
    /// The default implementation calls [`get_vertex`] and filters the
    /// property map.  Storage engines that natively support column projection
    /// should override this to avoid reading unneeded columns.
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

    fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError>;
    fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError>;
    fn scan_vertices_by_tag_paginated(
        &self,
        space: &str,
        tag: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Vertex>, StorageError> {
        let _ = (space, tag, offset, limit);
        Err(StorageError::not_supported(
            "Native vertex pagination is not supported by this storage implementation",
        ))
    }
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

    /// Fetch an edge with only the requested properties.
    ///
    /// The default implementation calls [`get_edge`] and filters the
    /// property map.  Storage engines that natively support column projection
    /// should override this to avoid decoding unneeded columns.
    fn get_edge_projected(
        &self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
        projection: &[String],
    ) -> Result<Option<Edge>, StorageError> {
        let edge = self.get_edge(space, src, dst, edge_type, rank)?;
        if projection.is_empty() {
            return Ok(edge);
        }
        Ok(edge.map(|mut e| {
            e.props.retain(|k, _| projection.contains(k));
            e
        }))
    }
    fn get_node_edges(
        &self,
        space: &str,
        node_id: &VertexId,
        direction: EdgeDirection,
    ) -> Result<Vec<Edge>, StorageError>;

    /// Lightweight batch neighbor read used by de-materialized expand hops
    /// (`id_only`/`count_only`).  Resolves the edge-type schema once for the
    /// batch and reads MVCC neighbors directly from the CSR, skipping
    /// `EdgeRecord` materialization and per-edge property decoding.  Cold
    /// snapshots are merged with the same dedup semantics as [`get_node_edges`].
    ///
    /// Returns the external neighbor `VertexId`s per input source id, in input
    /// order.
    fn neighbor_dst_ids_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<Vec<VertexId>>, StorageError>;

    /// Batch out-degree read for count-only expand tails.  Counts distinct
    /// edges (deduped across hot and cold) per source id, in input order.
    fn out_degree_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<usize>, StorageError>;

    fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError>;
    fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
    fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError>;
    fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError>;

    /// Scan edges of a specific type with pagination support.
    /// Returns at most `limit` edges starting from `offset`.
    /// The `offset` parameter is 0-based.
    /// The `limit` parameter controls the page size.
    fn scan_edges_by_type_paginated(
        &self,
        space: &str,
        edge_type: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Edge>, StorageError> {
        let _ = (space, edge_type, offset, limit);
        Err(StorageError::not_supported(
            "Native edge pagination is not supported by this storage implementation",
        ))
    }

    fn lookup_index(
        &self,
        space: &str,
        index: &str,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError>;

    /// Enable the per-table edge property index for `edge_type`, building it
    /// from existing edge data. Returns `true` if the index was enabled.
    fn enable_edge_property_index(
        &self,
        space: &str,
        edge_type: &str,
        pool_capacity: u64,
    ) -> Result<bool, StorageError> {
        let _ = (space, edge_type, pool_capacity);
        Err(StorageError::not_supported(
            "Edge property index management is not supported by this storage implementation",
        ))
    }

    /// Whether the per-table edge property index is enabled for `edge_type`.
    fn has_edge_property_index(&self, space: &str, edge_type: &str) -> Result<bool, StorageError> {
        let _ = (space, edge_type);
        Err(StorageError::not_supported(
            "Edge property index management is not supported by this storage implementation",
        ))
    }

    /// Drop the per-table edge property index for `edge_type`.
    fn disable_edge_property_index(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<(), StorageError> {
        let _ = (space, edge_type);
        Err(StorageError::not_supported(
            "Edge property index management is not supported by this storage implementation",
        ))
    }

    /// Look up edges of `edge_type` whose `prop_name` value falls within
    /// `[lower, upper)` using the per-table edge property index.
    ///
    /// Bounds are `Value`-typed; the storage layer encodes them with the
    /// ordered codec and applies the inclusion flags. Unbounded side = `None`.
    #[allow(clippy::too_many_arguments)]
    fn lookup_edges_by_property_range(
        &self,
        space: &str,
        edge_type: &str,
        prop_name: &str,
        lower: Option<&Value>,
        upper: Option<&Value>,
        include_lower: bool,
        include_upper: bool,
    ) -> Result<Vec<Edge>, StorageError> {
        let _ = (
            space,
            edge_type,
            prop_name,
            lower,
            upper,
            include_lower,
            include_upper,
        );
        Err(StorageError::not_supported(
            "Edge property range lookup is not supported by this storage implementation",
        ))
    }

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

    /// Resolve an edge type name from the storage-level edge type hash.
    ///
    /// Edge index rows carry the edge type as a truncated FNV-1a hash of the
    /// type name (see `edge_entity_ref` in `index/helpers.rs`).  This default
    /// implementation enumerates the space's edge types and matches the hash
    /// using the same shared FNV-1a implementation as the index write path so
    /// the two sides stay consistent.
    fn resolve_edge_type_name(
        &self,
        space: &str,
        hash: u32,
    ) -> Result<Option<String>, StorageError> {
        let edge_types = self.list_edge_types(space)?;
        Ok(edge_types.into_iter().find_map(|edge_type| {
            if crate::index::helpers::stable_hash(edge_type.edge_type_name.as_bytes()) as u32
                == hash
            {
                Some(edge_type.edge_type_name)
            } else {
                None
            }
        }))
    }

    fn get_tag_index(&self, space: &str, index: &str) -> Result<Option<Index>, StorageError>;
    fn list_tag_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError>;

    fn get_edge_index(&self, space: &str, index: &str) -> Result<Option<Index>, StorageError>;
    fn list_edge_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError>;

    /// Schema version history queries
    /// Query version history for a specific vertex tag
    fn get_vertex_version_history(
        &self,
        space: &str,
        tag: &str,
    ) -> Result<Option<LabelVersionHistory>, StorageError>;

    /// Query version history for a specific edge type
    fn get_edge_version_history(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Option<LabelVersionHistory>, StorageError>;

    /// Get schema changes between two versions for a vertex tag
    fn get_vertex_schema_changes(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<PropertyChange>, StorageError>;

    /// Get schema changes between two versions for an edge type
    fn get_edge_schema_changes(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<PropertyChange>, StorageError>;

    /// Detect breaking changes between versions for a vertex tag
    fn detect_vertex_breaking_changes(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<PropertyChange>, StorageError>;

    /// Detect breaking changes between versions for an edge type
    fn detect_edge_breaking_changes(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<PropertyChange>, StorageError>;

    // ── Cursor-based scan methods ──

    /// Create a lazy vertex scan cursor.
    ///
    /// Implementations must provide a native lazy cursor.
    fn create_vertex_cursor(
        &self,
        _space: &str,
        _options: &ScanOptions,
    ) -> Result<Box<dyn VertexCursor>, StorageError> {
        Err(StorageError::not_supported(
            "Native vertex cursor is not supported by this storage implementation",
        ))
    }

    /// Create a lazy edge scan cursor.
    ///
    /// Implementations must provide a native lazy cursor.
    fn create_edge_cursor(
        &self,
        _space: &str,
        _options: &ScanOptions,
    ) -> Result<Box<dyn EdgeCursor>, StorageError> {
        Err(StorageError::not_supported(
            "Native edge cursor is not supported by this storage implementation",
        ))
    }

    /// Create an index cursor for the given index and predicate.
    ///
    /// The default implementation returns a capability error.  Storage
    /// engines with native index cursor support should override this
    /// to return a lazy cursor.
    fn create_index_cursor(
        &self,
        _plan: &IndexScanPlan,
    ) -> Result<Box<dyn IndexCursor<Row = IndexRow>>, StorageError> {
        Err(StorageError::not_supported(
            "Native index cursor is not supported by this storage engine",
        ))
    }

    // ── Migration history ──

    fn list_migration_history(
        &self,
        _space: &str,
        _label: &str,
        _is_edge: bool,
    ) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError> {
        Err(StorageError::not_supported(
            "Migration history is not supported by this storage implementation",
        ))
    }

    fn get_applied_versions(
        &self,
        _space: &str,
        _label: &str,
        _is_edge: bool,
    ) -> Result<Vec<u64>, StorageError> {
        Err(StorageError::not_supported(
            "Migration history is not supported by this storage implementation",
        ))
    }

    fn record_migration_history(
        &self,
        _record: crate::MigrationHistoryRecord,
    ) -> Result<(), StorageError> {
        Err(StorageError::not_supported(
            "Migration history is not supported by this storage implementation",
        ))
    }

    fn list_all_migration_history(
        &self,
    ) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError> {
        Err(StorageError::not_supported(
            "Migration history is not supported by this storage implementation",
        ))
    }
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
    fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
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

    fn create_edge_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
    fn drop_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    fn rebuild_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
}

/// Authentication and authorization operations.
pub trait StorageAuthOps: Send + Sync + std::fmt::Debug {
    fn change_password(&mut self, info: &PasswordInfo) -> Result<bool, StorageError>;
    fn create_user(&mut self, info: &UserInfo) -> Result<bool, StorageError>;
    fn alter_user(&mut self, info: &UserAlterInfo) -> Result<bool, StorageError>;
    fn drop_user(&mut self, username: &str) -> Result<bool, StorageError>;
    fn user_exists(&self, username: &str) -> bool;
    fn list_users(&self) -> Vec<String>;
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

    fn create_checkpoint(&self) -> StorageResult<Option<crate::CheckpointStats>>;

    fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool>;

    fn cleanup_snapshots(&self) -> StorageResult<usize>;

    fn snapshot_stats(&self) -> crate::SnapshotStats;

    fn persistence_diagnostics(&self) -> Option<crate::PersistenceDiagnostics>;

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

    fn auto_checkpoint_if_needed(&self) -> StorageResult<Option<crate::CheckpointStats>> {
        if self.should_checkpoint() {
            self.create_checkpoint()
        } else {
            Ok(None)
        }
    }

    fn should_flush(&self) -> bool;

    fn should_checkpoint(&self) -> bool;

    fn set_outbox_materialized_lsn_provider(
        &self,
        _provider: Arc<
            dyn Fn() -> StorageResult<Option<graphdb_core::types::CommitLsn>> + Send + Sync,
        >,
    ) {
    }
}

/// Access to persistent schema context shared with higher-level components.
pub trait StorageSchemaContextOps: Send + Sync + std::fmt::Debug {
    fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
    fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>>;
}

/// Immutable context bound to one storage operation scope.
#[derive(Debug)]
pub struct StorageOperationContext {
    pub transaction_id: Option<TransactionId>,
    pub read_timestamp: Timestamp,
    pub write_timestamp: Option<Timestamp>,
    pub read_only: bool,
    pub auto_commit: bool,
    pub mutation_recorder: Option<Arc<dyn TransactionMutationRecorder>>,
    /// MVCC snapshot handles for GC coordination - stores (label_id, handle) pairs for vertex tables
    pub mvcc_vertex_snapshot_handles: Vec<(LabelId, SnapshotHandle)>,
    /// Edge table snapshots tracked by timestamp only (no handles needed)
    pub mvcc_edge_snapshot_registered: bool,
    /// Lazily registered vertex labels with their snapshot handles (for unregistration on finalize)
    pub registered_vertex_labels: parking_lot::RwLock<std::collections::HashSet<LabelId>>,
    /// Lazily registered edge partitions (for snapshot unregistration on finalize)
    pub registered_edge_partitions:
        parking_lot::RwLock<std::collections::HashSet<crate::engine::data_store::EdgeTableKey>>,
    /// Undo log entry count at the start of this statement's segment (group
    /// mode only). Used by `finalize_operation` to roll back only the failed
    /// statement's segment when a shared undo log is in use.
    pub auto_commit_group_start: Option<usize>,
}

impl PartialEq for StorageOperationContext {
    fn eq(&self, other: &Self) -> bool {
        self.transaction_id == other.transaction_id
            && self.read_timestamp == other.read_timestamp
            && self.write_timestamp == other.write_timestamp
            && self.read_only == other.read_only
            && self.auto_commit == other.auto_commit
    }
}

impl Eq for StorageOperationContext {}

impl Clone for StorageOperationContext {
    fn clone(&self) -> Self {
        Self {
            transaction_id: self.transaction_id,
            read_timestamp: self.read_timestamp,
            write_timestamp: self.write_timestamp,
            read_only: self.read_only,
            auto_commit: self.auto_commit,
            mutation_recorder: self.mutation_recorder.clone(),
            mvcc_vertex_snapshot_handles: self.mvcc_vertex_snapshot_handles.clone(),
            mvcc_edge_snapshot_registered: self.mvcc_edge_snapshot_registered,
            registered_vertex_labels: parking_lot::RwLock::new(
                self.registered_vertex_labels.read().clone(),
            ),
            registered_edge_partitions: parking_lot::RwLock::new(
                self.registered_edge_partitions.read().clone(),
            ),
            auto_commit_group_start: self.auto_commit_group_start,
        }
    }
}

impl StorageOperationContext {
    pub fn transaction(
        transaction_id: TransactionId,
        timestamp: Timestamp,
        read_only: bool,
    ) -> Self {
        Self {
            transaction_id: Some(transaction_id),
            read_timestamp: timestamp,
            write_timestamp: (!read_only).then_some(timestamp),
            read_only,
            auto_commit: false,
            mutation_recorder: None,
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        }
    }

    pub fn transaction_with_timestamps(
        transaction_id: TransactionId,
        read_timestamp: Timestamp,
        write_timestamp: Option<Timestamp>,
        read_only: bool,
        auto_commit: bool,
    ) -> Self {
        Self {
            transaction_id: Some(transaction_id),
            read_timestamp,
            write_timestamp,
            read_only,
            auto_commit,
            mutation_recorder: None,
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
        }
    }

    pub fn with_mutation_recorder(
        mut self,
        recorder: Arc<dyn TransactionMutationRecorder>,
    ) -> Self {
        self.mutation_recorder = Some(recorder);
        self
    }

    /// Timestamp at which MVCC snapshots are registered for this operation.
    ///
    /// Read-only operations pin their read snapshot; auto-commit writes pin
    /// the write timestamp (the statement both reads and writes at it).
    pub fn snapshot_timestamp(&self) -> Option<Timestamp> {
        if self.read_only {
            Some(self.read_timestamp)
        } else {
            self.write_timestamp
        }
    }
}

pub trait StorageCommitOps: Send + Sync + std::fmt::Debug {
    fn commit_staged_writes(
        &self,
        transaction_id: TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> StorageResult<graphdb_core::types::CommitLsn>;

    fn abort_staged_writes(&self, transaction_id: TransactionId) -> StorageResult<()>;

    fn commit_staged_writes_with_durability(
        &self,
        transaction_id: TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
        _durability: graphdb_core::types::DurabilityLevel,
    ) -> StorageResult<graphdb_core::types::CommitLsn> {
        self.commit_staged_writes(transaction_id, intents)
    }

    fn recover_outbox_projection(
        &self,
        sync_manager: &graphdb_sync::SyncManager,
    ) -> StorageResult<usize>;
}

/// Creates an immutable storage handle bound to a single operation context.
pub trait StorageOperationContextOps: Send + Sync + std::fmt::Debug {
    fn bind_auto_commit_context(&self) -> StorageResult<Self>
    where
        Self: Sized;

    /// Bind a read-only statement context with a fixed snapshot timestamp.
    ///
    /// Read statements get a consistent statement-level snapshot: every
    /// storage access observes the same `read_timestamp`, and per-table MVCC
    /// snapshots are lazily registered on first table access so GC keeps the
    /// versions the statement may still read. The snapshot is unregistered by
    /// [`finalize_operation`](Self::finalize_operation) (or on Drop as a
    /// backstop). The bound `(space, snapshot_ts)` pair is also the
    /// serialization boundary for distributed reads.
    ///
    /// The default implementation returns `not_supported`; engines without a
    /// native read context fall back to the unbound handle.
    fn bind_read_operation_context(&self) -> StorageResult<Self>
    where
        Self: Sized,
    {
        Err(StorageError::not_supported(
            "Read operation context binding is not supported by this storage implementation",
        ))
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self
    where
        Self: Sized;

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>>;

    /// Finalize an operation-owned auto-commit timestamp.
    ///
    /// Explicit transaction contexts are finalized by `TransactionManager`
    /// and therefore treat this as a no-op.
    fn finalize_operation(&self, _committed: bool) -> StorageResult<()> {
        Ok(())
    }
}

/// Access to sync runtime context shared with higher-level components.
pub trait StorageSyncContextOps: Send + Sync + std::fmt::Debug {
    fn get_sync_manager(&self) -> Option<Arc<graphdb_sync::SyncManager>>;
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

    fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle>;

    fn stop_index_gc(&self);
}

/// Logical graph data access used by query execution.
pub trait GraphStore:
    StorageReader
    + StorageWriter
    + StorageOperationContextOps
    + StorageCommitOps
    + UndoTarget
    + Send
    + Sync
    + std::fmt::Debug
{
}

impl<T> GraphStore for T where
    T: StorageReader
        + StorageWriter
        + StorageOperationContextOps
        + StorageCommitOps
        + UndoTarget
        + Send
        + Sync
        + std::fmt::Debug
{
}

/// Catalog and schema access used by query planning and DDL execution.
pub trait CatalogStore:
    StorageSchemaOps + StorageSchemaContextOps + Send + Sync + std::fmt::Debug
{
}

impl<T> CatalogStore for T where
    T: StorageSchemaOps + StorageSchemaContextOps + Send + Sync + std::fmt::Debug
{
}

/// Minimal combined capability required by the query crate.
pub trait QueryStorage:
    GraphStore
    + CatalogStore
    + StorageAuthOps
    + StorageAdmin
    + crate::stats_reader::ColumnStatsReader
    + crate::AutoCommitBatchOps
    + crate::AutoCommitGroupOps
{
    /// Snapshot handle bound to this storage handle, when the handle is
    /// bound to an operation context with a pinned read/write snapshot.
    ///
    /// Read-only statement contexts pin a fixed read timestamp; auto-commit
    /// write contexts pin their write timestamp. Unbound handles (raw global
    /// storage) return `None`. This lets the query layer observe which
    /// snapshot a per-query bound handle reads at, without reaching into the
    /// storage internals.
    ///
    /// When the operation context already registered per-table MVCC snapshot
    /// handles, the first one is preferred (it carries the storage's own
    /// monotonically increasing handle id); otherwise a query-level handle
    /// is synthesized from the pinned timestamp (`id = 0`).
    fn snapshot_handle(&self) -> Option<SnapshotHandle> {
        let context = self.operation_context()?;
        let ts = context.snapshot_timestamp()?;
        Some(
            context
                .mvcc_vertex_snapshot_handles
                .first()
                .map(|(_, handle)| *handle)
                .unwrap_or_else(|| SnapshotHandle::new(ts, 0)),
        )
    }
}
impl<T> QueryStorage for T where
    T: GraphStore
        + CatalogStore
        + StorageAuthOps
        + StorageAdmin
        + crate::stats_reader::ColumnStatsReader
        + crate::AutoCommitBatchOps
        + crate::AutoCommitGroupOps
{
}

/// Maintenance-only capabilities used by server initialization and administration.
pub trait StorageMaintenance: StorageAdmin + StoragePersistenceOps + StorageGcOps {}
impl<T> StorageMaintenance for T where T: StorageAdmin + StoragePersistenceOps + StorageGcOps {}

/// Combined storage interface with full read/write/schema/auth/admin capabilities.
///
/// Runtime context accessors such as schema, transaction, and sync context are kept
/// as separate traits so higher-level components only depend on them when necessary.
pub trait StorageClient:
    StorageReader
    + StorageWriter
    + StorageSchemaOps
    + StorageSchemaContextOps
    + StorageOperationContextOps
    + StorageCommitOps
    + StorageAuthOps
    + StorageAdmin
    + StoragePersistenceOps
    + StorageRecoveryOps
    + StorageGcOps
    + UndoTarget
    + crate::stats_reader::ColumnStatsReader
    + crate::AutoCommitBatchOps
    + crate::AutoCommitGroupOps
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
        + StorageOperationContextOps
        + StorageCommitOps
        + StorageAuthOps
        + StorageAdmin
        + StoragePersistenceOps
        + StorageRecoveryOps
        + StorageGcOps
        + UndoTarget
        + crate::stats_reader::ColumnStatsReader
        + crate::AutoCommitBatchOps
        + crate::AutoCommitGroupOps
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

    // ── ColdSnapshot management ──

    /// List all registered cold snapshots with their metadata.
    fn list_cold_snapshots(&self) -> StorageResult<Vec<ColdSnapshotInfo>>;

    /// Register a cold snapshot from a `.lkcs` file.
    fn load_cold_snapshot(&self, path: &Path) -> StorageResult<ColdSnapshotInfo>;

    /// Drop all cold snapshots of an edge label from the registry. The
    /// underlying `.lkcs` files are left untouched.
    fn remove_cold_snapshot(&self, label: LabelId) -> StorageResult<()>;

    /// Re-export the most recent cold snapshot of `label` to `path`.
    fn export_cold_snapshot(&self, label: LabelId, path: &Path) -> StorageResult<ColdSnapshotInfo>;

    /// Consolidate every registered version of each given label into a
    /// single snapshot at the newest timestamp, replacing the label's shelf.
    /// Returns the merged snapshots' metadata.
    fn merge_cold_snapshots(&self, labels: &[LabelId]) -> StorageResult<Vec<ColdSnapshotInfo>>;

    /// Resolve the directory that cold snapshots are served from, when the
    /// engine is configured with one. Used to expose `.lkcs` files over the
    /// gRPC snapshot share.
    fn cold_snapshot_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// Metadata describing one registered cold snapshot.
#[derive(Debug, Clone)]
pub struct ColdSnapshotInfo {
    pub label: LabelId,
    pub label_name: String,
    pub snapshot_ts: Timestamp,
    pub edge_count: u64,
    pub file_path: String,
    pub file_size: u64,
    pub checksum: u32,
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

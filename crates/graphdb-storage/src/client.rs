use crate::cursor::{EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, ScanOptions, VertexCursor};
use crate::engine::background_freeze::FreezeStats;
use crate::engine::graph_storage::context::ExportedEdgeSnapshotRecord;
use crate::schema::{LabelVersionHistory, PropertyChange};
use crate::SnapshotHandle;
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
    + StoragePersistenceOps
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

    /// Export the given space to CSV files under `path/<space_name>/`.
    ///
    /// Each tag produces a `<tag>.csv` file; each edge type produces a
    /// `<edge_type>.csv` file.  A `schema.json` metadata file records the
    /// space, tags, and edge types with their property schemas.
    fn export_space(
        &self,
        space: &str,
        path: &std::path::Path,
    ) -> Result<(), StorageError> {
        use std::io::Write;

        let base = path.join(space);
        std::fs::create_dir_all(&base)
            .map_err(|e| StorageError::io_error(format!("Failed to create export dir: {e}")))?;

        // Export schema metadata
        let tags = self.list_tags(space)?;
        let edge_types = self.list_edge_types(space)?;

        let schema_meta = serde_json::json!({
            "space": space,
            "tags": tags.iter().map(|t| serde_json::json!({
                "name": t.tag_name,
                "properties": t.properties.iter().map(|p| serde_json::json!({
                    "name": p.name,
                    "type": p.data_type.to_string(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "edge_types": edge_types.iter().map(|e| serde_json::json!({
                "name": e.edge_type_name,
                "src_tag": e.src_tag_name,
                "dst_tag": e.dst_tag_name,
                "properties": e.properties.iter().map(|p| serde_json::json!({
                    "name": p.name,
                    "type": p.data_type.to_string(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });

        let schema_path = base.join("schema.json");
        let mut schema_file = std::fs::File::create(&schema_path)
            .map_err(|e| StorageError::io_error(format!("Failed to create schema.json: {e}")))?;
        schema_file
            .write_all(serde_json::to_string_pretty(&schema_meta).unwrap_or_default().as_bytes())
            .map_err(|e| StorageError::io_error(format!("Failed to write schema.json: {e}")))?;

        // Export vertices by tag
        for tag_info in &tags {
            let vertices = self.scan_vertices_by_tag(space, &tag_info.tag_name)?;
            if vertices.is_empty() {
                continue;
            }

            // Collect all property keys across all vertices
            let mut prop_keys: Vec<String> = vertices
                .iter()
                .flat_map(|v| v.properties.keys())
                .cloned()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            prop_keys.sort();

            let csv_path = base.join(format!("{}.csv", tag_info.tag_name));
            let mut file = std::fs::File::create(&csv_path)
                .map_err(|e| StorageError::io_error(format!("Failed to create {}.csv: {e}", tag_info.tag_name)))?;

            // Write header
            let mut header = vec!["vid".to_string(), "id".to_string()];
            header.extend(prop_keys.clone());
            writeln!(file, "{}", header.join(","))
                .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;

            // Write rows
            for vertex in &vertices {
                let mut row = vec![vertex.vid.to_string(), vertex.id.to_string()];
                for key in &prop_keys {
                    let val = vertex
                        .properties
                        .get(key)
                        .map(format_csv_value)
                        .unwrap_or_default();
                    row.push(val);
                }
                writeln!(file, "{}", row.join(","))
                    .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;
            }
        }

        // Export edges by type
        for edge_info in &edge_types {
            let edges = self.scan_edges_by_type(space, &edge_info.edge_type_name)?;
            if edges.is_empty() {
                continue;
            }

            let mut prop_keys: Vec<String> = edges
                .iter()
                .flat_map(|e| e.props.keys())
                .cloned()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            prop_keys.sort();

            let csv_path = base.join(format!("{}.csv", edge_info.edge_type_name));
            let mut file = std::fs::File::create(&csv_path)
                .map_err(|e| StorageError::io_error(format!("Failed to create {}.csv: {e}", edge_info.edge_type_name)))?;

            let mut header = vec!["src".to_string(), "dst".to_string(), "ranking".to_string()];
            header.extend(prop_keys.clone());
            writeln!(file, "{}", header.join(","))
                .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;

            for edge in &edges {
                let mut row = vec![
                    edge.src.to_string(),
                    edge.dst.to_string(),
                    edge.ranking.to_string(),
                ];
                for key in &prop_keys {
                    let val = edge
                        .props
                        .get(key)
                        .map(format_csv_value)
                        .unwrap_or_default();
                    row.push(val);
                }
                writeln!(file, "{}", row.join(","))
                    .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;
            }
        }

        Ok(())
    }

    /// Import a space from CSV files under `path/<space_name>/`.
    ///
    /// Expects a `schema.json` metadata file and `<tag>.csv` / `<edge_type>.csv` data files.
    fn import_space(
        &mut self,
        space: &str,
        path: &std::path::Path,
    ) -> Result<(), StorageError> {
        import_space_impl(self, space, path)
    }
}
impl<T> QueryStorage for T where
    T: GraphStore
        + CatalogStore
        + StorageAuthOps
        + StorageAdmin
        + StoragePersistenceOps
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

/// Standalone import implementation to avoid `Self: ?Sized` issues in trait default methods.
fn import_space_impl<S: QueryStorage + ?Sized>(
    storage: &mut S,
    space: &str,
    path: &std::path::Path,
) -> Result<(), StorageError> {
    let base = path.join(space);
    if !base.exists() {
        return Err(StorageError::not_found(format!(
            "Import directory '{}' does not exist",
            base.display()
        )));
    }

    let schema_path = base.join("schema.json");
    let schema_content = if schema_path.exists() {
        std::fs::read_to_string(&schema_path)
            .map_err(|e| StorageError::io_error(format!("Failed to read schema.json: {e}")))?
    } else {
        return Err(StorageError::not_found(
            "schema.json not found in import directory".to_string(),
        ));
    };

    let schema_meta: serde_json::Value = serde_json::from_str(&schema_content)
        .map_err(|e| StorageError::parse_error(format!("Invalid schema.json: {e}")))?;

    let vid_type = graphdb_core::types::DataType::String;
    let mut space_info = graphdb_core::types::space::SpaceInfo::new(space.to_string())
        .with_vid_type(vid_type);
    let _ = StorageSchemaOps::create_space(storage, &mut space_info);

    if let Some(tags) = schema_meta.get("tags").and_then(|t| t.as_array()) {
        for tag_meta in tags {
            let tag_name = tag_meta
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if tag_name.is_empty() {
                continue;
            }

            let mut tag_info = graphdb_core::types::tag::TagInfo::new(tag_name.to_string());
            if let Some(props) = tag_meta.get("properties").and_then(|p| p.as_array()) {
                for prop in props {
                    let prop_name = prop.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let prop_type = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                    let data_type = parse_data_type(prop_type);
                    tag_info.properties.push(
                        graphdb_core::types::property::PropertyDef::new(
                            prop_name.to_string(),
                            data_type,
                        ),
                    );
                }
            }
            let _ = StorageSchemaOps::create_tag(storage, space, &tag_info);

            let csv_path = base.join(format!("{tag_name}.csv"));
            if csv_path.exists() {
                import_vertex_csv_from_path(space, tag_name, &csv_path, storage)?;
            }
        }
    }

    if let Some(edge_types) = schema_meta.get("edge_types").and_then(|t| t.as_array()) {
        for et_meta in edge_types {
            let et_name = et_meta.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if et_name.is_empty() {
                continue;
            }

            let mut et_info = graphdb_core::types::edge::EdgeTypeInfo::new(et_name.to_string());
            et_info.src_tag_name = et_meta
                .get("src_tag")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            et_info.dst_tag_name = et_meta
                .get("dst_tag")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(props) = et_meta.get("properties").and_then(|p| p.as_array()) {
                for prop in props {
                    let prop_name = prop.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let prop_type = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                    let data_type = parse_data_type(prop_type);
                    et_info.properties.push(
                        graphdb_core::types::property::PropertyDef::new(
                            prop_name.to_string(),
                            data_type,
                        ),
                    );
                }
            }
            let _ = StorageSchemaOps::create_edge_type(storage, space, &et_info);

            let csv_path = base.join(format!("{et_name}.csv"));
            if csv_path.exists() {
                import_edge_csv_from_path(space, et_name, &csv_path, storage)?;
            }
        }
    }

    Ok(())
}

/// Format a `Value` for CSV export (handles commas and quotes).
fn format_csv_value(v: &graphdb_core::Value) -> String {
    match v {
        graphdb_core::Value::Null(_) => String::new(),
        graphdb_core::Value::Bool(b) => b.to_string(),
        graphdb_core::Value::SmallInt(i) => i.to_string(),
        graphdb_core::Value::Int(i) => i.to_string(),
        graphdb_core::Value::BigInt(i) => i.to_string(),
        graphdb_core::Value::Float(f) => f.to_string(),
        graphdb_core::Value::Double(f) => f.to_string(),
        graphdb_core::Value::String(s) => {
            let s: &str = s.as_ref();
            if s.contains(',') || s.contains('"') || s.contains('\n') {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        }
        other => format!("{}", other),
    }
}

/// Parse a type name string into a `DataType`.
fn parse_data_type(s: &str) -> graphdb_core::types::DataType {
    let upper = s.trim().to_uppercase();
    match upper.as_str() {
        "INT8" | "TINYINT" => graphdb_core::types::DataType::SmallInt,
        "INT16" | "SMALLINT" => graphdb_core::types::DataType::SmallInt,
        "INT32" | "INT" | "INTEGER" => graphdb_core::types::DataType::Int,
        "INT64" | "BIGINT" => graphdb_core::types::DataType::BigInt,
        "FLOAT" | "FLOAT32" => graphdb_core::types::DataType::Float,
        "DOUBLE" | "FLOAT64" => graphdb_core::types::DataType::Double,
        "BOOL" | "BOOLEAN" => graphdb_core::types::DataType::Bool,
        "STRING" | "TEXT" | "VARCHAR" => graphdb_core::types::DataType::String,
        _ => graphdb_core::types::DataType::String,
    }
}

/// Import vertex data from a CSV file into the given space and tag.
fn import_vertex_csv_from_path<W: StorageWriter + ?Sized>(
    space: &str,
    tag_name: &str,
    csv_path: &std::path::Path,
    writer: &mut W,
) -> Result<(), StorageError> {
    use std::io::BufRead;

    let file = std::fs::File::open(csv_path)
        .map_err(|e| StorageError::io_error(format!("Failed to open {}: {e}", csv_path.display())))?;
    let reader = std::io::BufReader::new(file);
    let mut lines = reader.lines();

    // Read header
    let header_line = match lines.next() {
        Some(Ok(line)) => line,
        _ => return Ok(()),
    };
    let headers: Vec<String> = header_line
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect();

    // Find vid and id column indices
    let vid_idx = headers.iter().position(|h| h.as_str() == "vid");
    let id_idx = headers.iter().position(|h| h.as_str() == "id");

    // Property columns (everything that's not vid or id)
    let prop_cols: Vec<(usize, String)> = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| h.as_str() != "vid" && h.as_str() != "id")
        .map(|(i, h)| (i, h.clone()))
        .collect();

    let mut vertices = Vec::new();
    for line in lines {
        let line = line.map_err(|e| StorageError::io_error(format!("CSV read error: {e}")))?;
        let fields: Vec<&str> = line.split(',').collect();

        let vid = vid_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);
        let id = id_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(vid);

        let mut properties = std::collections::HashMap::new();
        for (col_idx, col_name) in &prop_cols {
            if let Some(val_str) = fields.get(*col_idx) {
                let val_str = val_str.trim().trim_matches('"');
                if !val_str.is_empty() {
                    properties.insert(
                        col_name.clone(),
                        graphdb_core::Value::string(val_str),
                    );
                }
            }
        }

        let vertex = graphdb_core::Vertex {
            vid: graphdb_core::types::VertexId::from_int64(id),
            id,
            tags: vec![graphdb_core::Tag::new(tag_name.to_string(), std::collections::HashMap::new())],
            properties,
        };
        vertices.push(vertex);

        if vertices.len() >= 1000 {
            writer.batch_insert_vertices(space, vertices.clone())?;
            vertices.clear();
        }
    }

    if !vertices.is_empty() {
        writer.batch_insert_vertices(space, vertices)?;
    }

    Ok(())
}

/// Import edge data from a CSV file into the given space and edge type.
fn import_edge_csv_from_path<W: StorageWriter + ?Sized>(
    space: &str,
    edge_type: &str,
    csv_path: &std::path::Path,
    writer: &mut W,
) -> Result<(), StorageError> {
    use std::io::BufRead;

    let file = std::fs::File::open(csv_path)
        .map_err(|e| StorageError::io_error(format!("Failed to open {}: {e}", csv_path.display())))?;
    let reader = std::io::BufReader::new(file);
    let mut lines = reader.lines();

    // Read header
    let header_line = match lines.next() {
        Some(Ok(line)) => line,
        _ => return Ok(()),
    };
    let headers: Vec<String> = header_line
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect();

    let src_idx = headers.iter().position(|h| h.as_str() == "src");
    let dst_idx = headers.iter().position(|h| h.as_str() == "dst");
    let rank_idx = headers.iter().position(|h| h.as_str() == "ranking");

    let prop_cols: Vec<(usize, String)> = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| h.as_str() != "src" && h.as_str() != "dst" && h.as_str() != "ranking")
        .map(|(i, h)| (i, h.clone()))
        .collect();

    let mut edges = Vec::new();
    for line in lines {
        let line = line.map_err(|e| StorageError::io_error(format!("CSV read error: {e}")))?;
        let fields: Vec<&str> = line.split(',').collect();

        let src = src_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);
        let dst = dst_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);
        let ranking = rank_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);

        let mut props = std::collections::HashMap::new();
        for (col_idx, col_name) in &prop_cols {
            if let Some(val_str) = fields.get(*col_idx) {
                let val_str = val_str.trim().trim_matches('"');
                if !val_str.is_empty() {
                    props.insert(
                        col_name.clone(),
                        graphdb_core::Value::string(val_str),
                    );
                }
            }
        }

        let edge = graphdb_core::Edge {
            src: graphdb_core::types::VertexId::from_int64(src),
            dst: graphdb_core::types::VertexId::from_int64(dst),
            edge_type: edge_type.to_string(),
            ranking,
            props,
        };
        edges.push(edge);

        if edges.len() >= 1000 {
            writer.batch_insert_edges(space, edges.clone())?;
            edges.clear();
        }
    }

    if !edges.is_empty() {
        writer.batch_insert_edges(space, edges)?;
    }

    Ok(())
}

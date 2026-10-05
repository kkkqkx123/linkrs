//! Read-only graph, catalog and schema queries.

use crate::cursor::{EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, ScanOptions, VertexCursor};
use crate::schema::{LabelVersionHistory, PropertyChange};
use graphdb_core::types::{EdgeTypeInfo, Index, SpaceInfo, TagInfo, VertexId};
use graphdb_core::{Edge, EdgeDirection, StorageError, Value, Vertex};
use std::sync::Arc;

/// Read-only data and schema operations.
pub trait StorageReader: Send + Sync + std::fmt::Debug {
    fn get_vertex(
        &self,
        space: &str,
        tag: &str,
        id: &VertexId,
    ) -> Result<Option<Vertex>, StorageError>;

    /// Monotonic physical layout version of the vertex/edge layout.
    ///
    /// Bumped on compaction, restore, and remap. `0` means the
    /// implementation does not track a layout version (default) —
    /// consumers then cannot use it to invalidate cached plans.
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
        tag: &str,
        id: &VertexId,
        projection: &[std::sync::Arc<str>],
    ) -> Result<Option<Vertex>, StorageError> {
        let vertex = self.get_vertex(space, tag, id)?;
        if projection.is_empty() {
            return Ok(vertex);
        }
        Ok(vertex.map(|mut v| {
            v.tag.properties.retain(|k, _| projection.contains(k));
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
        projection: &[Arc<str>],
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
        edge_types: &[String],
    ) -> Result<Vec<Edge>, StorageError>;

    /// Filtered and projected per-node edge fanout.
    ///
    /// `edge_types` is pushed to storage: empty means all types, otherwise
    /// only matching tables are visited. `projection` trims decoded columns
    /// (`None` means all columns, `Some(&[])` means topology only).
    /// `limit` caps emitted edges per direction branch.
    /// The default implementation filters and trims in memory for adapters;
    /// the native engine overrides with CSR-level pushdown.
    fn get_node_edges_projected(
        &self,
        space: &str,
        node_id: &VertexId,
        direction: EdgeDirection,
        edge_types: &[String],
        projection: Option<&[Arc<str>]>,
        limit: Option<usize>,
    ) -> Result<Vec<Edge>, StorageError> {
        let mut edges = self.get_node_edges(space, node_id, direction, edge_types)?;
        if let Some(projection) = projection {
            if !projection.is_empty() {
                for edge in &mut edges {
                    edge.props.retain(|k, _| projection.contains(k));
                }
            } else {
                for edge in &mut edges {
                    edge.props.clear();
                }
            }
        }
        if let Some(limit) = limit {
            edges.truncate(limit);
        }
        Ok(edges)
    }

    /// Batch vertex point lookups with the schema resolved once.
    ///
    /// Returns one entry per input id in input order. The default
    /// implementation loops over `get_vertex` for adapters; the native
    /// engine overrides with a single-timestamp batched path.
    fn get_vertices_batch(
        &self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
    ) -> Result<Vec<Option<Vertex>>, StorageError> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.get_vertex(space, tag, id)?);
        }
        Ok(out)
    }

    /// Lightweight batch neighbor read used by de-materialized expand hops
    /// (`id_only`/`count_only`).  Resolves the edge-type schema once for the
    /// batch and reads MVCC neighbors directly from the CSR, skipping
    /// `EdgeRecord` materialization and per-edge property decoding.
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
    /// edges per source id, in input order.
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

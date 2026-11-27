//! Read-only graph, catalog and schema queries.

use crate::cursor::{
    EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, OrderedIndexProof, ScanOptions, VertexCursor,
};
use crate::schema::{LabelVersionHistory, PropertyChange};
use linkrs_core::types::{DataType, EdgeTypeInfo, Index, LabelId, SpaceInfo, TagInfo, VertexId};
use linkrs_core::{Edge, EdgeDirection, StorageError, Value, Vertex};
use std::sync::Arc;

/// Topology-only edge identity returned by the batched adjacency reader.
///
/// Kept separate from `EdgeHeader` so a full batch can share one `Arc` per
/// edge type instead of allocating one string per edge.
#[derive(Debug, Clone)]
pub struct EdgeTopology {
    pub src: VertexId,
    pub dst: VertexId,
    pub edge_type: Arc<str>,
    pub ranking: i64,
}

/// Per-edge-type schema needed by one topology-only adjacency read.
#[derive(Debug, Clone)]
pub(crate) struct TopologyTypeSchema {
    pub(crate) name: Arc<str>,
    pub(crate) type_id: u32,
    pub(crate) edge_label: LabelId,
    pub(crate) src_label: Option<LabelId>,
    pub(crate) dst_label: Option<LabelId>,
}

/// Space and edge-type schema resolved once for a query's topology reads.
///
/// The layout version is part of the snapshot contract: a cached instance is
/// only reusable while the physical layout is unchanged.
#[derive(Debug, Clone)]
pub struct TopologySchema {
    pub(crate) space: String,
    pub(crate) vid_type: DataType,
    pub(crate) layout_version: u64,
    pub(crate) entries: Vec<TopologyTypeSchema>,
}

impl TopologySchema {
    /// Resolve through the public read API so non-native readers can build
    /// the same DTO without exposing their internals.
    pub fn resolve<R: StorageReader + ?Sized>(
        reader: &R,
        space: &str,
        edge_types: &[String],
    ) -> Result<Self, StorageError> {
        let space_info = reader
            .get_space(space)?
            .ok_or_else(|| StorageError::not_found(format!("Space {space} not found")))?;
        let mut entries = Vec::new();
        for info in reader
            .list_edge_types(space)?
            .iter()
            .filter(|info| edge_types.is_empty() || edge_types.contains(&info.edge_type_name))
        {
            let src_label = resolve_endpoint_tag_label(reader, space, &info.src_tag_name)?;
            let dst_label = resolve_endpoint_tag_label(reader, space, &info.dst_tag_name)?;
            entries.push(TopologyTypeSchema {
                name: Arc::from(info.edge_type_name.as_str()),
                type_id: entries.len() as u32,
                edge_label: info.edge_type_id,
                src_label,
                dst_label,
            });
        }
        Ok(Self {
            space: space.to_string(),
            vid_type: space_info.vid_type,
            layout_version: reader.layout_version(),
            entries,
        })
    }

    pub(crate) fn matches(&self, space: &str, layout_version: u64) -> bool {
        self.space == space && self.layout_version == layout_version
    }
}

fn resolve_endpoint_tag_label<R: StorageReader + ?Sized>(
    reader: &R,
    space: &str,
    tag: &str,
) -> Result<Option<LabelId>, StorageError> {
    if tag.is_empty() {
        return Ok(Some(0));
    }
    Ok(reader.get_tag(space, tag)?.map(|info| info.tag_id))
}

/// Topology-only edges of one seed batch in parent-major order.
///
/// `child_offsets` has one entry per parent plus a trailing end offset, so a
/// consumer can accumulate directly over `[child_offsets[p], child_offsets[p + 1])`
/// without building one object per edge.
#[derive(Debug, Clone, Default)]
pub struct PackedEdgeTopology {
    pub parents: Vec<VertexId>,
    pub child_offsets: Vec<usize>,
    pub src: Vec<VertexId>,
    pub dst: Vec<VertexId>,
    pub edge_type_ids: Vec<u32>,
    pub edge_type_names: Vec<Arc<str>>,
    pub rankings: Vec<i64>,
    /// Child-aligned storage hints for destination existence reads. `None`
    /// means the far endpoint carries no usable storage row address.
    pub destination_handles: Vec<Option<(u32, LabelId)>>,
}

impl PackedEdgeTopology {
    pub fn parent_count(&self) -> usize {
        self.child_offsets.len().saturating_sub(1)
    }

    pub fn child_range(&self, parent: usize) -> std::ops::Range<usize> {
        if parent >= self.parent_count() {
            return 0..0;
        }
        self.child_offsets[parent]..self.child_offsets[parent + 1]
    }

    pub fn edge_type(&self, index: usize) -> Arc<str> {
        Arc::clone(&self.edge_type_names[self.edge_type_ids[index] as usize])
    }

    pub fn intern_edge_type(&mut self, name: &Arc<str>) -> u32 {
        if let Some(index) = self
            .edge_type_names
            .iter()
            .position(|item| Arc::ptr_eq(item, name) || item == name)
        {
            return index as u32;
        }
        let index = self.edge_type_names.len();
        self.edge_type_names.push(Arc::clone(name));
        index as u32
    }

    /// Expand the packed layout into the per-parent form used by portable
    /// readers and tests. The production topology-only path consumes the
    /// packed arrays directly.
    pub fn to_batches(&self) -> Vec<Vec<EdgeTopology>> {
        let mut batches = Vec::with_capacity(self.parent_count());
        for parent in 0..self.parent_count() {
            let mut batch = Vec::new();
            for index in self.child_range(parent) {
                batch.push(EdgeTopology {
                    src: self.src[index],
                    dst: self.dst[index],
                    edge_type: self.edge_type(index),
                    ranking: self.rankings[index],
                });
            }
            batches.push(batch);
        }
        batches
    }
}

/// Handle-level edge counts for one seed batch.
///
/// Entries are batch-wide and sorted by internal handle. Count-only
/// consumers sum multiplicities globally, so per-parent boundaries carry no
/// information and are deliberately not materialized. `parents` keeps the
/// seed cardinality for diagnostics.
#[derive(Debug, Clone, Default)]
pub struct PackedHandleCounts {
    pub parents: Vec<VertexId>,
    pub handles: Vec<(u32, LabelId)>,
    pub counts: Vec<u64>,
    /// Snapshot, schema, handle and edge-table setup before the CSR walk.
    pub setup_us: u64,
    /// Visible CSR walk time, including the per-edge MVCC gate.
    pub traverse_us: u64,
    /// Endpoint ordering and multiplicity aggregation time.
    pub aggregate_us: u64,
}

impl PackedHandleCounts {
    pub fn total_edges(&self) -> u64 {
        self.counts.iter().sum()
    }
}

/// Direct and two-hop frontier degrees of one seed batch.
#[derive(Debug, Clone, Default)]
pub struct FrontierDegrees {
    pub direct: Vec<usize>,
    pub two_hop: Vec<usize>,
    /// Whether the dangling-visibility resolution was skipped under a proven
    /// dangling-free token. Query layers surface this as a plan mark.
    pub dangling_skipped: bool,
    /// Phase timings (microseconds) of the fused two-hop read: first-hop
    /// topology visit, endpoint ordering, conditional external resolution,
    /// tail-degree reads plus scatter. Portable fallbacks leave zeros.
    pub first_hop_us: u64,
    pub sort_us: u64,
    pub resolve_us: u64,
    pub tail_us: u64,
}

/// Portable existence gate built from topology-only edge reads and visible
/// endpoint reads. Native engines override this with one reverse-CSR pass.
pub fn portable_incoming_edge_exists_batch<S: StorageReader + ?Sized>(
    reader: &S,
    space: &str,
    dst_ids: &[VertexId],
    src_tag: &str,
    dst_tag: &str,
    edge_type: &str,
) -> Result<Vec<bool>, StorageError> {
    let edge_types = [edge_type.to_string()];
    let batches = reader.get_node_edges_projected_batch(
        space,
        dst_ids,
        EdgeDirection::In,
        &edge_types,
        Some(&[]),
    )?;
    let mut exists = Vec::with_capacity(dst_ids.len());
    for (dst_id, edges) in dst_ids.iter().zip(batches) {
        let mut found = false;
        for edge in edges {
            if edge.dst != *dst_id || edge.edge_type != edge_type {
                continue;
            }
            let Some(src) = reader.get_vertex(space, src_tag, &edge.src)? else {
                continue;
            };
            if src.tag.name != src_tag || reader.get_vertex(space, dst_tag, dst_id)?.is_none() {
                continue;
            }
            found = true;
            break;
        }
        exists.push(found);
    }
    Ok(exists)
}

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

    /// Resolve the edge-type schema used by packed topology reads.
    ///
    /// Callers cache the result for one query execution and pass it back to
    /// the batched reader, so concurrent partitions do not repeat catalog
    /// lookups for the same plan constant.
    fn resolve_topology_schema(
        &self,
        space: &str,
        edge_types: &[String],
    ) -> Result<Arc<TopologySchema>, StorageError> {
        TopologySchema::resolve(self, space, edge_types).map(Arc::new)
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

    /// Projected edge fanout for a batch of source vertices, in input order.
    ///
    /// The default is the exact per-vertex loop; the native engine resolves
    /// the edge-type schema once and reads each edge table once for the whole
    /// batch. Projection and TTL visibility match [`Self::get_node_edges_projected`].
    fn get_node_edges_projected_batch(
        &self,
        space: &str,
        node_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
        projection: Option<&[Arc<str>]>,
    ) -> Result<Vec<Vec<Edge>>, StorageError> {
        let mut out = Vec::with_capacity(node_ids.len());
        for id in node_ids {
            out.push(
                self.get_node_edges_projected(space, id, direction, edge_types, projection, None)?,
            );
        }
        Ok(out)
    }

    /// Topology-only edge fanout for a batch of source vertices.
    ///
    /// The default converts a topology-only projected read; the native
    /// engine reads the CSR records directly and never builds `Edge` values.
    fn get_node_edge_headers_batch(
        &self,
        space: &str,
        node_ids: &[VertexId],
        _handles: Option<&[Option<(u32, LabelId)>]>,
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<Vec<EdgeTopology>>, StorageError> {
        let batches =
            self.get_node_edges_projected_batch(space, node_ids, direction, edge_types, Some(&[]))?;
        Ok(batches
            .into_iter()
            .map(|edges| {
                edges
                    .into_iter()
                    .map(|edge| EdgeTopology {
                        src: *edge.src(),
                        dst: *edge.dst(),
                        edge_type: Arc::from(edge.edge_type()),
                        ranking: edge.ranking(),
                    })
                    .collect()
            })
            .collect())
    }

    /// Packed form of [`Self::get_node_edge_headers_batch`].
    ///
    /// Portable adapters flatten the per-parent batches; the native engine
    /// overrides this with a single CSR pass over the seed batch.
    fn get_packed_node_edge_headers_batch(
        &self,
        space: &str,
        node_ids: &[VertexId],
        handles: Option<&[Option<(u32, LabelId)>]>,
        direction: EdgeDirection,
        edge_types: &[String],
        _schema: &TopologySchema,
    ) -> Result<PackedEdgeTopology, StorageError> {
        let batches =
            self.get_node_edge_headers_batch(space, node_ids, handles, direction, edge_types)?;
        let mut packed = PackedEdgeTopology {
            parents: node_ids.to_vec(),
            ..PackedEdgeTopology::default()
        };
        packed.child_offsets.push(0);
        for batch in batches {
            for topology in batch {
                packed.src.push(topology.src);
                packed.dst.push(topology.dst);
                let type_id = packed.intern_edge_type(&topology.edge_type);
                packed.edge_type_ids.push(type_id);
                packed.rankings.push(topology.ranking);
            }
            packed.child_offsets.push(packed.dst.len());
        }
        Ok(packed)
    }

    /// Handle-level adjacency counts for a count-only packed expand.
    ///
    /// The native engine groups visible CSR neighbors by internal far
    /// endpoint handle and returns them sorted by handle. `None` means the
    /// shape is not served natively and the caller must use the full packed
    /// topology path.
    fn get_packed_handle_counts_batch(
        &self,
        space: &str,
        parents: &[VertexId],
        handles: Option<&[Option<(u32, LabelId)>]>,
        direction: EdgeDirection,
        edge_types: &[String],
        _schema: &TopologySchema,
    ) -> Result<Option<PackedHandleCounts>, StorageError> {
        let _ = (space, parents, handles, direction, edge_types);
        Ok(None)
    }

    /// Resolve external vertex ids from topology-carried internal handles.
    ///
    /// `None` signals that the reader cannot serve a handle-only lookup;
    /// callers then take the ordinary external-id path. A `Some` entry may
    /// still be `None` when the row is absent at the read snapshot.
    fn vertex_ids_by_handles_batch(
        &self,
        space: &str,
        tag: &str,
        handles: &[Option<(u32, LabelId)>],
    ) -> Result<Option<Vec<Option<VertexId>>>, StorageError> {
        let _ = (space, tag, handles);
        Ok(None)
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

    /// Batch vertex lookups decoding only the requested properties.
    ///
    /// Same ordering, timestamp and visibility semantics as
    /// [`get_vertices_batch`](Self::get_vertices_batch): one entry per input
    /// id in input order, `None` for missing vertices. An empty projection
    /// reads the full vertex, matching [`get_vertex_projected`](Self::get_vertex_projected).
    /// The default implementation loops over `get_vertex_projected` for
    /// adapters; the native engine overrides with a single-timestamp batched
    /// path. Query expand hops use this for destination bypass columns so a
    /// property projection never pays full-attribute decoding per chunk.
    fn get_vertices_projected_batch(
        &self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
        projection: &[std::sync::Arc<str>],
    ) -> Result<Vec<Option<Vertex>>, StorageError> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.get_vertex_projected(space, tag, id, projection)?);
        }
        Ok(out)
    }

    /// Batch property-column reads for destination bypass slots.
    ///
    /// Returns one aligned property vector per input id; `None` means the
    /// vertex is absent at the read snapshot. Missing properties inside a
    /// present vertex are `Value::Null`, matching the projected entity path.
    /// An empty property list performs an existence-only read.
    fn get_vertex_property_columns_batch(
        &self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
        properties: &[Arc<str>],
    ) -> Result<Vec<Option<Vec<Value>>>, StorageError> {
        let vertices = self.get_vertices_projected_batch(space, tag, ids, properties)?;
        Ok(vertices
            .into_iter()
            .map(|vertex| {
                vertex.map(|vertex| {
                    properties
                        .iter()
                        .map(|name| {
                            vertex
                                .property_value(name.as_ref())
                                .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                        })
                        .collect()
                })
            })
            .collect())
    }

    /// Property-column read that reuses storage row addresses captured by a
    /// topology read. Adapters ignore the hints and use the ordinary batch
    /// path; the native engine overrides this to skip external-id reverse
    /// lookup while keeping the same visibility and TTL checks.
    fn get_vertex_property_columns_with_handles_batch(
        &self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
        properties: &[Arc<str>],
        handles: &[Option<(u32, LabelId)>],
    ) -> Result<Vec<Option<Vec<Value>>>, StorageError> {
        let _ = handles;
        self.get_vertex_property_columns_batch(space, tag, ids, properties)
    }

    /// Whether any edge type reached by a topology fast path has TTL
    /// configured.
    ///
    /// Fast paths that read raw CSR adjacency (`neighbor_dst_ids_batch`,
    /// `out_degree_batch`, multi-hop intermediate hops, GetNeighbors source)
    /// never materialize edge properties, so they cannot evaluate TTL. Before
    /// taking such a path a caller checks this predicate and, when it is true,
    /// degrades to the materializing path so results match the "expired means
    /// absent" contract.
    ///
    /// `edge_types` empty means "every edge type in the space". Returns `true`
    /// when at least one of the relevant edge types has a valid TTL.
    fn edge_types_ttl_active(
        &self,
        space: &str,
        edge_types: &[String],
    ) -> Result<bool, StorageError> {
        let is_active = |info: &EdgeTypeInfo| {
            linkrs_core::types::ttl::has_valid_ttl(info.ttl_duration, info.ttl_col.as_deref())
        };
        if edge_types.is_empty() {
            Ok(self.list_edge_types(space)?.iter().any(is_active))
        } else {
            for name in edge_types {
                if let Some(info) = self.get_edge_type(space, name)? {
                    if is_active(&info) {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
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

    /// Sum the tail degrees of each source's neighbors in one batch.
    ///
    /// The result has one entry per source and counts every neighbor edge
    /// occurrence, so self-loops and parallel edges keep walk semantics.
    /// Adapters use the portable two-call form; the native engine overrides
    /// it with an internal-endpoint read that avoids external id round trips.
    fn neighbor_degree_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<usize>, StorageError> {
        Ok(self
            .frontier_degree_batch(space, src_ids, direction, edge_types)?
            .two_hop)
    }

    /// Direct and two-hop frontier degrees in one batch.
    ///
    /// The default composes the existing degree reads so adapters keep their
    /// portable semantics; the native engine stages the first hop once and
    /// derives both counts from the same adjacency pass.
    fn frontier_degree_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<FrontierDegrees, StorageError> {
        let direct = self.out_degree_batch(space, src_ids, direction, edge_types)?;
        let neighbors = self.neighbor_dst_ids_batch(space, src_ids, direction, edge_types)?;
        let lengths: Vec<usize> = neighbors.iter().map(Vec::len).collect();
        let flat: Vec<VertexId> = neighbors.into_iter().flatten().collect();
        let degrees = self.out_degree_batch(space, &flat, direction, edge_types)?;
        let mut two_hop = Vec::with_capacity(src_ids.len());
        let mut offset = 0;
        for length in lengths {
            let sum: usize = degrees[offset..offset + length].iter().sum();
            two_hop.push(sum);
            offset += length;
        }
        Ok(FrontierDegrees {
            direct,
            two_hop,
            dangling_skipped: false,
            first_hop_us: 0,
            sort_us: 0,
            resolve_us: 0,
            tail_us: 0,
        })
    }

    /// Open an index cursor that materializes at most `max_entries` per
    /// generation and resumes strictly after `start_after`. Engines without
    /// a paged native cursor report `not_supported`.
    fn open_ordered_index_cursor(
        &self,
        plan: &IndexScanPlan,
        start_after: Option<Vec<u8>>,
        max_entries: usize,
    ) -> Result<Box<dyn IndexCursor<Row = IndexRow>>, StorageError> {
        let _ = (plan, start_after, max_entries);
        Err(StorageError::not_supported(
            "Paged index cursors are not supported by this storage implementation",
        ))
    }

    /// Whether `(space, edge_type)` is proven to have no dangling endpoints.
    ///
    /// Inserts validate endpoints, so proof only needs the absence of
    /// un-repaired plain vertex deletes. The default reports unproven so
    /// adapters keep their existing visibility checks; the native engine
    /// overrides it with its delete/repair-maintained token.
    fn edge_type_dangling_free(&self, space: &str, edge_type: &str) -> bool {
        let _ = (space, edge_type);
        false
    }

    /// Whether the calling context is a single auto-commit statement (or
    /// absent). Skipping existence re-reads is only sound there: the token
    /// is revoked at delete-statement time, which always precedes the
    /// delete's commit, so a same-process check cannot miss a visible
    /// delete. Explicit multi-statement transactions keep the full
    /// recording path.
    fn allows_dangling_skip(&self) -> bool {
        false
    }

    /// Whether one ordered index key can back an ascending early-stop scan.
    ///
    /// Only engines that can prove global key order across generations and
    /// shards return a proof. The query layer still has to prove that the key
    /// is the destination's unique primary key.
    fn ordered_index_proof(
        &self,
        space: &str,
        tag: &str,
        property: &str,
    ) -> Result<Option<OrderedIndexProof>, StorageError> {
        let _ = (space, tag, property);
        Ok(None)
    }

    /// Whether a visible incoming edge exists for each destination id, in
    /// input order. The result only reports existence: parallel edges and
    /// duplicates collapse into one boolean per input.
    ///
    /// Implementations must apply the same vertex, edge, TTL and staged-write
    /// visibility rules as a full in-direction expansion.
    fn incoming_edge_exists_batch(
        &self,
        space: &str,
        dst_ids: &[VertexId],
        src_tag: &str,
        dst_tag: &str,
        edge_type: &str,
    ) -> Result<Vec<bool>, StorageError> {
        portable_incoming_edge_exists_batch(self, space, dst_ids, src_tag, dst_tag, edge_type)
    }

    fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError>;
    /// Table-sharded parallel variant of [`Self::scan_edges_by_type`]: the
    /// type's edge tables fan out across `shards` rayon shards sharing one
    /// read timestamp, concatenated in key order. Single-table types fall
    /// back to the serial scan internally. The default body is the serial
    /// scan, so adapters and test doubles inherit it unchanged.
    fn scan_edges_by_type_parallel(
        &self,
        space: &str,
        edge_type: &str,
        shards: usize,
    ) -> Result<Vec<Edge>, StorageError> {
        let _ = shards;
        self.scan_edges_by_type(space, edge_type)
    }
    fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
    fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError>;
    fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError>;
    /// Physical edge total for one edge type, summed over the stored legs.
    ///
    /// Sizing only (cardinality estimates, partition width): it ignores
    /// visibility, so snapshot reads must keep using `count_edges_by_type`
    /// or `count_visible_edges`. The default body falls back to the visible
    /// counter so adapters and test doubles inherit correct behavior
    /// unchanged, at the cost of a full-leg walk.
    fn approximate_edge_count(&self, space: &str, edge_type: &str) -> Result<u64, StorageError> {
        self.count_edges_by_type(space, edge_type)
    }
    /// Snapshot-exact visible-vertex count for pure `COUNT` queries: equals
    /// a full tag scan under the same read timestamp. `None` sums every
    /// tag of the space; missing tags contribute zero.
    fn count_visible_vertices(&self, space: &str, tag: Option<&str>) -> Result<u64, StorageError>;
    /// Snapshot-exact visible-edge count for pure `COUNT` queries: equals
    /// the seed-driven degree sum over fully scanned endpoints, excluding
    /// dangling seed-side endpoints and ignoring far-side existence.
    /// Only `Out` and `In` are served; `Both` is refused loudly.
    fn count_visible_edges(
        &self,
        space: &str,
        edge_type: &str,
        direction: EdgeDirection,
    ) -> Result<u64, StorageError>;

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

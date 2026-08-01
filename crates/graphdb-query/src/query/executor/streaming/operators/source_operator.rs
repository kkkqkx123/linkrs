use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use parking_lot::RwLock;

use super::super::state::GlobalState;
use super::spec::{BoundIndexPredicate, IndexProjection};
use super::state::SourceState;
use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::types::MAX_TIMESTAMP;
use crate::core::wal::EntityRef;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::base::{MemoryBudget, MemoryReservation};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use crate::storage::{
    open_edge_scan, open_index_cursor, open_vertex_scan, EdgeCursor, IndexCursor, IndexPredicate,
    IndexRow, IndexScanPlan, RequiredProperty, ScanOptions, VecEdgeCursor, VertexCursor,
};

#[derive(Debug, Default)]
pub enum NeighborScanState {
    #[default]
    Init,
    Collecting {
        vertex_ids: Vec<VertexId>,
        position: usize,
        direction: EdgeDirection,
        seen: HashSet<VertexId>,
        neighbor_ids: Vec<VertexId>,
    },
    Fetching {
        neighbor_ids: Vec<VertexId>,
        position: usize,
    },
    Done,
}

fn make_vertex_row(vertex: crate::core::vertex_edge_path::Vertex) -> Vec<Value> {
    vec![Value::Vertex(Box::new(vertex))]
}

fn make_covering_vertex_row(
    entity_ref: &EntityRef,
    columns: Vec<(String, Value)>,
) -> Option<Vec<Value>> {
    let vertex_id = entity_ref_to_vertex_id(entity_ref)?;
    let properties = columns.into_iter().collect();
    Some(make_vertex_row(
        crate::core::vertex_edge_path::Vertex::new_with_properties(
            vertex_id,
            Vec::new(),
            properties,
        ),
    ))
}

fn make_edge_row(edge: crate::core::vertex_edge_path::Edge) -> Vec<Value> {
    vec![Value::Edge(Box::new(edge))]
}

fn storage_error(
    source: &str,
    operation: &str,
    space_name: &str,
    error: impl std::fmt::Display,
) -> QueryError {
    QueryError::execution(format!(
        "{} {} failed for space '{}': {}",
        source, operation, space_name, error
    ))
}

/// Source operator with arena-based state for counters.
///
/// Heavy mutable resources (cursors) are kept inline for practical lifetime
/// management; simple counters and state machines live in the `SourceState`
/// arena on [`OperatorBase`].
#[derive(Debug)]
pub enum SourceOperator {
    /// Buffered vertex scan — rows come from the spec.
    ScanVertices {
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    /// Storage-backed vertex scan — rows come from a storage cursor.
    StorageScanVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        limit: Option<usize>,
        partition_range: Option<std::ops::Range<i64>>,
        col_names: Vec<String>,
        /// Optional property names to project. When non-empty, only these
        /// properties are retained in the loaded Vertex objects, reducing
        /// memory and boxing overhead.
        projected_properties: Vec<String>,
        /// Cursor kept inline for practical lifetime management.
        cursor: Option<Box<dyn VertexCursor>>,
    },
    /// Buffered edge scan — rows come from the spec.
    ScanEdges {
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    /// Storage-backed edge scan — rows come from a storage cursor.
    StorageScanEdges {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        limit: Option<usize>,
        edge_type: Option<String>,
        partition_range: Option<std::ops::Range<i64>>,
        col_names: Vec<String>,
        projected_properties: Vec<String>,
        /// Cursor kept inline for practical lifetime management.
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    /// Fetch vertices by ID.
    GetVertices {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        /// Pre-parsed VertexId values for fast lookup (avoids re-parsing per batch).
        cached_ids: Vec<VertexId>,
        projected_properties: Vec<String>,
    },
    /// Fetch edges by src/dst/type/rank.
    GetEdges {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        /// Cursor kept inline for practical lifetime management.
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    /// Traverse neighbors of each input vertex.
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        direction: String,
        projected_properties: Vec<String>,
        /// State kept inline (complex state machine with owned data).
        state: NeighborScanState,
    },
    /// Scan edges via index.
    EdgeIndexScan {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_type: Option<String>,
        index_name: String,
        predicate: Option<BoundIndexPredicate>,
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    /// Index scan with typed predicate and projection.
    IndexScan {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        index_name: String,
        index_id: u64,
        predicate: BoundIndexPredicate,
        projection: IndexProjection,
        output_layout: Arc<SlotLayout>,
        partition_range: Option<Range<i64>>,
        cursor: Option<Box<dyn IndexCursor<Row = IndexRow>>>,
    },
    Argument,
    /// Property retrieval (zero-input source, will migrate to Unary in M2).
    GetProp {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        entity_slot: usize,
        prop_names: Vec<String>,
        is_vertex: bool,
        output_layout: Arc<SlotLayout>,
    },
    /// Alias for IndexScan (same semantics, kept for transitional compat).
    LookupIndex {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        index_name: String,
        index_id: u64,
        predicate: BoundIndexPredicate,
        projection: IndexProjection,
        output_layout: Arc<SlotLayout>,
        partition_range: Option<Range<i64>>,
        cursor: Option<Box<dyn IndexCursor<Row = IndexRow>>>,
    },
    Start,
}

impl SourceOperator {
    /// Create a SourceOperator with immutable config from an immutable spec
    /// and the per-query storage client.  Mutable runtime state is created
    /// separately in [`SourceOperator::open`] and stored in the operator
    /// state arena on [`OperatorBase`].
    pub fn from_spec(
        spec: &super::spec::SourceSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
    ) -> Self {
        match spec {
            super::spec::SourceSpec::ScanVertices { rows, col_names } => Self::ScanVertices {
                buffer: rows.clone(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            super::spec::SourceSpec::StorageScanVertices {
                space_name,
                limit,
                col_names,
                projected_properties,
            } => Self::StorageScanVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                partition_range: None,
                col_names: col_names.clone(),
                projected_properties: projected_properties.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::ScanEdges { rows, col_names } => Self::ScanEdges {
                buffer: rows.clone(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            super::spec::SourceSpec::StorageScanEdges {
                space_name,
                limit,
                edge_type,
                col_names,
                projected_properties,
            } => Self::StorageScanEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                edge_type: edge_type.clone(),
                partition_range: None,
                col_names: col_names.clone(),
                projected_properties: projected_properties.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::GetVertices {
                space_name,
                vertex_ids,
                projected_properties,
            } => Self::GetVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                vertex_ids: vertex_ids.clone(),
                cached_ids: Vec::new(),
                projected_properties: projected_properties.clone(),
            },
            super::spec::SourceSpec::GetEdges {
                space_name,
                edge_type,
                src,
                dst,
                rank,
            } => Self::GetEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_type: edge_type.clone(),
                src: src.clone(),
                dst: dst.clone(),
                rank: *rank,
                cursor: None,
            },
            super::spec::SourceSpec::GetNeighbors {
                space_name,
                direction,
                projected_properties,
            } => Self::GetNeighbors {
                storage: storage.clone(),
                space_name: space_name.clone(),
                direction: direction.clone(),
                projected_properties: projected_properties.clone(),
                state: NeighborScanState::Init,
            },
            super::spec::SourceSpec::EdgeIndexScan {
                space_name,
                edge_type,
                index_name,
                predicate,
            } => Self::EdgeIndexScan {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_type: edge_type.clone(),
                index_name: index_name.clone(),
                predicate: predicate.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::IndexScan {
                space_name,
                index_name,
                index_id,
                predicate,
                projection,
                output_layout,
                ..
            } => Self::IndexScan {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                index_id: *index_id,
                predicate: predicate.clone(),
                projection: projection.clone(),
                output_layout: output_layout.clone(),
                partition_range: None,
                cursor: None,
            },
            super::spec::SourceSpec::Argument => Self::Argument,
            super::spec::SourceSpec::GetProp {
                space_name,
                entity_slot,
                prop_names,
                is_vertex,
                output_layout,
            } => Self::GetProp {
                storage: storage.clone(),
                space_name: space_name.clone(),
                entity_slot: *entity_slot,
                prop_names: prop_names.clone(),
                is_vertex: *is_vertex,
                output_layout: output_layout.clone(),
            },
            super::spec::SourceSpec::LookupIndex {
                space_name,
                index_name,
                index_id,
                predicate,
                projection,
                output_layout,
                ..
            } => Self::LookupIndex {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                index_id: *index_id,
                predicate: predicate.clone(),
                projection: projection.clone(),
                output_layout: output_layout.clone(),
                partition_range: None,
                cursor: None,
            },
            super::spec::SourceSpec::Start => Self::Start,
        }
    }

    pub fn open(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        match self {
            Self::ScanVertices {
                current_index,
                col_names,
                ..
            } => {
                *current_index = 0;
                base.insert_state(GlobalState::Source(SourceState::ScanVertices {
                    current_index: 0,
                    col_names: col_names.clone(),
                }));
            }
            Self::ScanEdges {
                current_index,
                col_names,
                ..
            } => {
                *current_index = 0;
                base.insert_state(GlobalState::Source(SourceState::ScanEdges {
                    current_index: 0,
                    col_names: col_names.clone(),
                }));
            }
            Self::StorageScanVertices {
                storage,
                space_name,
                limit,
                partition_range,
                col_names,
                projected_properties,
                cursor,
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("StorageScanVertices requires storage".to_string())
                })?;
                *cursor = Some(
                    open_vertex_scan(
                        storage_ref,
                        space_name,
                        &ScanOptions {
                            limit: *limit,
                            vertex_id_range: partition_range.clone(),
                            projection: (!projected_properties.is_empty()).then(|| {
                                projected_properties
                                    .iter()
                                    .map(|n| RequiredProperty::new(n.clone()))
                                    .collect()
                            }),
                            ..ScanOptions::default()
                        },
                    )
                    .map_err(|error| {
                        storage_error("StorageScanVertices", "open cursor", space_name, error)
                    })?,
                );
                base.insert_state(GlobalState::Source(SourceState::StorageScanVertices {
                    partition_id: base.partition_id.unwrap_or(0),
                    partition_range: partition_range.clone(),
                    cursor: None,
                    buffer: Vec::new(),
                    current_index: 0,
                    col_names: col_names.clone(),
                }));
            }
            Self::StorageScanEdges {
                storage,
                space_name,
                limit,
                edge_type,
                partition_range,
                col_names,
                projected_properties,
                cursor,
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("StorageScanEdges requires storage".to_string())
                })?;
                *cursor = Some(
                    open_edge_scan(
                        storage_ref,
                        space_name,
                        &ScanOptions {
                            limit: *limit,
                            edge_type: edge_type.clone(),
                            edge_src_id_range: partition_range.clone(),
                            projection: (!projected_properties.is_empty()).then(|| {
                                projected_properties
                                    .iter()
                                    .map(|n| RequiredProperty::new(n.clone()))
                                    .collect()
                            }),
                            ..ScanOptions::default()
                        },
                    )
                    .map_err(|error| {
                        storage_error("StorageScanEdges", "open cursor", space_name, error)
                    })?,
                );
                base.insert_state(GlobalState::Source(SourceState::StorageScanEdges {
                    partition_id: base.partition_id.unwrap_or(0),
                    partition_range: partition_range.clone(),
                    cursor: None,
                    buffer: Vec::new(),
                    current_index: 0,
                    col_names: col_names.clone(),
                }));
            }
            Self::GetVertices {
                vertex_ids,
                cached_ids,
                ..
            } => {
                if let Some(ids) = vertex_ids.as_ref() {
                    cached_ids.clear();
                    cached_ids.reserve(ids.len());
                    for id_val in ids {
                        if let Ok(vid) = VertexId::try_from(id_val) {
                            cached_ids.push(vid);
                        }
                    }
                }
                base.insert_state(GlobalState::Source(SourceState::GetVertices {
                    position: 0,
                }));
            }
            Self::GetEdges {
                storage,
                space_name,
                edge_type,
                src,
                dst,
                rank,
                cursor,
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("GetEdges requires storage".to_string())
                })?;
                let guard = storage_ref.read();
                let edges = if let (Some(src), Some(dst), Some(edge_type)) =
                    (src.as_deref(), dst.as_deref(), edge_type.as_deref())
                {
                    let src = parse_vertex_id(src);
                    let dst = parse_vertex_id(dst);
                    guard
                        .get_edge(space_name, &src, &dst, edge_type, *rank)
                        .map_err(|error| storage_error("GetEdges", "get edge", space_name, error))?
                        .into_iter()
                        .collect::<Vec<_>>()
                } else {
                    let scan_opts = ScanOptions {
                        edge_type: edge_type.clone(),
                        ..ScanOptions::default()
                    };
                    drop(guard);
                    let scan_cursor =
                        open_edge_scan(storage_ref, space_name, &scan_opts).map_err(|error| {
                            storage_error("GetEdges", "open cursor", space_name, error)
                        })?;
                    *cursor = Some(scan_cursor);
                    Vec::new()
                };
                if !edges.is_empty() {
                    *cursor = Some(Box::new(VecEdgeCursor::new(edges)));
                }
                base.insert_state(GlobalState::Source(SourceState::GetEdges { cursor: None }));
            }
            Self::GetNeighbors { state, .. } => {
                *state = NeighborScanState::Init;
                base.insert_state(GlobalState::Source(SourceState::GetNeighbors {
                    state: NeighborScanState::Init,
                }));
            }
            Self::EdgeIndexScan {
                storage,
                space_name,
                edge_type,
                index_name,
                predicate,
                cursor,
                ..
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("EdgeIndexScan requires storage".to_string())
                })?;
                let guard = storage_ref.read();
                let edges = lookup_edges_via_property_index(
                    &*guard,
                    space_name,
                    edge_type.as_deref(),
                    index_name,
                    predicate.as_ref(),
                )
                .map_err(|error| {
                    storage_error("EdgeIndexScan", "open cursor", space_name, error)
                })?;
                drop(guard);
                *cursor = Some(Box::new(VecEdgeCursor::new(edges)));
                base.insert_state(GlobalState::Source(SourceState::EdgeIndexScan {
                    cursor: None,
                }));
            }
            Self::IndexScan {
                storage,
                space_name,
                index_id,
                predicate,
                projection,
                partition_range,
                cursor,
                ..
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("IndexScan requires storage".to_string())
                })?;
                let plan = build_index_scan_plan(
                    storage_ref,
                    space_name,
                    *index_id,
                    predicate,
                    projection,
                    partition_range.clone(),
                )?;
                *cursor = Some(open_index_cursor(storage_ref, &plan).map_err(|error| {
                    storage_error("IndexScan", "open cursor", space_name, error)
                })?);
                base.insert_state(GlobalState::Source(SourceState::IndexScan { cursor: None }));
            }
            Self::Argument => {
                base.insert_state(GlobalState::Source(SourceState::Argument));
            }
            Self::GetProp {
                entity_slot,
                prop_names,
                ..
            } => {
                base.insert_state(GlobalState::Source(SourceState::GetProp {
                    entity_slot: *entity_slot,
                    prop_names: prop_names.clone(),
                }));
            }
            Self::LookupIndex {
                storage,
                space_name,
                index_id,
                predicate,
                projection,
                partition_range,
                cursor,
                ..
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("LookupIndex requires storage".to_string())
                })?;
                let plan = build_index_scan_plan(
                    storage_ref,
                    space_name,
                    *index_id,
                    predicate,
                    projection,
                    partition_range.clone(),
                )?;
                *cursor = Some(open_index_cursor(storage_ref, &plan).map_err(|error| {
                    storage_error("LookupIndex", "open cursor", space_name, error)
                })?);
                base.insert_state(GlobalState::Source(SourceState::LookupIndex {
                    cursor: None,
                }));
            }
            Self::Start => {
                base.insert_state(GlobalState::Source(SourceState::Start { emitted: false }));
            }
        }
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(&mut self, base: &mut OperatorBase) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::ScanVertices {
                buffer,
                current_index,
                col_names,
                ..
            }
            | Self::ScanEdges {
                buffer,
                current_index,
                col_names,
                ..
            } => next_buffer_chunk(base, buffer, current_index, col_names),
            Self::StorageScanVertices {
                space_name, cursor, ..
            } => loop {
                base.ensure_not_cancelled()?;
                let mut cur = match cursor.take() {
                    Some(c) => c,
                    None => return Ok(None),
                };
                let batch = cur.next_batch(base.chunk_size).map_err(|error| {
                    storage_error("StorageScanVertices", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_vertex_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk =
                        DataChunk::new_with_layout(rows, Arc::clone(&base.output_layout));
                    chunk.materialize_columns();
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    *cursor = Some(cur);
                    return Ok(Some(chunk));
                }
                *cursor = Some(cur);
            },
            Self::StorageScanEdges {
                space_name, cursor, ..
            } => loop {
                base.ensure_not_cancelled()?;
                let mut cur = match cursor.take() {
                    Some(c) => c,
                    None => return Ok(None),
                };
                let batch = cur.next_batch(base.chunk_size).map_err(|error| {
                    storage_error("StorageScanEdges", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_edge_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk =
                        DataChunk::new_with_layout(rows, Arc::clone(&base.output_layout));
                    chunk.materialize_columns();
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    *cursor = Some(cur);
                    return Ok(Some(chunk));
                }
                *cursor = Some(cur);
            },
            Self::GetVertices {
                storage,
                space_name,
                vertex_ids,
                cached_ids,
                projected_properties,
            } => {
                // Fast path: single-ID lookup without batch/position machinery.
                if let Some(ids) = vertex_ids.as_ref() {
                    if ids.len() == 1 {
                        // Check if already returned (state position >= ids.len()).
                        {
                            let mut arena = base.state_arena();
                            if let Some(GlobalState::Source(SourceState::GetVertices {
                                position,
                            })) = arena.global.get_mut(&base.state_key())
                            {
                                if *position >= ids.len() {
                                    return Ok(None);
                                }
                            }
                        }
                        let storage_ref = storage.as_ref().ok_or_else(|| {
                            QueryError::execution("GetVertices requires storage".to_string())
                        })?;
                        let guard = storage_ref.read();
                        let vid = match cached_ids.first() {
                            Some(vid) => *vid,
                            None => VertexId::try_from(&ids[0]).unwrap_or_default(),
                        };
                        let vertex_opt = if projected_properties.is_empty() {
                            guard.get_vertex(space_name, &vid).map_err(|error| {
                                storage_error("GetVertices", "get vertex", space_name, error)
                            })?
                        } else {
                            guard
                                .get_vertex_projected(space_name, &vid, projected_properties)
                                .map_err(|error| {
                                    storage_error("GetVertices", "get vertex", space_name, error)
                                })?
                        };
                        // Mark position as done so subsequent calls return None.
                        let mark_done = |base: &mut OperatorBase| {
                            let mut arena = base.state_arena();
                            if let Some(GlobalState::Source(SourceState::GetVertices {
                                position,
                            })) = arena.global.get_mut(&base.state_key())
                            {
                                *position = ids.len();
                            }
                        };
                        if let Some(vertex) = vertex_opt {
                            let rows = vec![make_vertex_row(vertex)];
                            let reservation = reserve_memory(base, &rows)?;
                            let mut chunk =
                                DataChunk::new_with_layout(rows, base.output_layout.clone());
                            chunk.materialize_columns();
                            if let Some(r) = reservation {
                                chunk = chunk.with_memory_reservation(r);
                            }
                            mark_done(base);
                            return Ok(Some(chunk));
                        }
                        mark_done(base);
                        return Ok(None);
                    }
                }
                loop {
                    let storage_ref = storage.as_ref().ok_or_else(|| {
                        QueryError::execution("GetVertices requires storage".to_string())
                    })?;
                    let guard = storage_ref.read();
                    let ids = vertex_ids.as_ref().ok_or_else(|| {
                        QueryError::execution("GetVertices requires vertex IDs".to_string())
                    })?;
                    let (position, done) = {
                        let mut arena = base.state_arena();
                        let s = arena.global.get_mut(&base.state_key()).unwrap();
                        let GlobalState::Source(SourceState::GetVertices { position }) = s else {
                            return Ok(None);
                        };
                        if *position >= ids.len() {
                            (0, true)
                        } else {
                            let end = (*position + base.chunk_size).min(ids.len());
                            *position = end;
                            (end, false)
                        }
                    };
                    if done {
                        return Ok(None);
                    }
                    let start = position.saturating_sub(base.chunk_size);
                    let mut rows = Vec::with_capacity(position - start);
                    if projected_properties.is_empty() {
                        for vid in &cached_ids[start..position] {
                            if let Some(vertex) =
                                guard.get_vertex(space_name, vid).map_err(|error| {
                                    storage_error("GetVertices", "get vertex", space_name, error)
                                })?
                            {
                                rows.push(make_vertex_row(vertex));
                            }
                        }
                    } else {
                        for vid in &cached_ids[start..position] {
                            if let Some(vertex) = guard
                                .get_vertex_projected(space_name, vid, projected_properties)
                                .map_err(|error| {
                                    storage_error("GetVertices", "get vertex", space_name, error)
                                })?
                            {
                                rows.push(make_vertex_row(vertex));
                            }
                        }
                    }
                    if !rows.is_empty() {
                        let reservation = reserve_memory(base, &rows)?;
                        let mut chunk =
                            DataChunk::new_with_layout(rows, base.output_layout.clone());
                        chunk.materialize_columns();
                        if let Some(r) = reservation {
                            chunk = chunk.with_memory_reservation(r);
                        }
                        return Ok(Some(chunk));
                    }
                }
            }
            Self::GetEdges {
                space_name, cursor, ..
            } => {
                let mut cur = match cursor.take() {
                    Some(c) => c,
                    None => return Ok(None),
                };
                let batch = cur
                    .next_batch(base.chunk_size)
                    .map_err(|error| storage_error("GetEdges", "read cursor", space_name, error))?;
                if batch.is_empty() {
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_edge_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk = DataChunk::new_with_layout(rows, base.output_layout.clone());
                    chunk.materialize_columns();
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    *cursor = Some(cur);
                    return Ok(Some(chunk));
                }
                *cursor = Some(cur);
                Ok(None)
            }
            Self::GetNeighbors {
                storage,
                space_name,
                direction,
                projected_properties,
                state,
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("GetNeighbors requires storage".to_string())
                })?;

                loop {
                    base.ensure_not_cancelled()?;
                    match state {
                        NeighborScanState::Init => {
                            let dir: EdgeDirection = direction.as_str().into();
                            let guard = storage_ref.read();
                            let vertices = guard.scan_vertices(space_name).map_err(|error| {
                                storage_error("GetNeighbors", "scan vertices", space_name, error)
                            })?;
                            let ids: Vec<VertexId> = vertices.into_iter().map(|v| v.vid).collect();
                            drop(guard);

                            *state = NeighborScanState::Collecting {
                                vertex_ids: ids,
                                position: 0,
                                direction: dir,
                                seen: HashSet::new(),
                                neighbor_ids: Vec::new(),
                            };
                        }
                        NeighborScanState::Collecting {
                            vertex_ids,
                            position,
                            direction,
                            seen,
                            neighbor_ids,
                        } => {
                            if *position >= vertex_ids.len() {
                                if neighbor_ids.is_empty() {
                                    *state = NeighborScanState::Done;
                                    return Ok(None);
                                }
                                let nids = std::mem::take(neighbor_ids);
                                *state = NeighborScanState::Fetching {
                                    neighbor_ids: nids,
                                    position: 0,
                                };
                                continue;
                            }
                            let end = (*position + base.chunk_size).min(vertex_ids.len());
                            let guard = storage_ref.read();
                            for vid in &vertex_ids[*position..end] {
                                let edges = guard
                                    .get_node_edges(space_name, vid, *direction)
                                    .map_err(|error| {
                                        storage_error(
                                            "GetNeighbors",
                                            "get node edges",
                                            space_name,
                                            error,
                                        )
                                    })?;
                                for edge in edges {
                                    let nid = match direction {
                                        EdgeDirection::Out => *edge.dst(),
                                        EdgeDirection::In => *edge.src(),
                                        EdgeDirection::Both => {
                                            if edge.src() == vid {
                                                *edge.dst()
                                            } else {
                                                *edge.src()
                                            }
                                        }
                                    };
                                    if seen.insert(nid) {
                                        neighbor_ids.push(nid);
                                    }
                                }
                            }
                            drop(guard);
                            *position = end;
                        }
                        NeighborScanState::Fetching {
                            neighbor_ids,
                            position,
                        } => {
                            if *position >= neighbor_ids.len() {
                                *state = NeighborScanState::Done;
                                return Ok(None);
                            }
                            let end = (*position + base.chunk_size).min(neighbor_ids.len());
                            let guard = storage_ref.read();
                            let batch_size = end - *position;
                            let mut rows = Vec::with_capacity(batch_size);
                            if projected_properties.is_empty() {
                                for neighbor_id in &neighbor_ids[*position..end] {
                                    if let Some(vertex) = guard
                                        .get_vertex(space_name, neighbor_id)
                                        .map_err(|error| {
                                            storage_error(
                                                "GetNeighbors",
                                                "get neighbor vertex",
                                                space_name,
                                                error,
                                            )
                                        })?
                                    {
                                        rows.push(make_vertex_row(vertex));
                                    }
                                }
                            } else {
                                for neighbor_id in &neighbor_ids[*position..end] {
                                    if let Some(vertex) = guard
                                        .get_vertex_projected(
                                            space_name,
                                            neighbor_id,
                                            projected_properties,
                                        )
                                        .map_err(|error| {
                                            storage_error(
                                                "GetNeighbors",
                                                "get neighbor vertex",
                                                space_name,
                                                error,
                                            )
                                        })?
                                    {
                                        rows.push(make_vertex_row(vertex));
                                    }
                                }
                            }
                            drop(guard);
                            *position = end;
                            if !rows.is_empty() {
                                let reservation = reserve_memory(base, &rows)?;
                                let mut chunk = DataChunk::new_with_layout(
                                    rows,
                                    Arc::clone(&base.output_layout),
                                );
                                chunk.materialize_columns();
                                if let Some(r) = reservation {
                                    chunk = chunk.with_memory_reservation(r);
                                }
                                return Ok(Some(chunk));
                            }
                        }
                        NeighborScanState::Done => return Ok(None),
                    }
                }
            }
            Self::GetProp { .. } => {
                // GetProp is not yet implemented as a source operator.
                // The M1 plan specifies it should be a Unary operator that
                // reads entity IDs from its input child.
                // Until the unary migration is complete (M2), this variant
                // returns a capability-unavailable error.
                Err(QueryError::execution(
                    "GetProp is not available as a source operator; \
                     use the unary GetProp (coming in M2)"
                        .to_string(),
                ))
            }
            Self::LookupIndex {
                storage,
                space_name,
                output_layout,
                projection,
                cursor,
                ..
            } => {
                let vertex_projection = match projection {
                    IndexProjection::Columns(cols) => cols.clone(),
                    _ => Vec::new(),
                };
                next_index_chunk(
                    storage.as_ref().ok_or_else(|| {
                        QueryError::execution("LookupIndex requires storage".to_string())
                    })?,
                    space_name,
                    cursor,
                    output_layout,
                    base,
                    "LookupIndex",
                    &vertex_projection,
                )
            }
            Self::EdgeIndexScan {
                space_name, cursor, ..
            } => {
                let mut cur = match cursor.take() {
                    Some(c) => c,
                    None => return Ok(None),
                };
                let batch = cur.next_batch(base.chunk_size).map_err(|error| {
                    storage_error("EdgeIndexScan", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_edge_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk = DataChunk::new_with_layout(rows, base.output_layout.clone());
                    chunk.materialize_columns();
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    *cursor = Some(cur);
                    return Ok(Some(chunk));
                }
                *cursor = Some(cur);
                Ok(None)
            }
            Self::IndexScan {
                storage,
                space_name,
                output_layout,
                projection,
                cursor,
                ..
            } => {
                let vertex_projection = match projection {
                    IndexProjection::Columns(cols) => cols.clone(),
                    _ => Vec::new(),
                };
                next_index_chunk(
                    storage.as_ref().ok_or_else(|| {
                        QueryError::execution("IndexScan requires storage".to_string())
                    })?,
                    space_name,
                    cursor,
                    output_layout,
                    base,
                    "IndexScan",
                    &vertex_projection,
                )
            }
            Self::Start => {
                let mut arena = base.state_arena();
                let s = arena.global.get_mut(&base.state_key());
                let emitted = match s {
                    Some(GlobalState::Source(SourceState::Start { ref mut emitted })) => emitted,
                    _ => return Ok(None),
                };
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let chunk =
                    DataChunk::new_with_layout(vec![Vec::new()], Arc::clone(&base.output_layout));
                Ok(Some(chunk))
            }
            Self::Argument => {
                let rt = base.runtime.as_ref().ok_or_else(|| {
                    QueryError::execution("Argument requires a runtime with correlation frame")
                })?;
                let frame = rt.take_correlation_frame();
                match frame {
                    Some((_layout, row)) => {
                        let chunk =
                            DataChunk::new_with_layout(vec![row], Arc::clone(&base.output_layout));

                        Ok(Some(chunk))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    pub fn stop(&mut self, _base: &mut OperatorBase) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        // Drop the arena state — this releases any cursors, buffers, etc.
        base.take_state();
        base.lifecycle.mark_closed();
        Ok(())
    }
}

/// Resolve an edge index definition to its indexed property and execute a
/// value-range lookup via the per-table edge property index.
fn lookup_edges_via_property_index(
    storage: &dyn QueryStorage,
    space_name: &str,
    edge_type: Option<&str>,
    index_name: &str,
    predicate: Option<&BoundIndexPredicate>,
) -> Result<Vec<crate::core::Edge>, QueryError> {
    let edge_type = edge_type
        .ok_or_else(|| QueryError::execution("EdgeIndexScan requires an edge type".to_string()))?;
    let index = storage
        .get_edge_index(space_name, index_name)
        .map_err(|error| {
            QueryError::execution(format!(
                "EdgeIndexScan: failed to load index {} in space {}: {}",
                index_name, space_name, error
            ))
        })?
        .ok_or_else(|| {
            QueryError::execution(format!(
                "EdgeIndexScan: edge index {} not found in space {}",
                index_name, space_name
            ))
        })?;
    let prop_name = index
        .fields
        .first()
        .map(|field| field.name.clone())
        .ok_or_else(|| {
            QueryError::execution(format!(
                "EdgeIndexScan: index {} has no indexed fields",
                index_name
            ))
        })?;
    let (lower, upper, include_lower, include_upper) = match predicate {
        Some(BoundIndexPredicate::Equal { value, .. }) => {
            (Some(value.clone()), Some(value.clone()), true, false)
        }
        Some(BoundIndexPredicate::Range {
            begin,
            end,
            include_begin,
            include_end,
            ..
        }) => (begin.clone(), end.clone(), *include_begin, *include_end),
        Some(BoundIndexPredicate::Prefix { prefix, .. }) => {
            (Some(prefix.clone()), Some(prefix.clone()), true, false)
        }
        _ => (None, None, true, false),
    };
    storage
        .lookup_edges_by_property_range(
            space_name,
            edge_type,
            &prop_name,
            lower.as_ref(),
            upper.as_ref(),
            include_lower,
            include_upper,
        )
        .map_err(|error| {
            QueryError::execution(format!(
                "EdgeIndexScan: property range lookup failed in space {}: {}",
                space_name, error
            ))
        })
}

fn build_index_scan_plan(
    storage: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    index_id: u64,
    predicate: &BoundIndexPredicate,
    projection: &IndexProjection,
    partition_range: Option<Range<i64>>,
) -> Result<IndexScanPlan, QueryError> {
    let physical_predicate = match predicate {
        BoundIndexPredicate::Equal { value, .. } => IndexPredicate::Equal(value.clone()),
        BoundIndexPredicate::Range {
            begin,
            end,
            include_begin,
            include_end,
            ..
        } => IndexPredicate::Range {
            lower: begin.clone(),
            upper: end.clone(),
            include_lower: *include_begin,
            include_upper: *include_end,
        },
        BoundIndexPredicate::Prefix { prefix, .. } => IndexPredicate::Prefix(prefix.clone()),
        BoundIndexPredicate::Full => IndexPredicate::All,
    };

    let projection = match projection {
        IndexProjection::RowIdOnly => None,
        IndexProjection::Columns(columns) => Some(columns.clone()),
        IndexProjection::AllColumns => Some(Vec::new()),
    };
    let read_timestamp = storage
        .read()
        .operation_context()
        .map(|context| context.read_timestamp)
        .unwrap_or(MAX_TIMESTAMP);

    // A manifest shard is bounded by complete native-index keys, including
    // space and index prefixes. The storage derives complete key bounds from
    // the predicate and intersects them with the manifest. Partition ranges
    // (i64 vertex/edge ID ranges) are forwarded to the storage layer which
    // constructs precise key-range selectors using index metadata.
    let partition_id_range = partition_range;

    Ok(IndexScanPlan {
        space: space_name.to_string(),
        index_id,
        predicate: physical_predicate,
        partition: graphdb_storage::storage::PartitionSelector::All,
        partition_id_range,
        projection,
        limit: None,
        offset: 0,
        read_timestamp,
    })
}

fn next_index_chunk(
    storage: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    cursor: &mut Option<Box<dyn IndexCursor<Row = IndexRow>>>,
    output_layout: &Arc<SlotLayout>,
    base: &mut OperatorBase,
    source: &str,
    vertex_projection: &[String],
) -> Result<Option<DataChunk>, QueryError> {
    loop {
        base.ensure_not_cancelled()?;
        let mut index_cursor = match cursor.take() {
            Some(cursor) => cursor,
            None => return Ok(None),
        };
        let rows = index_cursor
            .next_batch(base.chunk_size)
            .map_err(|error| storage_error(source, "read cursor", space_name, error))?;
        let exhausted = index_cursor.is_exhausted();
        let mut output_rows = Vec::with_capacity(rows.len());

        if !rows.is_empty() {
            let guard = storage.read();
            for row in rows {
                match row {
                    IndexRow::Covering {
                        entity_ref,
                        columns,
                    } => {
                        if let Some(row) = make_covering_vertex_row(&entity_ref, columns) {
                            output_rows.push(row);
                        }
                    }
                    IndexRow::RowId(entity_ref) => {
                        let vertex_id = entity_ref_to_vertex_id(&entity_ref);
                        if let Some(vid) = vertex_id {
                            let result = if vertex_projection.is_empty() {
                                guard.get_vertex(space_name, &vid)
                            } else {
                                guard.get_vertex_projected(space_name, &vid, vertex_projection)
                            };
                            match result {
                                Ok(Some(vertex)) => output_rows.push(make_vertex_row(vertex)),
                                Ok(None) => {
                                    debug_assert!(
                                        false,
                                        "cursor yielded stale vertex {} in space {}",
                                        vid, space_name
                                    );
                                }
                                Err(error) => {
                                    return Err(storage_error(
                                        source,
                                        "get indexed vertex",
                                        space_name,
                                        error,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        *cursor = Some(index_cursor);
        if !output_rows.is_empty() {
            let reservation = reserve_memory(base, &output_rows)?;
            let mut chunk = DataChunk::new_with_layout(output_rows, output_layout.clone());
            chunk.materialize_columns();
            if let Some(reservation) = reservation {
                chunk = chunk.with_memory_reservation(reservation);
            }
            return Ok(Some(chunk));
        }
        if exhausted {
            return Ok(None);
        }
    }
}

/// Convert an EntityRef to VertexId for back-to-table fetches.
fn entity_ref_to_vertex_id(entity_ref: &EntityRef) -> Option<VertexId> {
    match entity_ref {
        EntityRef::Vertex(vid) => Some(*vid),
        EntityRef::Edge { .. } => None, // Edge indexes not yet supported
    }
}

fn reserve_memory(
    base: &OperatorBase,
    rows: &[Vec<Value>],
) -> Result<Option<MemoryReservation>, QueryError> {
    let Some(runtime) = base.runtime.as_ref() else {
        return Ok(None);
    };
    let bytes = MemoryBudget::estimate_rows_memory(rows);
    runtime.memory_budget.reserve(bytes).map(Some)
}

fn parse_vertex_id(value: &str) -> VertexId {
    value
        .parse::<i64>()
        .map(VertexId::from_int64)
        .unwrap_or_else(|_| VertexId::from_string(value.to_string()))
}

fn next_buffer_chunk(
    base: &OperatorBase,
    buffer: &[Vec<Value>],
    current_index: &mut usize,
    col_names: &[String],
) -> Result<Option<DataChunk>, QueryError> {
    if *current_index >= buffer.len() {
        return Ok(None);
    }
    let end = (*current_index + base.chunk_size).min(buffer.len());
    let rows = buffer[*current_index..end].to_vec();
    *current_index = end;
    let reservation = reserve_memory(base, &rows)?;
    let layout = if col_names.is_empty() {
        Arc::clone(&base.output_layout)
    } else {
        Arc::new(SlotLayout::from_names(col_names))
    };
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.materialize_columns();
    if let Some(reservation) = reservation {
        chunk = chunk.with_memory_reservation(reservation);
    }
    Ok(Some(chunk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covering_index_rows_do_not_require_a_table_fetch() {
        let row = make_covering_vertex_row(
            &EntityRef::Vertex(VertexId::from_int64(7)),
            vec![("name".to_string(), Value::string("Alice"))],
        )
        .expect("vertex entity should produce a covering row");
        let Value::Vertex(vertex) = &row[0] else {
            panic!("covering row should contain a vertex");
        };
        assert_eq!(vertex.vid.as_int64(), Some(7));
        assert_eq!(
            vertex.get_property_any("name"),
            Some(&Value::string("Alice"))
        );
    }

    #[test]
    fn scan_source_terminates_after_consuming_its_buffer() {
        let mut source = SourceOperator::ScanVertices {
            buffer: vec![vec![Value::BigInt(1)]],
            current_index: 0,
            col_names: Vec::new(),
        };
        let mut base = OperatorBase::new(0);

        source.open(&mut base).expect("source should open");
        assert_eq!(
            source
                .next(&mut base)
                .expect("first pull should succeed")
                .map(|chunk| chunk.len()),
            Some(1)
        );
        assert!(source
            .next(&mut base)
            .expect("second pull should succeed")
            .is_none());
    }

    #[test]
    fn scan_source_splits_across_multiple_chunks() {
        let mut base = OperatorBase::new(0);
        let chunk_size = base.chunk_size;
        let row_count = chunk_size * 2 + 7;
        let buffer: Vec<Vec<Value>> = (0..row_count)
            .map(|i| vec![Value::BigInt(i as i64)])
            .collect();
        let mut source = SourceOperator::ScanVertices {
            buffer,
            current_index: 0,
            col_names: Vec::new(),
        };
        source.open(&mut base).expect("source should open");

        let chunk1 = source
            .next(&mut base)
            .expect("first pull should succeed")
            .expect("first chunk should be Some");
        assert_eq!(chunk1.len(), chunk_size);

        let chunk2 = source
            .next(&mut base)
            .expect("second pull should succeed")
            .expect("second chunk should be Some");
        assert_eq!(chunk2.len(), chunk_size);

        let chunk3 = source
            .next(&mut base)
            .expect("third pull should succeed")
            .expect("third chunk should be Some");
        assert_eq!(chunk3.len(), 7);

        assert!(source
            .next(&mut base)
            .expect("fourth pull should succeed")
            .is_none());

        // Verify no data loss: concatenate all chunks
        let total: i64 = chunk1
            .rows
            .iter()
            .chain(chunk2.rows.iter())
            .chain(chunk3.rows.iter())
            .map(|row| match &row[0] {
                Value::BigInt(n) => *n,
                _ => 0,
            })
            .sum();
        let expected: i64 = (0..row_count as i64).sum();
        assert_eq!(total, expected);
    }

    #[test]
    fn buffered_scan_propagates_memory_budget_errors() {
        let runtime = Arc::new(
            crate::query::executor::streaming::runtime::ExecutionRuntime::new(
                crate::query::executor::streaming::runtime::QueryIdentity::default(),
                MemoryBudget::new(0),
                None,
                #[cfg(feature = "fulltext-search")]
                None,
                #[cfg(feature = "qdrant")]
                None,
            ),
        );
        let mut base = OperatorBase::new(0).with_runtime(Some(runtime));
        let mut source = SourceOperator::ScanVertices {
            buffer: vec![vec![Value::string("row")]],
            current_index: 0,
            col_names: Vec::new(),
        };

        source.open(&mut base).expect("source should open");
        let error = source
            .next(&mut base)
            .expect_err("the source must propagate a memory budget error");
        assert!(error.to_string().contains("Memory budget exceeded"));
    }

    #[test]
    fn buffered_source_open_returns_configuration_errors() {
        let mut source = SourceOperator::GetVertices {
            storage: None,
            space_name: "test".to_string(),
            vertex_ids: None,
            cached_ids: Vec::new(),
            projected_properties: Vec::new(),
        };
        let mut base = OperatorBase::new(0);

        // GetVertices without storage is an error in the new incremental path
        let error = source
            .next(&mut base)
            .expect_err("source without storage must fail");
        assert!(error.to_string().contains("requires storage"));
        assert!(source.close(&mut base).is_ok());
    }

    #[test]
    fn get_vertices_single_id_returns_none_on_second_call() {
        let mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        let storage = Arc::new(RwLock::new(mock));
        let mut source = SourceOperator::GetVertices {
            storage: Some(storage),
            space_name: "test".to_string(),
            vertex_ids: Some(vec![Value::string("1")]),
            cached_ids: Vec::new(),
            projected_properties: Vec::new(),
        };
        let runtime = Arc::new(
            crate::query::executor::streaming::runtime::ExecutionRuntime::new(
                crate::query::executor::streaming::runtime::QueryIdentity::default(),
                MemoryBudget::new(1024 * 1024),
                None,
                #[cfg(feature = "fulltext-search")]
                None,
                #[cfg(feature = "qdrant")]
                None,
            ),
        );
        let mut base = OperatorBase::new(0).with_runtime(Some(runtime));

        source.open(&mut base).expect("open should succeed");

        let result1 = source.next(&mut base).expect("first next should succeed");
        assert!(
            result1.is_none(),
            "no vertex in mock, first call returns None"
        );

        let result2 = source.next(&mut base).expect("second next should succeed");
        assert!(
            result2.is_none(),
            "second call must also return None (regression: do not re-emit)"
        );
    }
}

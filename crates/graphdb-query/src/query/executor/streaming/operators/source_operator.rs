use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::base::{MemoryBudget, MemoryReservation};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::storage::cursor::{
    open_edge_scan, open_vertex_scan, EdgeCursor, ScanOptions, VecEdgeCursor, VertexCursor,
};
use crate::storage::StorageClient;

#[derive(Debug)]
pub enum NeighborScanState {
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

impl Default for NeighborScanState {
    fn default() -> Self {
        Self::Init
    }
}

fn make_vertex_row(vertex: crate::core::vertex_edge_path::Vertex) -> Vec<Value> {
    vec![Value::Vertex(Box::new(vertex))]
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

#[derive(Debug)]
pub enum SourceOperator {
    ScanVertices {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    StorageScanVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        partition_id: usize,
        partition_range: Option<std::ops::Range<i64>>,
        cursor: Option<Box<dyn VertexCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    ScanEdges {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    StorageScanEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        edge_type: Option<String>,
        partition_id: usize,
        partition_range: Option<std::ops::Range<i64>>,
        cursor: Option<Box<dyn EdgeCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    GetVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        position: usize,
    },
    GetEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        direction: String,
        state: NeighborScanState,
    },
    EdgeIndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    IndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: Option<String>,
        index_value: Option<Value>,
        resolved_ids: Vec<Value>,
        position: usize,
    },
    Argument,
    GetProp {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        edge_ids: Option<Vec<Value>>,
        prop_names: Vec<String>,
    },
    LookupIndex {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        index_condition: Option<(String, Value)>,
        limit: Option<usize>,
    },
    Start,
}

impl SourceOperator {
    /// Create a SourceOperator with fresh mutable state from an immutable spec
    /// and the per-query storage client.
    pub fn from_spec(
        spec: &super::spec::SourceSpec,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
    ) -> Self {
        match spec {
            super::spec::SourceSpec::ScanVertices { rows, col_names } => {
                Self::ScanVertices {
                    partition_id: 0,
                    buffer: rows.clone(),
                    current_index: 0,
                    col_names: col_names.clone(),
                }
            }
            super::spec::SourceSpec::StorageScanVertices {
                space_name,
                limit,
                col_names,
            } => Self::StorageScanVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                partition_id: 0,
                partition_range: None,
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            super::spec::SourceSpec::ScanEdges { rows, col_names } => {
                Self::ScanEdges {
                    partition_id: 0,
                    buffer: rows.clone(),
                    current_index: 0,
                    col_names: col_names.clone(),
                }
            }
            super::spec::SourceSpec::StorageScanEdges {
                space_name,
                limit,
                edge_type,
                col_names,
            } => Self::StorageScanEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                edge_type: edge_type.clone(),
                partition_id: 0,
                partition_range: None,
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            super::spec::SourceSpec::GetVertices {
                space_name,
                vertex_ids,
            } => Self::GetVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                vertex_ids: vertex_ids.clone(),
                position: 0,
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
            } => Self::GetNeighbors {
                storage: storage.clone(),
                space_name: space_name.clone(),
                direction: direction.clone(),
                state: NeighborScanState::Init,
            },
            super::spec::SourceSpec::EdgeIndexScan {
                space_name,
                edge_type,
            } => Self::EdgeIndexScan {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_type: edge_type.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::IndexScan {
                space_name,
                index_name,
                index_value,
            } => Self::IndexScan {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                index_value: index_value.clone(),
                resolved_ids: Vec::new(),
                position: 0,
            },
            super::spec::SourceSpec::Argument => Self::Argument,
            super::spec::SourceSpec::GetProp {
                space_name,
                vertex_ids,
                edge_ids,
                prop_names,
            } => Self::GetProp {
                storage: storage.clone(),
                space_name: space_name.clone(),
                vertex_ids: vertex_ids.clone(),
                edge_ids: edge_ids.clone(),
                prop_names: prop_names.clone(),
            },
            super::spec::SourceSpec::LookupIndex {
                space_name,
                index_name,
                index_condition,
                limit,
            } => Self::LookupIndex {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                index_condition: index_condition.clone(),
                limit: *limit,
            },
            super::spec::SourceSpec::Start => Self::Start,
        }
    }

    pub fn open(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        match self {
            Self::ScanVertices { current_index, .. } | Self::ScanEdges { current_index, .. } => {
                *current_index = 0;
            }
            Self::StorageScanVertices {
                storage,
                space_name,
                limit,
                partition_range,
                cursor,
                current_index,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("StorageScanVertices requires storage".to_string())
                })?;
                let options = ScanOptions {
                    limit: *limit,
                    vertex_id_range: partition_range.clone(),
                    ..ScanOptions::default()
                };
                *cursor = Some(open_vertex_scan(storage, space_name, &options).map_err(
                    |error| storage_error("StorageScanVertices", "open cursor", space_name, error),
                )?);
                *current_index = 0;
            }
            Self::StorageScanEdges {
                storage,
                space_name,
                limit,
                edge_type,
                partition_range,
                cursor,
                current_index,
                ..
            } => {
                let storage = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("StorageScanEdges requires storage".to_string())
                })?;
                let options = ScanOptions {
                    limit: *limit,
                    edge_type: edge_type.clone(),
                    edge_src_id_range: partition_range.clone(),
                    ..ScanOptions::default()
                };
                *cursor = Some(
                    open_edge_scan(storage, space_name, &options).map_err(|error| {
                        storage_error("StorageScanEdges", "open cursor", space_name, error)
                    })?,
                );
                *current_index = 0;
            }
            Self::GetVertices { position, .. } => {
                *position = 0;
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
                    // Release storage read lock before cursor creation
                    drop(guard);
                    let cursor_obj =
                        open_edge_scan(storage_ref, space_name, &scan_opts).map_err(|error| {
                            storage_error("GetEdges", "open cursor", space_name, error)
                        })?;
                    *cursor = Some(cursor_obj);
                    Vec::new()
                };
                if !edges.is_empty() {
                    *cursor = Some(Box::new(VecEdgeCursor::new(edges)));
                }
            }
            Self::GetNeighbors { state, .. } => {
                *state = NeighborScanState::Init;
            }
            Self::EdgeIndexScan {
                storage,
                space_name,
                edge_type,
                cursor,
            } => {
                if let Some(storage_ref) = storage {
                    let options = ScanOptions {
                        edge_type: edge_type.clone(),
                        ..ScanOptions::default()
                    };
                    *cursor = Some(open_edge_scan(storage_ref, space_name, &options).map_err(
                        |error| storage_error("EdgeIndexScan", "open cursor", space_name, error),
                    )?);
                }
            }
            Self::IndexScan {
                resolved_ids,
                position,
                ..
            } => {
                *position = 0;
                resolved_ids.clear();
            }
            _ => {}
        }

        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(&mut self, base: &mut OperatorBase) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::ScanVertices {
                current_index,
                buffer,
                col_names,
                ..
            }
            | Self::ScanEdges {
                current_index,
                buffer,
                col_names,
                ..
            } => next_buffer_chunk(base, buffer, current_index, col_names),
            Self::StorageScanVertices {
                space_name,
                cursor: cursor_state,
                col_names,
                ..
            } => loop {
                base.ensure_not_cancelled()?;
                let Some(cursor) = cursor_state.as_mut() else {
                    return Ok(None);
                };
                let batch = cursor.next_batch(base.chunk_size).map_err(|error| {
                    storage_error("StorageScanVertices", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    *cursor_state = None;
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_vertex_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk = DataChunk::new_with_layout(
                        rows,
                        Arc::new(SlotLayout::from_names(col_names)),
                    );
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    return Ok(Some(chunk));
                }
            },
            Self::StorageScanEdges {
                space_name,
                cursor: cursor_state,
                col_names,
                ..
            } => loop {
                base.ensure_not_cancelled()?;
                let Some(cursor) = cursor_state.as_mut() else {
                    return Ok(None);
                };
                let batch = cursor.next_batch(base.chunk_size).map_err(|error| {
                    storage_error("StorageScanEdges", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    *cursor_state = None;
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_edge_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk = DataChunk::new_with_layout(
                        rows,
                        Arc::new(SlotLayout::from_names(col_names)),
                    );
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    return Ok(Some(chunk));
                }
            },
            Self::GetVertices {
                storage,
                space_name,
                vertex_ids,
                position,
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("GetVertices requires storage".to_string())
                })?;
                let guard = storage_ref.read();
                let ids = vertex_ids.as_ref().ok_or_else(|| {
                    QueryError::execution("GetVertices requires vertex IDs".to_string())
                })?;
                while *position < ids.len() {
                    base.ensure_not_cancelled()?;
                    let end = (*position + base.chunk_size).min(ids.len());
                    let batch = &ids[*position..end];
                    *position = end;
                    let mut rows = Vec::new();
                    for id_val in batch {
                        if let Ok(vid) = VertexId::try_from(id_val) {
                            if let Some(vertex) =
                                guard.get_vertex(space_name, &vid).map_err(|error| {
                                    storage_error("GetVertices", "get vertex", space_name, error)
                                })?
                            {
                                rows.push(make_vertex_row(vertex));
                            }
                        }
                    }
                    if !rows.is_empty() {
                        let reservation = reserve_memory(base, &rows)?;
                        let mut chunk = DataChunk::from_rows(rows);
                        if let Some(r) = reservation {
                            chunk = chunk.with_memory_reservation(r);
                        }
                        return Ok(Some(chunk));
                    }
                }
                Ok(None)
            }
            Self::GetEdges {
                space_name,
                cursor: cursor_state,
                ..
            } => {
                let Some(cursor) = cursor_state.as_mut() else {
                    return Ok(None);
                };
                let batch = cursor
                    .next_batch(base.chunk_size)
                    .map_err(|error| storage_error("GetEdges", "read cursor", space_name, error))?;
                if batch.is_empty() {
                    *cursor_state = None;
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_edge_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk = DataChunk::from_rows(rows);
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    return Ok(Some(chunk));
                }
                Ok(None)
            }
            Self::GetNeighbors {
                storage,
                space_name,
                direction,
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
                                // All vertices processed, transition to fetching
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
                                    if seen.insert(nid.clone()) {
                                        neighbor_ids.push(nid);
                                    }
                                }
                            }
                            drop(guard);
                            *position = end;
                            // Continue loop to check if we can transition or process next batch
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
                            let mut rows = Vec::new();
                            for neighbor_id in &neighbor_ids[*position..end] {
                                if let Some(vertex) =
                                    guard.get_vertex(space_name, neighbor_id).map_err(|error| {
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
                            drop(guard);
                            *position = end;

                            if !rows.is_empty() {
                                let reservation = reserve_memory(base, &rows)?;
                                let mut chunk = DataChunk::from_rows(rows);
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
            Self::GetProp { .. } | Self::LookupIndex { .. } => Ok(None),
            Self::EdgeIndexScan {
                space_name,
                cursor: cursor_state,
                ..
            } => loop {
                base.ensure_not_cancelled()?;
                let Some(cursor) = cursor_state.as_mut() else {
                    return Ok(None);
                };
                let batch = cursor.next_batch(base.chunk_size).map_err(|error| {
                    storage_error("EdgeIndexScan", "read cursor", space_name, error)
                })?;
                if batch.is_empty() {
                    *cursor_state = None;
                    return Ok(None);
                }
                let rows = batch.into_iter().map(make_edge_row).collect::<Vec<_>>();
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let mut chunk = DataChunk::from_rows(rows);
                    if let Some(r) = reservation {
                        chunk = chunk.with_memory_reservation(r);
                    }
                    return Ok(Some(chunk));
                }
            },
            Self::IndexScan {
                storage,
                space_name,
                index_name,
                index_value,
                resolved_ids,
                position,
            } => {
                let storage_ref = storage.as_ref().ok_or_else(|| {
                    QueryError::execution("IndexScan requires storage".to_string())
                })?;

                // Lazy index lookup on first next() call
                if resolved_ids.is_empty() && position == &0 {
                    let guard = storage_ref.read();
                    *resolved_ids = match (index_name.as_ref(), index_value.as_ref()) {
                        (Some(name), Some(value)) => guard
                            .lookup_index(space_name, name, value)
                            .map_err(|error| {
                                storage_error("IndexScan", "lookup index", space_name, error)
                            })?,
                        _ => Vec::new(),
                    };
                }

                let guard = storage_ref.read();
                while *position < resolved_ids.len() {
                    base.ensure_not_cancelled()?;
                    let end = (*position + base.chunk_size).min(resolved_ids.len());
                    let batch = &resolved_ids[*position..end];
                    *position = end;
                    let mut rows = Vec::new();
                    for id_val in batch {
                        if let Ok(vid) = VertexId::try_from(id_val) {
                            if let Some(vertex) =
                                guard.get_vertex(space_name, &vid).map_err(|error| {
                                    storage_error("IndexScan", "get vertex", space_name, error)
                                })?
                            {
                                rows.push(make_vertex_row(vertex));
                            }
                        }
                    }
                    if !rows.is_empty() {
                        let reservation = reserve_memory(base, &rows)?;
                        let mut chunk = DataChunk::from_rows(rows);
                        if let Some(r) = reservation {
                            chunk = chunk.with_memory_reservation(r);
                        }
                        return Ok(Some(chunk));
                    }
                }
                Ok(None)
            }
            Self::Argument | Self::Start => Ok(None),
        }
    }

    pub fn stop(&mut self, _base: &mut OperatorBase) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        match self {
            Self::StorageScanVertices {
                cursor,
                current_index,
                ..
            } => {
                *cursor = None;
                *current_index = 0;
            }
            Self::StorageScanEdges {
                cursor,
                current_index,
                ..
            } => {
                *cursor = None;
                *current_index = 0;
            }
            Self::ScanVertices { current_index, .. } | Self::ScanEdges { current_index, .. } => {
                *current_index = 0;
            }
            Self::GetVertices { position, .. } => {
                *position = 0;
            }
            Self::EdgeIndexScan { cursor, .. } => {
                *cursor = None;
            }
            Self::IndexScan {
                resolved_ids,
                position,
                ..
            } => {
                resolved_ids.clear();
                *position = 0;
            }
            Self::GetEdges { cursor, .. } => {
                *cursor = None;
            }
            Self::GetNeighbors { state, .. } => {
                *state = NeighborScanState::Init;
            }
            Self::GetProp { .. } | Self::LookupIndex { .. } | Self::Argument | Self::Start => {}
        }
        base.lifecycle.mark_closed();
        Ok(())
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
    let mut chunk = DataChunk::new_with_layout(rows, Arc::new(SlotLayout::from_names(col_names)));
    if let Some(reservation) = reservation {
        chunk = chunk.with_memory_reservation(reservation);
    }
    Ok(Some(chunk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_source_terminates_after_consuming_its_buffer() {
        let mut source = SourceOperator::ScanVertices {
            partition_id: 0,
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
            partition_id: 0,
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
            partition_id: 0,
            buffer: vec![vec![Value::String("row".to_string())]],
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
            position: 0,
        };
        let mut base = OperatorBase::new(0);

        // GetVertices without storage is an error in the new incremental path
        let error = source
            .next(&mut base)
            .expect_err("source without storage must fail");
        assert!(error.to_string().contains("requires storage"));
        assert!(source.close(&mut base).is_ok());
    }
}

use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::state::SourceState;
use crate::query::executor::streaming::state::GlobalState;
use crate::storage::{EdgeCursor, ScanOptions, VecEdgeCursor, open_edge_scan};

use super::SourceOperator;
use super::util::{make_edge_row, make_vertex_row, parse_vertex_id, reserve_memory, storage_error};

/// Open the point-lookup source variants.
pub(crate) fn open(op: &mut SourceOperator, base: &mut OperatorBase) -> Result<(), QueryError> {
    match op {
        SourceOperator::GetVertices {
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
            base.insert_state(GlobalState::Source(SourceState::GetVertices { position: 0 }));
        }
        SourceOperator::GetEdges {
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
        _ => unreachable!("point_lookup::open called for a non-lookup source"),
    }
    Ok(())
}

/// Emit the next chunk for `GetVertices` or `GetEdges`.
pub(crate) fn next(
    op: &mut SourceOperator,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    match op {
        SourceOperator::GetVertices {
            storage,
            space_name,
            vertex_ids,
            cached_ids,
            projected_properties,
        } => next_get_vertices(
            base,
            storage,
            space_name,
            vertex_ids,
            cached_ids,
            projected_properties,
        ),
        SourceOperator::GetEdges {
            space_name, cursor, ..
        } => next_get_edges(base, space_name, cursor),
        _ => unreachable!("point_lookup::next called for a non-lookup source"),
    }
}

#[allow(clippy::too_many_arguments)]
fn next_get_vertices(
    base: &mut OperatorBase,
    storage: &Option<Arc<parking_lot::RwLock<dyn crate::storage::QueryStorage>>>,
    space_name: &str,
    vertex_ids: &Option<Vec<Value>>,
    cached_ids: &[VertexId],
    projected_properties: &[String],
) -> Result<Option<DataChunk>, QueryError> {
    // Fast path: single-ID lookup without batch/position machinery.
    if let Some(ids) = vertex_ids.as_ref() {
        if ids.len() == 1 {
            // Check if already returned (state position >= ids.len()).
            {
                let mut arena = base.state_arena();
                if let Some(GlobalState::Source(SourceState::GetVertices { position })) =
                    arena.global.get_mut(&base.state_key())
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
                if let Some(GlobalState::Source(SourceState::GetVertices { position })) =
                    arena.global.get_mut(&base.state_key())
                {
                    *position = ids.len();
                }
            };
            if let Some(vertex) = vertex_opt {
                let rows = vec![make_vertex_row(vertex)];
                let reservation = reserve_memory(base, &rows)?;
                let mut chunk = DataChunk::new_with_layout(rows, base.output_layout.clone());
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
                if let Some(vertex) = guard.get_vertex(space_name, vid).map_err(|error| {
                    storage_error("GetVertices", "get vertex", space_name, error)
                })? {
                    rows.push(make_vertex_row(vertex));
                }
            }
        } else {
            for vid in &cached_ids[start..position] {
                if let Some(vertex) = guard
                    .get_vertex_projected(space_name, vid, projected_properties)
                    .map_err(|error| {
                        storage_error("GetVertices", "get vertex", space_name, error)
                    })? {
                    rows.push(make_vertex_row(vertex));
                }
            }
        }
        if !rows.is_empty() {
            let reservation = reserve_memory(base, &rows)?;
            let mut chunk = DataChunk::new_with_layout(rows, base.output_layout.clone());
            chunk.materialize_columns();
            if let Some(r) = reservation {
                chunk = chunk.with_memory_reservation(r);
            }
            return Ok(Some(chunk));
        }
    }
}

fn next_get_edges(
    base: &mut OperatorBase,
    space_name: &str,
    cursor: &mut Option<Box<dyn EdgeCursor>>,
) -> Result<Option<DataChunk>, QueryError> {
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
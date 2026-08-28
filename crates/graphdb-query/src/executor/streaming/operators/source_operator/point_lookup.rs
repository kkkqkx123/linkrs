use graphdb_core::error::QueryError;
use graphdb_core::types::storage_ids::VertexId;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::operators::state::SourceState;
use crate::executor::streaming::state::GlobalState;
use crate::executor::streaming::state::GlobalStateKey;
use crate::storage::{open_edge_scan, ScanOptions, VecEdgeCursor};

use super::util::{
    attach_columnar_stats, make_edge_row, make_flat_edge_row, make_flat_vertex_row,
    parse_vertex_id, reserve_memory, storage_error,
};
use super::SourceOperator;
use super::SourceOperatorKind;

/// Open the point-lookup source variants.
pub(crate) fn open(op: &mut SourceOperator) -> Result<(), QueryError> {
    let state = match &mut op.kind {
        SourceOperatorKind::GetVertices {
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
            GlobalState::Source(SourceState::GetVertices { position: 0 })
        }
        SourceOperatorKind::GetEdges {
            storage,
            space_name,
            edge_type,
            src,
            dst,
            rank,
            projected_properties,
            cursor,
        } => {
            let storage_ref = storage
                .as_ref()
                .ok_or_else(|| QueryError::execution("GetEdges requires storage".to_string()))?;
            let guard = storage_ref.read();
            let edges = if let (Some(src), Some(dst), Some(edge_type)) =
                (src.as_deref(), dst.as_deref(), edge_type.as_deref())
            {
                let src = parse_vertex_id(src);
                let dst = parse_vertex_id(dst);
                if projected_properties.is_empty() {
                    guard
                        .get_edge(space_name, &src, &dst, edge_type, *rank)
                        .map_err(|error| storage_error("GetEdges", "get edge", space_name, error))?
                        .into_iter()
                        .collect::<Vec<_>>()
                } else {
                    guard
                        .get_edge_projected(
                            space_name,
                            &src,
                            &dst,
                            edge_type,
                            *rank,
                            projected_properties,
                        )
                        .map_err(|error| {
                            storage_error("GetEdges", "get edge projected", space_name, error)
                        })?
                        .into_iter()
                        .collect::<Vec<_>>()
                }
            } else {
                let scan_opts = ScanOptions {
                    edge_type: edge_type.clone(),
                    ..ScanOptions::default()
                };
                drop(guard);
                let scan_cursor = open_edge_scan(storage_ref, space_name, &scan_opts)
                    .map_err(|error| storage_error("GetEdges", "open cursor", space_name, error))?;
                *cursor = Some(scan_cursor);
                Vec::new()
            };
            if !edges.is_empty() {
                *cursor = Some(Box::new(VecEdgeCursor::new(edges)));
            }
            GlobalState::Source(SourceState::GetEdges { cursor: None })
        }
        _ => unreachable!("point_lookup::open called for a non-lookup source"),
    };
    op.insert_state(state);
    Ok(())
}

/// Emit the next chunk for `GetVertices` or `GetEdges`.
pub(crate) fn next(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
    if matches!(&op.kind, SourceOperatorKind::GetVertices { .. }) {
        return next_get_vertices(op);
    }
    if matches!(&op.kind, SourceOperatorKind::GetEdges { .. }) {
        return next_get_edges(op);
    }
    unreachable!("point_lookup::next called for a non-lookup source")
}

#[allow(clippy::too_many_arguments)]
fn next_get_vertices(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
    let SourceOperatorKind::GetVertices {
        storage,
        space_name,
        vertex_ids,
        cached_ids,
        projected_properties,
    } = &mut op.kind
    else {
        unreachable!("next_get_vertices called for a non-get-vertices source");
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let vertex_ids = &*vertex_ids;
    let cached_ids = &*cached_ids;
    let projected_properties = &*projected_properties;
    // Fast path: single-ID lookup without batch/position machinery.
    if let Some(ids) = vertex_ids.as_ref() {
        if ids.len() == 1 {
            // Check if already returned (state position >= ids.len()).
            {
                let mut arena = op
                    .runtime
                    .as_ref()
                    .expect("runtime required")
                    .state_arena_for(op.config.partition_id)
                    .lock();
                if let Some(GlobalState::Source(SourceState::GetVertices { position })) =
                    arena.global.get_mut(&GlobalStateKey(
                        op.config.physical_operator_id,
                        op.config.partition_id,
                    ))
                {
                    if *position >= ids.len() {
                        return Ok(None);
                    }
                }
            }
            let storage_ref = storage
                .as_ref()
                .ok_or_else(|| QueryError::execution("GetVertices requires storage".to_string()))?;
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
            let mark_done = {
                let runtime = &op.runtime;
                let key = GlobalStateKey(op.config.physical_operator_id, op.config.partition_id);
                move |done: usize| {
                    let mut arena = runtime
                        .as_ref()
                        .expect("runtime required")
                        .state_arena_for(key.1)
                        .lock();
                    if let Some(GlobalState::Source(SourceState::GetVertices { position })) =
                        arena.global.get_mut(&key)
                    {
                        *position = done;
                    }
                }
            };
            if let Some(vertex) = vertex_opt {
                let rows = vec![make_flat_vertex_row(vertex, projected_properties)];
                let reservation = reserve_memory(&op.runtime, &rows)?;
                let chunk = attach_columnar_stats(
                    &op.runtime,
                    DataChunk::new_with_layout(rows, op.output_layout.clone()),
                );
                let chunk = if let Some(r) = reservation {
                    chunk.with_memory_reservation(r)
                } else {
                    chunk
                };
                mark_done(ids.len());
                return Ok(Some(chunk));
            }
            mark_done(ids.len());
            return Ok(None);
        }
    }
    loop {
        let storage_ref = storage
            .as_ref()
            .ok_or_else(|| QueryError::execution("GetVertices requires storage".to_string()))?;
        let guard = storage_ref.read();
        let ids = vertex_ids
            .as_ref()
            .ok_or_else(|| QueryError::execution("GetVertices requires vertex IDs".to_string()))?;
        let (position, done) = {
            let mut arena = op
                .runtime
                .as_ref()
                .expect("runtime required")
                .state_arena_for(op.config.partition_id)
                .lock();
            let s = arena
                .global
                .get_mut(&GlobalStateKey(
                    op.config.physical_operator_id,
                    op.config.partition_id,
                ))
                .unwrap();
            let GlobalState::Source(SourceState::GetVertices { position }) = s else {
                return Ok(None);
            };
            if *position >= ids.len() {
                (0, true)
            } else {
                let end = (*position + op.config.chunk_size).min(ids.len());
                *position = end;
                (end, false)
            }
        };
        if done {
            return Ok(None);
        }
        let start = position.saturating_sub(op.config.chunk_size);
        let mut rows = Vec::with_capacity(position - start);
        if projected_properties.is_empty() {
            for vid in &cached_ids[start..position] {
                if let Some(vertex) = guard.get_vertex(space_name, vid).map_err(|error| {
                    storage_error("GetVertices", "get vertex", space_name, error)
                })? {
                    rows.push(make_flat_vertex_row(vertex, projected_properties));
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
                    rows.push(make_flat_vertex_row(vertex, projected_properties));
                }
            }
        }
        if !rows.is_empty() {
            let reservation = reserve_memory(&op.runtime, &rows)?;
            let chunk = attach_columnar_stats(
                &op.runtime,
                DataChunk::new_with_layout(rows, op.output_layout.clone()),
            );
            let chunk = if let Some(r) = reservation {
                chunk.with_memory_reservation(r)
            } else {
                chunk
            };
            return Ok(Some(chunk));
        }
    }
}

fn next_get_edges(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
    let SourceOperatorKind::GetEdges {
        space_name,
        projected_properties,
        cursor,
        ..
    } = &mut op.kind
    else {
        unreachable!("next_get_edges called for a non-get-edges source");
    };
    let space_name = &*space_name;
    let projected_properties = &*projected_properties;
    let mut cur = match cursor.take() {
        Some(c) => c,
        None => return Ok(None),
    };
    let batch = cur
        .next_batch(op.config.chunk_size)
        .map_err(|error| storage_error("GetEdges", "read cursor", space_name, error))?;
    if batch.is_empty() {
        return Ok(None);
    }
    let flat = !projected_properties.is_empty();
    let rows = batch
        .into_iter()
        .map(|mut edge| {
            if flat {
                edge.props
                    .retain(|key, _| projected_properties.contains(key));
                make_flat_edge_row(edge, projected_properties)
            } else {
                make_edge_row(edge)
            }
        })
        .collect::<Vec<_>>();
    if !rows.is_empty() {
        let reservation = reserve_memory(&op.runtime, &rows)?;
        let chunk = attach_columnar_stats(
            &op.runtime,
            DataChunk::new_with_layout(rows, op.output_layout.clone()),
        );
        let chunk = if let Some(r) = reservation {
            chunk.with_memory_reservation(r)
        } else {
            chunk
        };
        *cursor = Some(cur);
        return Ok(Some(chunk));
    }
    *cursor = Some(cur);
    Ok(None)
}

use std::collections::HashSet;

use crate::core::types::storage_ids::VertexId;
use crate::core::{error::QueryError, EdgeDirection};
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::operators::state::SourceState;
use crate::executor::streaming::state::GlobalState;
use std::sync::Arc;

use super::util::{attach_columnar_stats, make_flat_vertex_row, reserve_memory, storage_error};
use super::SourceOperator;
use super::SourceOperatorKind;

/// Two-phase neighbor scan state machine.
///
/// `Init` → `Collecting` (gather neighbor IDs from each vertex's edges) →
/// `Fetching` (page in neighbor vertices by ID) → `Done`.
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

/// Open the `GetNeighbors` source: reset the scan state machine.
pub(crate) fn open(op: &mut SourceOperator) -> Result<(), QueryError> {
    let state = match &mut op.kind {
        SourceOperatorKind::GetNeighbors { state, .. } => {
            *state = NeighborScanState::Init;
            NeighborScanState::Init
        }
        _ => unreachable!("neighbors::open called for a non-neighbor source"),
    };
    op.insert_state(GlobalState::Source(SourceState::GetNeighbors { state }));
    Ok(())
}

/// Emit the next chunk of neighbor vertices.
pub(crate) fn next(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
    if !matches!(&op.kind, SourceOperatorKind::GetNeighbors { .. }) {
        unreachable!("neighbors::next called for a non-neighbor source");
    }
    next_get_neighbors(op)
}

#[allow(clippy::too_many_arguments)]
fn next_get_neighbors(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
    let SourceOperatorKind::GetNeighbors {
        storage,
        space_name,
        direction,
        projected_properties,
        state,
    } = &mut op.kind
    else {
        unreachable!("next_get_neighbors called for a non-neighbor source");
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let direction = &*direction;
    let projected_properties = &*projected_properties;
    let state = &mut *state;
    let storage_ref = storage
        .as_ref()
        .ok_or_else(|| QueryError::execution("GetNeighbors requires storage".to_string()))?;

    loop {
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
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
                let end = (*position + op.config.chunk_size).min(vertex_ids.len());
                let guard = storage_ref.read();
                for vid in &vertex_ids[*position..end] {
                    let edges =
                        guard
                            .get_node_edges(space_name, vid, *direction)
                            .map_err(|error| {
                                storage_error("GetNeighbors", "get node edges", space_name, error)
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
                let end = (*position + op.config.chunk_size).min(neighbor_ids.len());
                let guard = storage_ref.read();
                let batch_size = end - *position;
                let mut rows = Vec::with_capacity(batch_size);
                if projected_properties.is_empty() {
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
                            rows.push(make_flat_vertex_row(vertex, projected_properties));
                        }
                    }
                } else {
                    for neighbor_id in &neighbor_ids[*position..end] {
                        if let Some(vertex) = guard
                            .get_vertex_projected(space_name, neighbor_id, projected_properties)
                            .map_err(|error| {
                                storage_error(
                                    "GetNeighbors",
                                    "get neighbor vertex",
                                    space_name,
                                    error,
                                )
                            })?
                        {
                            rows.push(make_flat_vertex_row(vertex, projected_properties));
                        }
                    }
                }
                drop(guard);
                *position = end;
                if !rows.is_empty() {
                    let reservation = reserve_memory(&op.runtime, &rows)?;
                    let chunk = attach_columnar_stats(
                        &op.runtime,
                        DataChunk::new_with_layout(rows, Arc::clone(&op.output_layout)),
                    );
                    let chunk = if let Some(r) = reservation {
                        chunk.with_memory_reservation(r)
                    } else {
                        chunk
                    };
                    return Ok(Some(chunk));
                }
            }
            NeighborScanState::Done => return Ok(None),
        }
    }
}

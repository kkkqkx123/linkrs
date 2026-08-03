use std::collections::HashSet;

use crate::core::{error::QueryError, EdgeDirection};
use crate::core::types::storage_ids::VertexId;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::state::SourceState;
use crate::query::executor::streaming::state::GlobalState;
use crate::storage::QueryStorage;
use std::sync::Arc;

use super::SourceOperator;
use super::util::{attach_columnar_stats, make_flat_vertex_row, reserve_memory, storage_error};

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
pub(crate) fn open(op: &mut SourceOperator, base: &mut OperatorBase) -> Result<(), QueryError> {
    match op {
        SourceOperator::GetNeighbors { state, .. } => {
            *state = NeighborScanState::Init;
            base.insert_state(GlobalState::Source(SourceState::GetNeighbors {
                state: NeighborScanState::Init,
            }));
        }
        _ => unreachable!("neighbors::open called for a non-neighbor source"),
    }
    Ok(())
}

/// Emit the next chunk of neighbor vertices.
pub(crate) fn next(
    op: &mut SourceOperator,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    match op {
        SourceOperator::GetNeighbors {
            storage,
            space_name,
            direction,
            projected_properties,
            state,
        } => next_get_neighbors(
            base,
            storage,
            space_name,
            direction,
            projected_properties,
            state,
        ),
        _ => unreachable!("neighbors::next called for a non-neighbor source"),
    }
}

#[allow(clippy::too_many_arguments)]
fn next_get_neighbors(
    base: &mut OperatorBase,
    storage: &Option<Arc<parking_lot::RwLock<dyn QueryStorage>>>,
    space_name: &str,
    direction: &str,
    projected_properties: &[String],
    state: &mut NeighborScanState,
) -> Result<Option<DataChunk>, QueryError> {
    let storage_ref = storage.as_ref().ok_or_else(|| {
        QueryError::execution("GetNeighbors requires storage".to_string())
    })?;

    loop {
        base.ensure_not_cancelled()?;
        match state {
            NeighborScanState::Init => {
                let dir: EdgeDirection = direction.into();
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
                            })? {
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
                            })? {
                            rows.push(make_flat_vertex_row(vertex, projected_properties));
                        }
                    }
                }
                drop(guard);
                *position = end;
                if !rows.is_empty() {
                    let reservation = reserve_memory(base, &rows)?;
                    let chunk = attach_columnar_stats(
                        base,
                        DataChunk::new_with_layout(rows, Arc::clone(&base.output_layout)),
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
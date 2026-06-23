use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::VertexId;
use crate::core::Edge;
use crate::query::executor::base::EdgeDirection;
use crate::storage::StorageReader;
use parking_lot::RwLock;

/// Obtaining neighbor nodes
///
/// Self-loop edges (A->A) can lead to an inflated result or an infinite loop during graph traversal.
/// By default, this function removes duplicates from self-loop edges by tracking the combinations of edge types and rankings that have already been processed.
/// Ensure that self-loop edges of the same type and ranking are only returned once.
///
/// # Parameters
/// “storage”: The storage component on the client side.
/// `node_id`: The ID of the current node.
/// `edge_direction`: Direction of the edge
/// `edge_types`: Filter by edge type
/// `allow_loop`: Whether self-loop edges are allowed (default is `false`, which means duplicate self-loop edges are removed).
///
/// # Return
/// List of neighbor nodes
///
/// # Example
/// ```
/// let neighbors = get_neighbors(
///     &storage,
///     &node_id,
///     EdgeDirection::Out,
///     &Some(vec!["follow".to_string()]),
///     false,
/// )?;
/// ```
pub fn get_neighbors<S: StorageReader>(
    storage: &Arc<RwLock<S>>,
    node_id: &VertexId,
    edge_direction: EdgeDirection,
    edge_types: &Option<Vec<String>>,
    space_name: &str,
    allow_loop: bool,
) -> DBResult<Vec<VertexId>> {
    let storage_guard = storage.read();

    let edges = storage_guard
        .get_node_edges(space_name, node_id, EdgeDirection::Both)
        .map_err(|e| DBError::storage(e.to_string()))?;

    let filtered_edges: Vec<Edge> = if let Some(ref edge_types) = edge_types {
        edges
            .into_iter()
            .filter(|edge| edge_types.contains(&edge.edge_type))
            .collect()
    } else {
        edges
    };

    let mut seen_self_loops: HashSet<(String, i64)> = HashSet::new();

    let neighbors: Vec<VertexId> = filtered_edges
        .into_iter()
        .filter_map(|edge| {
            let is_self_loop = edge.src == edge.dst;

            if is_self_loop && !allow_loop {
                let key = (edge.edge_type.clone(), edge.ranking);
                if !seen_self_loops.insert(key) {
                    return None;
                }
            }

            match edge_direction {
                EdgeDirection::In => {
                    if edge.dst == *node_id {
                        Some(edge.src)
                    } else {
                        None
                    }
                }
                EdgeDirection::Out => {
                    if edge.src == *node_id {
                        Some(edge.dst)
                    } else {
                        None
                    }
                }
                EdgeDirection::Both => {
                    if edge.src == *node_id {
                        Some(edge.dst)
                    } else if edge.dst == *node_id {
                        Some(edge.src)
                    } else {
                        None
                    }
                }
            }
        })
        .collect();

    Ok(neighbors)
}

/// Obtaining neighbor nodes and edges
///
/// By default, duplicate self-loop edges are removed to ensure that self-loop edges of the same type and ranking are only returned once.
///
/// # Parameters
/// - `storage`: storage client
/// - `node_id`: current node ID
/// - `edge_direction`: edge direction
/// - `edge_types`: edge type filtering
/// - `allow_loop`: whether to allow self-loop edges (default false, i.e. de-emphasize self-loop edges)
///
/// # Back
/// List of tuples representing neighbor nodes and edges
pub fn get_neighbors_with_edges<S: StorageReader>(
    storage: &Arc<RwLock<S>>,
    node_id: &VertexId,
    edge_direction: EdgeDirection,
    edge_types: &Option<Vec<String>>,
    space_name: &str,
    allow_loop: bool,
) -> DBResult<Vec<(VertexId, Edge)>> {
    let storage_guard = storage.read();

    let edges = storage_guard
        .get_node_edges(space_name, node_id, EdgeDirection::Both)
        .map_err(|e| DBError::storage(e.to_string()))?;

    let filtered_edges: Vec<Edge> = if let Some(ref edge_types) = edge_types {
        edges
            .into_iter()
            .filter(|edge| edge_types.contains(&edge.edge_type))
            .collect()
    } else {
        edges
    };

    let mut seen_self_loops: HashSet<(String, i64)> = HashSet::new();

    let neighbors_with_edges: Vec<(VertexId, Edge)> = filtered_edges
        .into_iter()
        .filter_map(|edge| {
            let is_self_loop = edge.src == edge.dst;

            if is_self_loop && !allow_loop {
                let key = (edge.edge_type.clone(), edge.ranking);
                if !seen_self_loops.insert(key) {
                    return None;
                }
            }

            match edge_direction {
                EdgeDirection::In => {
                    if edge.dst == *node_id {
                        Some((edge.src, edge))
                    } else {
                        None
                    }
                }
                EdgeDirection::Out => {
                    if edge.src == *node_id {
                        Some((edge.dst, edge))
                    } else {
                        None
                    }
                }
                EdgeDirection::Both => {
                    if edge.src == *node_id {
                        Some((edge.dst, edge))
                    } else if edge.dst == *node_id {
                        Some((edge.src, edge))
                    } else {
                        None
                    }
                }
            }
        })
        .collect();

    Ok(neighbors_with_edges)
}

//! Bidirectional BFS (Broad-Search First) shortest path algorithm
//!
//! Use a bidirectional breadth-first search to find the shortest path.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::core::types::{EdgeDirection, VertexId};
use crate::core::{Edge, NPath, Path, Vertex};
use crate::query::QueryError;
use crate::storage::StorageReader;
use parking_lot::RwLock;

use super::traits::ShortestPathAlgorithm;
use super::types::{combine_npaths, has_duplicate_edges, AlgorithmStats, SelfLoopDedup};

/// Bidirectional BFS (Broad-Search First) shortest path algorithm
pub struct BidirectionalBFS<S: StorageReader> {
    storage: Arc<RwLock<S>>,
    stats: AlgorithmStats,
    edge_direction: EdgeDirection,
    right_edge_direction: EdgeDirection,
    space_name: String,
}

impl<S: StorageReader> BidirectionalBFS<S> {
    pub fn new(storage: Arc<RwLock<S>>, space_name: String) -> Self {
        Self {
            storage,
            stats: AlgorithmStats::new(),
            edge_direction: EdgeDirection::Both,
            right_edge_direction: EdgeDirection::Both,
            space_name,
        }
    }

    pub fn with_edge_direction(mut self, direction: EdgeDirection) -> Self {
        let right_direction = match direction {
            EdgeDirection::Out => EdgeDirection::In,
            EdgeDirection::In => EdgeDirection::Out,
            EdgeDirection::Both => EdgeDirection::Both,
        };
        self.edge_direction = direction;
        self.right_edge_direction = right_direction;
        self
    }

    /// Obtaining neighbor nodes and edges
    fn get_neighbors_with_edges(
        &self,
        node_id: &VertexId,
        edge_types: Option<&[String]>,
        direction: EdgeDirection,
    ) -> Result<Vec<(VertexId, Edge, f64)>, QueryError> {
        let storage = self.storage.read();

        let edges = storage
            .get_node_edges(&self.space_name, node_id, direction)
            .map_err(|e| QueryError::storage(e.to_string()))?;

        let filtered_edges = if let Some(types) = edge_types {
            edges
                .into_iter()
                .filter(|edge| types.contains(&edge.edge_type))
                .collect()
        } else {
            edges
        };

        // Remove duplicates from the self-loop edges.
        let mut dedup = SelfLoopDedup::new();

        let neighbors_with_edges: Vec<(VertexId, Edge, f64)> = filtered_edges
            .into_iter()
            .filter(|edge| dedup.should_include(edge))
            .filter_map(|mut edge| {
                let (neighbor_id, weight) = match direction {
                    EdgeDirection::In => {
                        if edge.dst == *node_id {
                            // Reverse edge direction to match traversal direction (current -> neighbor)
                            std::mem::swap(&mut edge.src, &mut edge.dst);
                            (edge.dst, edge.ranking as f64)
                        } else {
                            return None;
                        }
                    }
                    EdgeDirection::Out => {
                        if edge.src == *node_id {
                            (edge.dst, edge.ranking as f64)
                        } else {
                            return None;
                        }
                    }
                    EdgeDirection::Both => {
                        if edge.src == *node_id {
                            (edge.dst, edge.ranking as f64)
                        } else if edge.dst == *node_id {
                            // When traversing backward (edge.dst == node_id), reverse the edge
                            std::mem::swap(&mut edge.src, &mut edge.dst);
                            (edge.dst, edge.ranking as f64)
                        } else {
                            return None;
                        }
                    }
                };
                Some((neighbor_id, edge, weight))
            })
            .collect();

        Ok(neighbors_with_edges)
    }

    /// Obtain the vertices
    fn get_vertex(&self, vid: &VertexId) -> Result<Option<Vertex>, QueryError> {
        let storage = self.storage.read();
        storage
            .get_vertex(&self.space_name, vid)
            .map_err(|e| QueryError::storage(e.to_string()))
    }
}

impl<S: StorageReader> ShortestPathAlgorithm for BidirectionalBFS<S> {
    fn find_paths(
        &mut self,
        start_ids: &[VertexId],
        end_ids: &[VertexId],
        edge_types: Option<&[String]>,
        max_depth: Option<usize>,
        single_shortest: bool,
        limit: usize,
    ) -> Result<Vec<Path>, QueryError> {
        let mut result_paths = Vec::new();
        let mut visited_left: HashMap<VertexId, Arc<NPath>> = HashMap::new();
        let mut visited_right: HashMap<VertexId, Arc<NPath>> = HashMap::new();
        let mut left_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();
        let mut right_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();

        for start_id in start_ids {
            if let Ok(Some(start_vertex)) = self.get_vertex(start_id) {
                let initial_npath = Arc::new(NPath::new(Arc::new(start_vertex)));
                left_queue.push_back((*start_id, initial_npath.clone()));
                visited_left.insert(*start_id, initial_npath);
            }
        }

        for end_id in end_ids {
            if let Ok(Some(end_vertex)) = self.get_vertex(end_id) {
                let initial_npath = Arc::new(NPath::new(Arc::new(end_vertex)));
                right_queue.push_back((*end_id, initial_npath.clone()));
                visited_right.insert(*end_id, initial_npath);
            }
        }

        while !left_queue.is_empty() && !right_queue.is_empty() {
            if single_shortest && !result_paths.is_empty() {
                break;
            }

            if result_paths.len() >= limit {
                break;
            }

            // Process only nodes from the current BFS level (left side)
            let left_level_size = left_queue.len();
            let mut left_next: Vec<(VertexId, Arc<NPath>)> = Vec::new();
            for _ in 0..left_level_size {
                if let Some((current_id, current_npath)) = left_queue.pop_front() {
                    self.stats.increment_nodes_visited();

                    if let Some(max_d) = max_depth {
                        if current_npath.len() >= max_d {
                            continue;
                        }
                    }

                    let neighbors = self.get_neighbors_with_edges(
                        &current_id,
                        edge_types,
                        self.edge_direction,
                    )?;
                    self.stats.increment_edges_traversed(neighbors.len());

                    for (neighbor_id, edge, _weight) in neighbors {
                        if visited_left.contains_key(&neighbor_id) {
                            continue;
                        }

                        if let Ok(Some(neighbor_vertex)) = self.get_vertex(&neighbor_id) {
                            let new_npath = Arc::new(NPath::extend(
                                current_npath.clone(),
                                Arc::new(edge.clone()),
                                Arc::new(neighbor_vertex),
                            ));
                            left_next.push((neighbor_id, new_npath.clone()));
                            visited_left.insert(neighbor_id, new_npath);
                        }
                    }
                }
            }

            // Extend left_queue with next level
            for (id, npath) in left_next.drain(..) {
                left_queue.push_back((id, npath));
            }

            if single_shortest && !result_paths.is_empty() {
                break;
            }
            if result_paths.len() >= limit {
                break;
            }

            // Process only nodes from the current BFS level (right side)
            let right_level_size = right_queue.len();
            let mut right_next: Vec<(VertexId, Arc<NPath>)> = Vec::new();
            for _ in 0..right_level_size {
                if let Some((current_id, current_npath)) = right_queue.pop_front() {
                    self.stats.increment_nodes_visited();

                    if visited_left.contains_key(&current_id) {
                        // This vertex was already reached from the left side.
                        // Combine the left path (start -> meeting) and right path (end -> meeting).
                        if let Some(left_npath) = visited_left.get(&current_id) {
                            let total_len = left_npath.len() + current_npath.len();
                            let exceeds_depth = max_depth.is_some_and(|max_d| total_len > max_d);
                            if !exceeds_depth {
                                if let Some(combined_path) =
                                    combine_npaths(left_npath, &current_npath)
                                {
                                    if !has_duplicate_edges(&combined_path) {
                                        result_paths.push(combined_path);
                                        if single_shortest || result_paths.len() >= limit {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    if let Some(max_d) = max_depth {
                        if current_npath.len() >= max_d {
                            continue;
                        }
                    }

                    let neighbors = self.get_neighbors_with_edges(
                        &current_id,
                        edge_types,
                        self.right_edge_direction,
                    )?;
                    self.stats.increment_edges_traversed(neighbors.len());

                    for (neighbor_id, edge, _weight) in neighbors {
                        if visited_right.contains_key(&neighbor_id) {
                            continue;
                        }

                        if let Ok(Some(neighbor_vertex)) = self.get_vertex(&neighbor_id) {
                            let new_npath = Arc::new(NPath::extend(
                                current_npath.clone(),
                                Arc::new(edge.clone()),
                                Arc::new(neighbor_vertex),
                            ));
                            right_next.push((neighbor_id, new_npath.clone()));
                            visited_right.insert(neighbor_id, new_npath);
                        }
                    }
                }
            }

            // After right expansion, check if any newly discovered vertex is in visited_left
            let mut combined_vertices: HashSet<VertexId> = HashSet::new();
            for (ref next_id, ref next_npath) in &right_next {
                if let Some(left_npath) = visited_left.get(next_id) {
                    let total_len = left_npath.len() + next_npath.len();
                    let exceeds_depth = max_depth.is_some_and(|max_d| total_len > max_d);
                    if !exceeds_depth {
                        if let Some(combined_path) = combine_npaths(left_npath, next_npath) {
                            if !has_duplicate_edges(&combined_path) {
                                result_paths.push(combined_path);
                                combined_vertices.insert(*next_id);
                                if single_shortest || result_paths.len() >= limit {
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // Extend right_queue with next level, skipping vertices already combined
            for (id, npath) in right_next.drain(..) {
                if !combined_vertices.contains(&id) {
                    right_queue.push_back((id, npath));
                }
            }

            if left_queue.is_empty() && right_queue.is_empty() {
                break;
            }
        }

        if single_shortest && !result_paths.is_empty() {
            result_paths.sort_by_key(|a| a.steps.len());
            result_paths.truncate(1);
        }

        if result_paths.len() > limit {
            result_paths.truncate(limit);
        }

        Ok(result_paths)
    }

    fn stats(&self) -> &AlgorithmStats {
        &self.stats
    }

    fn stats_mut(&mut self) -> &mut AlgorithmStats {
        &mut self.stats
    }
}

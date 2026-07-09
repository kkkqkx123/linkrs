//! Dijkstra's Shortest Path Algorithm
//!
//! Finding weighted shortest paths using Dijkstra's algorithm for binary heap optimization

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

use crate::core::types::VertexId;
use crate::core::{Edge, Path, Step, Vertex};
use crate::query::QueryError;
use crate::storage::StorageReader;
use parking_lot::RwLock;

use super::traits::ShortestPathAlgorithm;
use super::types::{
    has_duplicate_edges, AlgorithmStats, DistanceNode, EdgeWeightConfig, SelfLoopDedup,
};

/// Dijkstra's Shortest Path Algorithm
pub struct Dijkstra<S: StorageReader> {
    storage: Arc<RwLock<S>>,
    stats: AlgorithmStats,
    edge_direction: crate::core::types::EdgeDirection,
    weight_config: EdgeWeightConfig,
    space_name: String,
}

impl<S: StorageReader> Dijkstra<S> {
    pub fn new(storage: Arc<RwLock<S>>, space_name: String) -> Self {
        Self {
            storage,
            stats: AlgorithmStats::new(),
            edge_direction: crate::core::types::EdgeDirection::Both,
            weight_config: EdgeWeightConfig::Unweighted,
            space_name,
        }
    }

    pub fn with_edge_direction(mut self, direction: crate::core::types::EdgeDirection) -> Self {
        self.edge_direction = direction;
        self
    }

    pub fn with_weight_config(mut self, config: EdgeWeightConfig) -> Self {
        self.weight_config = config;
        self
    }

    /// Getting the weight of an edge
    fn get_edge_weight(&self, edge: &Edge) -> f64 {
        match &self.weight_config {
            EdgeWeightConfig::Unweighted => 1.0,
            EdgeWeightConfig::Ranking => edge.ranking as f64,
            EdgeWeightConfig::Property(prop_name) => edge
                .get_property(prop_name)
                .map(|v| match v {
                    crate::core::Value::SmallInt(i) => *i as f64,
                    crate::core::Value::Int(i) => *i as f64,
                    crate::core::Value::BigInt(i) => *i as f64,
                    crate::core::Value::Float(f) => (*f).into(),
                    crate::core::Value::Double(f) => *f,
                    _ => 1.0,
                })
                .unwrap_or(1.0),
        }
    }

    fn get_neighbors_with_edges(
        &self,
        node_id: &VertexId,
        edge_types: Option<&[String]>,
    ) -> Result<Vec<(VertexId, Edge, f64)>, QueryError> {
        let storage = self.storage.read();

        let edges = storage
            .get_node_edges(&self.space_name, node_id, self.edge_direction)
            .map_err(|e| QueryError::storage(e.to_string()))?;

        let filtered_edges = if let Some(types) = edge_types {
            edges
                .into_iter()
                .filter(|edge| types.contains(&edge.edge_type))
                .collect()
        } else {
            edges
        };

        let mut dedup = SelfLoopDedup::new();

        let neighbors_with_edges: Vec<(VertexId, Edge, f64)> = filtered_edges
            .into_iter()
            .filter(|edge| dedup.should_include(edge))
            .filter_map(|edge| {
                let neighbor_id = match self.edge_direction {
                    crate::core::types::EdgeDirection::In => {
                        if edge.dst == *node_id {
                            edge.src
                        } else {
                            return None;
                        }
                    }
                    crate::core::types::EdgeDirection::Out => {
                        if edge.src == *node_id {
                            edge.dst
                        } else {
                            return None;
                        }
                    }
                    crate::core::types::EdgeDirection::Both => {
                        if edge.src == *node_id {
                            edge.dst
                        } else if edge.dst == *node_id {
                            edge.src
                        } else {
                            return None;
                        }
                    }
                };
                let weight = self.get_edge_weight(&edge);
                Some((neighbor_id, edge, weight))
            })
            .collect();

        Ok(neighbors_with_edges)
    }

    fn get_vertex(&self, vid: &VertexId) -> Result<Option<Vertex>, QueryError> {
        let storage = self.storage.read();
        storage
            .get_vertex(&self.space_name, vid)
            .map_err(|e| QueryError::storage(e.to_string()))
    }

    fn reconstruct_path(
        &self,
        end_id: &VertexId,
        previous_map: &HashMap<VertexId, (VertexId, Edge)>,
        start_ids: &[VertexId],
    ) -> Result<Option<Path>, QueryError> {
        let mut path_edges: Vec<(VertexId, Edge)> = Vec::new();
        let mut current = *end_id;

        while let Some((prev_id, edge)) = previous_map.get(&current) {
            path_edges.push((current, edge.clone()));
            current = *prev_id;

            if start_ids.contains(&current) {
                let start_vertex = match self.get_vertex(&current)? {
                    Some(v) => v,
                    None => return Ok(None),
                };

                let mut path = Path {
                    src: Box::new(start_vertex),
                    steps: Vec::new(),
                };

                path_edges.reverse();
                for (dst_id, edge) in path_edges {
                    let dst_vertex = match self.get_vertex(&dst_id)? {
                        Some(v) => v,
                        None => return Ok(None),
                    };

                    path.steps.push(Step {
                        dst: Box::new(dst_vertex),
                        edge: Box::new(edge),
                    });
                }

                return Ok(Some(path));
            }
        }

        Ok(None)
    }
}

impl<S: StorageReader> ShortestPathAlgorithm for Dijkstra<S> {
    fn find_paths(
        &mut self,
        start_ids: &[VertexId],
        end_ids: &[VertexId],
        edge_types: Option<&[String]>,
        max_depth: Option<usize>,
        single_shortest: bool,
        limit: usize,
    ) -> Result<Vec<Path>, QueryError> {
        let mut distance_map: HashMap<VertexId, f64> = HashMap::new();
        let mut previous_map: HashMap<VertexId, (VertexId, Edge)> = HashMap::new();
        let mut visited_nodes: HashSet<VertexId> = HashSet::new();
        let mut priority_queue: BinaryHeap<Reverse<DistanceNode>> = BinaryHeap::new();

        for start_id in start_ids {
            distance_map.insert(*start_id, 0.0);
            priority_queue.push(Reverse(DistanceNode {
                distance: 0.0,
                vertex_id: *start_id,
            }));
        }

        let mut result_paths = Vec::new();

        while let Some(Reverse(current)) = priority_queue.pop() {
            if single_shortest && !result_paths.is_empty() {
                break;
            }

            if result_paths.len() >= limit {
                break;
            }

            if visited_nodes.contains(&current.vertex_id) {
                continue;
            }
            visited_nodes.insert(current.vertex_id);
            self.stats.increment_nodes_visited();

            if end_ids.contains(&current.vertex_id) {
                if let Some(path) =
                    self.reconstruct_path(&current.vertex_id, &previous_map, start_ids)?
                {
                    if !has_duplicate_edges(&path) {
                        result_paths.push(path);
                    }
                }
                continue;
            }

            if let Some(max_d) = max_depth {
                if current.distance as usize >= max_d {
                    continue;
                }
            }

            let neighbors = self.get_neighbors_with_edges(&current.vertex_id, edge_types)?;
            self.stats.increment_edges_traversed(neighbors.len());

            for (neighbor_id, edge, weight) in neighbors {
                if visited_nodes.contains(&neighbor_id) {
                    continue;
                }

                let new_distance = current.distance + weight;
                let existing_distance = distance_map.get(&neighbor_id).unwrap_or(&f64::INFINITY);

                if new_distance < *existing_distance {
                    distance_map.insert(neighbor_id, new_distance);
                    previous_map.insert(neighbor_id, (current.vertex_id, edge.clone()));
                    priority_queue.push(Reverse(DistanceNode {
                        distance: new_distance,
                        vertex_id: neighbor_id,
                    }));
                }
            }
        }

        if single_shortest && !result_paths.is_empty() {
            result_paths.sort_by(|a, b| {
                let weight_a: f64 = a.steps.iter().map(|s| self.get_edge_weight(&s.edge)).sum();
                let weight_b: f64 = b.steps.iter().map(|s| self.get_edge_weight(&s.edge)).sum();
                weight_a
                    .partial_cmp(&weight_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
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

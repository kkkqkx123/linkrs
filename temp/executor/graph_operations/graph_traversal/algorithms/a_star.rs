//! A* shortest path algorithm
//!
//! A* search algorithm using heuristic functions with support for weighted graphs and multi-terminal queries

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

use crate::core::types::VertexId;
use crate::core::{Edge, Path, Step, Value, Vertex};
use crate::query::QueryError;
use crate::storage::StorageReader;
use parking_lot::RwLock;

use super::traits::ShortestPathAlgorithm;
use super::types::{
    has_duplicate_edges, AlgorithmStats, EdgeWeightConfig, HeuristicFunction, SelfLoopDedup,
};

/// A* algorithm node
#[derive(Debug, Clone)]
pub struct AStarNode {
    /// Actual cost from the starting point to the current node
    pub g_cost: f64,
    /// Heuristic Estimated Costs (Estimates to Endpoints)
    pub h_cost: f64,
    /// Total cost = g_cost + h_cost
    pub f_cost: f64,
    pub vertex_id: VertexId,
}

impl Eq for AStarNode {}

impl PartialEq for AStarNode {
    fn eq(&self, other: &Self) -> bool {
        self.f_cost == other.f_cost && self.vertex_id == other.vertex_id
    }
}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .f_cost
            .partial_cmp(&self.f_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A* shortest path algorithm
pub struct AStar<S: StorageReader> {
    storage: Arc<RwLock<S>>,
    stats: AlgorithmStats,
    edge_direction: crate::core::types::EdgeDirection,
    /// weighting
    weight_config: EdgeWeightConfig,
    /// Heuristic function configuration
    heuristic_config: HeuristicFunction,
    space_name: String,
}

impl<S: StorageReader> AStar<S> {
    pub fn new(storage: Arc<RwLock<S>>, space_name: String) -> Self {
        Self {
            storage,
            stats: AlgorithmStats::new(),
            edge_direction: crate::core::types::EdgeDirection::Both,
            weight_config: EdgeWeightConfig::Unweighted,
            heuristic_config: HeuristicFunction::Zero,
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

    pub fn with_heuristic(mut self, heuristic: HeuristicFunction) -> Self {
        self.heuristic_config = heuristic;
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

    /// Calculating heuristic values
    /// For multiple endpoints, the heuristic value to the nearest endpoint is used
    fn calculate_heuristic(
        &self,
        current_id: &VertexId,
        end_ids: &[VertexId],
    ) -> Result<f64, QueryError> {
        if self.heuristic_config.is_zero() {
            return Ok(0.0);
        }

        let current_value = Value::from(*current_id);
        let current_props = self.get_vertex_props(current_id)?;

        let min_h = end_ids
            .iter()
            .filter_map(|end_id| {
                let end_value = Value::from(*end_id);
                let end_props = self.get_vertex_props(end_id).ok()?;
                Some(self.heuristic_config.evaluate(
                    &current_value,
                    &end_value,
                    current_props.as_ref(),
                    end_props.as_ref(),
                ))
            })
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(0.0);

        Ok(min_h)
    }

    /// Get vertex properties
    fn get_vertex_props(
        &self,
        vid: &VertexId,
    ) -> Result<Option<std::collections::HashMap<String, crate::core::Value>>, QueryError> {
        let storage = self.storage.read();
        storage
            .get_vertex(&self.space_name, vid)
            .map(|v| v.map(|vertex| vertex.properties))
            .map_err(|e| QueryError::storage(e.to_string()))
    }

    /// Get neighbor nodes and edges
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

    /// Get Vertex
    fn get_vertex(&self, vid: &VertexId) -> Result<Option<Vertex>, QueryError> {
        let storage = self.storage.read();
        storage
            .get_vertex(&self.space_name, vid)
            .map_err(|e| QueryError::storage(e.to_string()))
    }

    /// Reconstructing paths based on precursor mapping
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

impl<S: StorageReader> ShortestPathAlgorithm for AStar<S> {
    fn find_paths(
        &mut self,
        start_ids: &[VertexId],
        end_ids: &[VertexId],
        edge_types: Option<&[String]>,
        max_depth: Option<usize>,
        single_shortest: bool,
        limit: usize,
    ) -> Result<Vec<Path>, QueryError> {
        let mut g_cost_map: HashMap<VertexId, f64> = HashMap::new();
        let mut previous_map: HashMap<VertexId, (VertexId, Edge)> = HashMap::new();
        let mut closed_set: HashSet<VertexId> = HashSet::new();
        let mut open_set: HashSet<VertexId> = HashSet::new();
        let mut priority_queue: BinaryHeap<Reverse<AStarNode>> = BinaryHeap::new();

        // Initialization starting point
        for start_id in start_ids {
            let h_cost = self.calculate_heuristic(start_id, end_ids)?;

            g_cost_map.insert(*start_id, 0.0);
            open_set.insert(*start_id);
            priority_queue.push(Reverse(AStarNode {
                g_cost: 0.0,
                h_cost,
                f_cost: h_cost,
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

            if closed_set.contains(&current.vertex_id) {
                continue;
            }

            closed_set.insert(current.vertex_id);
            open_set.remove(&current.vertex_id);
            self.stats.increment_nodes_visited();

            // Check to see if you've reached the end of the line
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

            // Check Depth Limit
            if let Some(max_d) = max_depth {
                if current.g_cost as usize >= max_d {
                    continue;
                }
            }

            // Extended Neighborhood
            let neighbors = self.get_neighbors_with_edges(&current.vertex_id, edge_types)?;
            self.stats.increment_edges_traversed(neighbors.len());

            for (neighbor_id, edge, weight) in neighbors {
                if closed_set.contains(&neighbor_id) {
                    continue;
                }

                let tentative_g_cost = current.g_cost + weight;
                let existing_g_cost = g_cost_map.get(&neighbor_id).unwrap_or(&f64::INFINITY);

                if tentative_g_cost < *existing_g_cost {
                    g_cost_map.insert(neighbor_id, tentative_g_cost);
                    previous_map.insert(neighbor_id, (current.vertex_id, edge.clone()));

                    let h_cost = self.calculate_heuristic(&neighbor_id, end_ids)?;
                    let f_cost = tentative_g_cost + h_cost;

                    priority_queue.push(Reverse(AStarNode {
                        g_cost: tentative_g_cost,
                        h_cost,
                        f_cost,
                        vertex_id: neighbor_id,
                    }));
                    open_set.insert(current.vertex_id);
                }
            }
        }

        if single_shortest && !result_paths.is_empty() {
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

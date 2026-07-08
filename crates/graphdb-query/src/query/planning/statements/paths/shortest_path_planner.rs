//! Shortest Path Planner
//!
//! Responsible for planning shortest path queries, supporting algorithms such as BFS (Breadth-First Search).

use crate::core::types::graph_schema::EdgeDirection;
use crate::core::types::VertexId;
use crate::core::{Edge, StorageError, Value, Vertex};
use crate::query::planning::statements::seeks::seek_strategy_base::{
    NodePattern, SeekStrategyContext, SeekStrategySelector, SeekStrategyType,
};
use crate::storage::StorageReader;
use std::collections::{HashMap, HashSet, VecDeque};

pub type PlannerError = StorageError;

#[derive(Debug)]
pub struct ShortestPathPlanner;

impl Default for ShortestPathPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl ShortestPathPlanner {
    pub fn new() -> Self {
        Self
    }

    pub fn plan_shortest_path(
        &self,
        start: &NodePattern,
        end: &NodePattern,
        edge_pattern: &EdgePattern,
        space_id: u64,
    ) -> Result<ShortestPathPlan, PlannerError> {
        let bfs_config = BfsConfig {
            max_iterations: 10000,
            max_path_length: 100,
            direction: self.extract_direction(edge_pattern)?,
            edge_types: edge_pattern.types.clone().unwrap_or_default(),
        };

        let start_context = SeekStrategyContext::new(space_id, start.clone(), vec![]);
        let selector = SeekStrategySelector::new();
        let start_strategy = selector.select_strategy(&start_context);

        let start_finder = match start_strategy {
            SeekStrategyType::VertexSeek => StartVidSource::VertexSeek(start.clone()),
            SeekStrategyType::IndexSeek => StartVidSource::IndexScan(start.clone()),
            SeekStrategyType::PropIndexSeek => StartVidSource::PropIndexScan(start.clone()),
            SeekStrategyType::VariablePropIndexSeek => {
                StartVidSource::VariablePropIndexScan(start.clone())
            }
            SeekStrategyType::EdgeSeek => StartVidSource::EdgeScan(start.clone()),
            SeekStrategyType::ScanSeek => StartVidSource::FullScan(start.clone()),
        };

        let end_condition = EndCondition {
            pattern: end.clone(),
        };

        Ok(ShortestPathPlan {
            start: start_finder,
            end: end_condition,
            bfs_config,
        })
    }

    fn extract_direction(&self, edge: &EdgePattern) -> Result<EdgeDirection, PlannerError> {
        Ok(match edge.direction {
            Some(ref dir) => *dir,
            None => EdgeDirection::Both,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EdgePattern {
    pub types: Option<Vec<String>>,
    pub direction: Option<EdgeDirection>,
    pub properties: Vec<(String, Value)>,
}

#[derive(Debug)]
pub enum StartVidSource {
    VertexSeek(NodePattern),
    IndexScan(NodePattern),
    PropIndexScan(NodePattern),
    VariablePropIndexScan(NodePattern),
    EdgeScan(NodePattern),
    FullScan(NodePattern),
}

#[derive(Debug, Clone)]
pub struct EndCondition {
    pub pattern: NodePattern,
}

#[derive(Debug)]
pub struct BfsConfig {
    pub max_iterations: usize,
    pub max_path_length: usize,
    pub direction: EdgeDirection,
    pub edge_types: Vec<String>,
}

#[derive(Debug)]
pub struct ShortestPathPlan {
    pub start: StartVidSource,
    pub end: EndCondition,
    pub bfs_config: BfsConfig,
}

#[derive(Debug)]
pub struct ShortestPathResult {
    pub paths: Vec<ShortestPath>,
    pub nodes_visited: usize,
    pub edges_explored: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShortestPath {
    pub vertices: Vec<Value>,
    pub edges: Vec<Edge>,
}

impl ShortestPathPlanner {
    pub fn find_shortest_path<S: StorageReader>(
        &self,
        storage: &S,
        plan: &ShortestPathPlan,
    ) -> Result<ShortestPathResult, PlannerError> {
        let start_vids = self.resolve_start_vids(storage, &plan.start)?;
        let end_pattern = &plan.end.pattern;

        let mut all_paths = Vec::new();
        let mut total_nodes_visited = 0;
        let mut total_edges_explored = 0;

        for start_vid in start_vids {
            if let Some(end_vid) = self.resolve_end_vid(storage, end_pattern)? {
                match self.bfs_search(storage, &start_vid, &end_vid, &plan.bfs_config) {
                    Ok(Some(path)) => {
                        total_nodes_visited += path.vertices.len();
                        total_edges_explored += path.edges.len();
                        all_paths.push(path);
                    }
                    Ok(None) => continue,
                    Err(e) => return Err(e),
                }
            }
        }

        all_paths.sort_by_key(|a| a.vertices.len());

        Ok(ShortestPathResult {
            paths: all_paths,
            nodes_visited: total_nodes_visited,
            edges_explored: total_edges_explored,
        })
    }

    fn resolve_start_vids<S: StorageReader>(
        &self,
        storage: &S,
        start: &StartVidSource,
    ) -> Result<Vec<Value>, PlannerError> {
        match start {
            StartVidSource::VertexSeek(pattern) => match &pattern.vid {
                Some(vid) => {
                    let vid_clone: Value = vid.clone();
                    Ok(vec![vid_clone])
                }
                None => self.scan_matching_vertices(storage, pattern),
            },
            StartVidSource::IndexScan(pattern) => self.scan_matching_vertices(storage, pattern),
            StartVidSource::PropIndexScan(pattern) => self.scan_matching_vertices(storage, pattern),
            StartVidSource::VariablePropIndexScan(pattern) => {
                self.scan_matching_vertices(storage, pattern)
            }
            StartVidSource::EdgeScan(pattern) => self.scan_matching_vertices(storage, pattern),
            StartVidSource::FullScan(pattern) => self.scan_matching_vertices(storage, pattern),
        }
    }

    fn resolve_end_vid<S: StorageReader>(
        &self,
        storage: &S,
        pattern: &NodePattern,
    ) -> Result<Option<Value>, PlannerError> {
        match &pattern.vid {
            Some(vid) => {
                let vid_clone: Value = vid.clone();
                Ok(Some(vid_clone))
            }
            None => {
                let vertices = self.scan_matching_vertices(storage, pattern)?;
                Ok(vertices.first().cloned())
            }
        }
    }

    fn scan_matching_vertices<S: StorageReader>(
        &self,
        storage: &S,
        pattern: &NodePattern,
    ) -> Result<Vec<Value>, PlannerError> {
        let vertices = storage.scan_vertices("default")?;
        let mut matching: Vec<Value> = Vec::new();

        for vertex in vertices {
            if self.vertex_matches_pattern(&vertex, pattern) {
                matching.push(Value::from(*vertex.vid()));
            }
        }

        Ok(matching)
    }

    fn vertex_matches_pattern(&self, vertex: &Vertex, pattern: &NodePattern) -> bool {
        if !pattern.labels.is_empty() {
            let has_all_labels = pattern
                .labels
                .iter()
                .all(|label| vertex.tags.iter().any(|tag| tag.name == *label));
            if !has_all_labels {
                return false;
            }
        }

        for (prop_name, prop_value) in &pattern.properties {
            let found = vertex
                .get_all_properties()
                .iter()
                .any(|(name, value)| name == prop_name && **value == *prop_value);
            if !found {
                return false;
            }
        }

        true
    }

    fn bfs_search<S: StorageReader>(
        &self,
        storage: &S,
        start: &Value,
        end: &Value,
        config: &BfsConfig,
    ) -> Result<Option<ShortestPath>, StorageError> {
        let mut queue = VecDeque::new();
        let mut visited: HashSet<Value> = HashSet::new();
        let mut parent_map: HashMap<Value, (Value, Edge)> = HashMap::new();

        queue.push_back(start.clone());
        visited.insert(start.clone());

        while let Some(current) = queue.pop_front() {
            if current == *end {
                return Ok(self.reconstruct_path(start, end, &parent_map));
            }

            if parent_map.len() >= config.max_iterations {
                continue;
            }

            let current_vid = VertexId::try_from(&current)
                .map_err(|e| StorageError::invalid_input(e.to_string()))?;
            let edges = storage.get_node_edges("default", &current_vid, config.direction)?;

            for edge in edges {
                let neighbor = if Value::from(edge.src) == current {
                    Value::from(edge.dst)
                } else {
                    Value::from(edge.src)
                };

                if !config.edge_types.is_empty() && !config.edge_types.contains(&edge.edge_type) {
                    continue;
                }

                if visited.insert(neighbor.clone()) {
                    parent_map.insert(neighbor.clone(), (current.clone(), edge.clone()));
                    queue.push_back(neighbor);
                }
            }
        }

        Ok(None)
    }

    fn reconstruct_path(
        &self,
        start: &Value,
        end: &Value,
        parent_map: &HashMap<Value, (Value, Edge)>,
    ) -> Option<ShortestPath> {
        let mut vertices = Vec::new();
        let mut edges = Vec::new();
        let mut current = end.clone();

        let mut path = Vec::new();
        path.push(current.clone());

        while let Some((parent, edge)) = parent_map.get(&current) {
            vertices.push(current.clone());
            edges.push(edge.clone());
            current = parent.clone();
            path.push(current.clone());
        }

        vertices.push(start.clone());
        vertices.reverse();
        edges.reverse();

        Some(ShortestPath { vertices, edges })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shortest_path_planner_new() {
        let _planner = ShortestPathPlanner::new();
        // The test has been successful; reaching this point indicates that the goal has been achieved.
    }

    #[test]
    fn test_edge_pattern() {
        let _pattern = EdgePattern {
            types: Some(vec!["follows".to_string()]),
            direction: Some(EdgeDirection::Out),
            properties: vec![],
        };
        // The test has been successful; reaching this point indicates that the goal has been achieved.
    }
}

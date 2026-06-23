//! Subgraph Query Executor
//!
//! Support for obtaining a subgraph with a specified starting point within a given number of steps.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::core::error::{DBError, DBResult};
use crate::core::types::VertexId;
use crate::core::{Edge, Path, Value, Vertex};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{
    BaseExecutor, DBResult as ExecDBResult, EdgeDirection, ExecutionResult,
    Executor as BaseExecutorTrait, ExecutorStats, HasStorage, InputExecutor,
};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;
use parking_lot::RwLock;

use super::types::AlgorithmStats;

/// Subgraph query configuration
#[derive(Debug, Clone)]
pub struct SubgraphConfig {
    /// Maximum number of steps
    pub steps: usize,
    /// Side direction
    pub edge_direction: EdgeDirection,
    /// Edge type filtering
    pub edge_types: Option<Vec<String>>,
    /// Bidirectional edge type (used for handling bidirectional edges)
    pub bidirect_edge_types: Option<HashSet<String>>,
    /// Edge filtering criteria
    pub edge_filter: Option<String>,
    /// Vertex filtering criteria
    pub vertex_filter: Option<String>,
    /// Does it contain attributes?
    pub with_properties: bool,
    /// Result limitations
    pub limit: Option<usize>,
    /// Space name for edge lookups
    pub space_name: String,
}

impl Default for SubgraphConfig {
    fn default() -> Self {
        Self {
            steps: 1,
            edge_direction: EdgeDirection::Out,
            edge_types: None,
            bidirect_edge_types: None,
            edge_filter: None,
            vertex_filter: None,
            with_properties: true,
            limit: None,
            space_name: "default".to_string(),
        }
    }
}

impl SubgraphConfig {
    pub fn new(steps: usize) -> Self {
        Self {
            steps,
            ..Default::default()
        }
    }

    pub fn with_direction(mut self, direction: EdgeDirection) -> Self {
        self.edge_direction = direction;
        self
    }

    pub fn with_edge_types(mut self, edge_types: Vec<String>) -> Self {
        self.edge_types = Some(edge_types);
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Results of the subgraph query
#[derive(Debug, Clone)]
pub struct SubgraphResult {
    pub vertices: HashMap<VertexId, Vertex>,
    pub edges: Vec<Edge>,
    pub visited_vids: HashSet<VertexId>,
    pub stats: AlgorithmStats,
}

impl SubgraphResult {
    pub fn new() -> Self {
        Self {
            vertices: HashMap::new(),
            edges: Vec::new(),
            visited_vids: HashSet::new(),
            stats: AlgorithmStats::new(),
        }
    }

    pub fn to_paths(&self) -> Vec<Path> {
        let mut paths = Vec::new();

        for edge in &self.edges {
            if let Some(src_vertex) = self.vertices.get(&edge.src) {
                let mut path = Path::new(src_vertex.clone());
                let dst_vertex = self
                    .vertices
                    .get(&edge.dst)
                    .cloned()
                    .unwrap_or_else(|| Vertex::with_vid(edge.dst));
                path.steps.push(crate::core::Step::new(
                    dst_vertex,
                    edge.edge_type.clone(),
                    edge.edge_type.clone(),
                    edge.ranking,
                ));
                paths.push(path);
            }
        }

        paths
    }
}

impl Default for SubgraphResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Subgraph Query Executor
pub struct SubgraphExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    start_vids: Vec<VertexId>,
    config: SubgraphConfig,
    current_step: usize,
    history_vids: HashMap<VertexId, usize>,
    current_vids: HashSet<VertexId>,
    valid_vids: HashSet<VertexId>,
    next_vids: Vec<VertexId>,
    result: SubgraphResult,
    stats: AlgorithmStats,
}

impl<S: StorageClient> std::fmt::Debug for SubgraphExecutor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubgraphExecutor")
            .field("base", &"BaseExecutor")
            .field("start_vids", &self.start_vids)
            .field("config", &self.config)
            .field("current_step", &self.current_step)
            .field("history_vids_count", &self.history_vids.len())
            .field("valid_vids_count", &self.valid_vids.len())
            .finish()
    }
}

impl<S: StorageClient> SubgraphExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        start_vids: Vec<VertexId>,
        config: SubgraphConfig,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        let valid_vids: HashSet<VertexId> = start_vids.iter().copied().collect();

        Self {
            base: BaseExecutor::new(id, "SubgraphExecutor".to_string(), storage, expr_context),
            start_vids: start_vids.clone(),
            config,
            current_step: 1,
            history_vids: HashMap::new(),
            current_vids: HashSet::new(),
            valid_vids,
            next_vids: start_vids,
            result: SubgraphResult::new(),
            stats: AlgorithmStats::new(),
        }
    }

    fn get_neighbors(&self, node_id: &VertexId) -> DBResult<Vec<(VertexId, Edge)>> {
        let storage = self
            .base
            .storage
            .as_ref()
            .ok_or_else(|| DBError::storage("Storage not set".to_string()))?;
        let storage = storage.read();

        let edges = storage
            .get_node_edges(&self.config.space_name, node_id, self.config.edge_direction)
            .map_err(|e| DBError::storage(e.to_string()))?;

        let filtered_edges = if let Some(ref edge_types) = self.config.edge_types {
            edges
                .into_iter()
                .filter(|edge| edge_types.contains(&edge.edge_type))
                .collect()
        } else {
            edges
        };

        let neighbors: Vec<(VertexId, Edge)> = filtered_edges
            .into_iter()
            .filter_map(|edge| {
                if edge.src == *node_id {
                    Some((edge.dst, edge))
                } else if edge.dst == *node_id && self.config.edge_direction == EdgeDirection::Both
                {
                    Some((edge.src, edge))
                } else {
                    None
                }
            })
            .collect();

        Ok(neighbors)
    }

    /// Handling single-step extensions
    fn expand_step(&mut self) -> DBResult<bool> {
        if self.next_vids.is_empty() || self.current_step > self.config.steps {
            return Ok(false);
        }

        self.current_vids.clear();
        let current_step_vids: Vec<VertexId> = self.next_vids.drain(..).collect();

        for vid in current_step_vids {
            // Skip the already visited vertices (unless it is a bidirectional edge and special handling is required).
            if let Some(&visited_step) = self.history_vids.get(&vid) {
                if self.config.bidirect_edge_types.is_none() {
                    continue;
                }
                // Special handling of bidirectional edges: Check whether they were accessed in the previous two steps.
                if visited_step + 2 != self.current_step {
                    continue;
                }
            }

            let neighbors = self.get_neighbors(&vid)?;

            for (neighbor_id, edge) in neighbors {
                // Add edges to the result.
                self.result.edges.push(edge);

                // Add the target vertex to the set of valid vertices.
                self.valid_vids.insert(neighbor_id);

                // If it’s not the last step, add it to the list of steps to be visited next.
                if self.current_step < self.config.steps && self.current_vids.insert(neighbor_id) {
                    self.next_vids.push(neighbor_id);
                }
            }
        }

        // Update the history record
        for vid in &self.current_vids {
            self.history_vids.insert(*vid, self.current_step);
        }

        self.current_step += 1;

        // Check whether it is necessary to proceed further.
        Ok(!self.next_vids.is_empty() && self.current_step <= self.config.steps)
    }

    /// Obtain detailed information about the vertices
    fn fetch_vertices(&mut self) -> DBResult<()> {
        let storage = self
            .base
            .storage
            .as_ref()
            .ok_or_else(|| DBError::storage("Storage not set".to_string()))?;
        let storage = storage.read();

        for vid in &self.valid_vids {
            match storage.get_vertex(&self.config.space_name, vid) {
                Ok(Some(vertex)) => {
                    self.result.vertices.insert(*vid, vertex);
                }
                Ok(None) => {
                    // The vertex does not exist; create a vertex that contains only the VID (Vertex Identifier).
                    let vertex = Vertex::with_vid(*vid);
                    self.result.vertices.insert(*vid, vertex);
                }
                Err(e) => {
                    return Err(DBError::storage(e.to_string()));
                }
            }
        }

        Ok(())
    }

    /// Filtering edges (removing edges that point to invalid vertices)
    fn filter_edges(&mut self) {
        self.result.edges.retain(|edge| {
            self.valid_vids.contains(&edge.src) && self.valid_vids.contains(&edge.dst)
        });
    }

    /// Execute a subgraph query
    pub fn execute_subgraph(&mut self) -> DBResult<SubgraphResult> {
        let start_time = Instant::now();

        // Perform multi-step expansion
        while self.expand_step()? {}

        // Obtain detailed information about the vertices
        if self.config.with_properties {
            self.fetch_vertices()?;
        } else {
            // Please provide the specific text you would like to have translated. Once I have the text, I can assist you with the translation.
            for vid in &self.valid_vids {
                let vertex = Vertex::with_vid(*vid);
                self.result.vertices.insert(*vid, vertex);
            }
        }

        // Filter edges
        self.filter_edges();

        // Application restrictions
        if let Some(limit) = self.config.limit {
            if self.result.edges.len() > limit {
                self.result.edges.truncate(limit);
            }
        }

        self.stats
            .set_execution_time(start_time.elapsed().as_millis() as u64);
        self.result.stats = self.stats.clone();
        self.result.visited_vids = self.valid_vids.clone();

        Ok(self.result.clone())
    }

    /// Obtain the result path
    pub fn get_result_paths(&self) -> Vec<Path> {
        self.result.to_paths()
    }
}

impl<S: StorageClient + Send + 'static> BaseExecutorTrait<S> for SubgraphExecutor<S> {
    fn execute(&mut self) -> ExecDBResult<ExecutionResult> {
        let result = self
            .execute_subgraph()
            .map_err(|e| DBError::query(e.to_string()))?;

        let paths = result.to_paths();
        let rows: Vec<Vec<Value>> = paths.into_iter().map(|p| vec![Value::path(p)]).collect();
        let dataset = DataSet::from_rows(rows, vec!["path".to_string()]);
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> ExecDBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> ExecDBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        "Subgraph executor - retrieves subgraph within specified steps"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for SubgraphExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for SubgraphExecutor<S> {
    fn set_input(&mut self, _input: ExecutorEnum<S>) {
        // Subgraph queries do not require any input.
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;
    use crate::storage::MockStorage;

    #[test]
    fn test_subgraph_config_default() {
        let config = SubgraphConfig::default();
        assert_eq!(config.steps, 1);
        assert_eq!(config.edge_direction, EdgeDirection::Out);
        assert!(config.edge_types.is_none());
        assert!(config.limit.is_none());
    }

    #[test]
    fn test_subgraph_config_builder() {
        let config = SubgraphConfig::new(3)
            .with_direction(EdgeDirection::Both)
            .with_edge_types(vec!["knows".to_string()])
            .with_limit(100);

        assert_eq!(config.steps, 3);
        assert_eq!(config.edge_direction, EdgeDirection::Both);
        assert_eq!(config.edge_types, Some(vec!["knows".to_string()]));
        assert_eq!(config.limit, Some(100));
    }

    #[test]
    fn test_subgraph_executor_creation() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let config = SubgraphConfig::new(2);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let executor =
            SubgraphExecutor::new(1, storage, vec![VertexId::from_string("a")], config, expr_context);

        assert_eq!(executor.start_vids.len(), 1);
        assert_eq!(executor.config.steps, 2);
        assert_eq!(executor.valid_vids.len(), 1);
    }

    #[test]
    fn test_subgraph_result() {
        let mut result = SubgraphResult::new();

        // Add some vertices.
        result
            .vertices
            .insert(VertexId::from_string("a"), Vertex::with_vid(VertexId::from_string("a")));
        result
            .vertices
            .insert(VertexId::from_string("b"), Vertex::with_vid(VertexId::from_string("b")));

        // Add an edge.
        let edge = Edge::new(
            VertexId::from_string("a"),
            VertexId::from_string("b"),
            "knows".to_string(),
            0,
            HashMap::new(),
        );
        result.edges.push(edge);

        // Converting the test result into a path
        let paths = result.to_paths();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].steps.len(), 1);
    }
}

//! Shortest Path Executor
//!
//! Implement the search for the shortest path using a specific algorithm from the algorithm module.
//! Responsible for the lifecycle management of actuators and the scheduling of algorithms

use std::sync::Arc;

use crate::core::error::DBResult;
use crate::core::types::VertexId;
use crate::core::{Path, Value};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{
    BaseExecutor, EdgeDirection, ExecutorConfig, InputExecutor, ShortestPathConfig,
};
use crate::query::executor::base::{ExecutionResult, Executor, HasStorage};
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;
use parking_lot::RwLock;

// Introducing the algorithm module
use super::algorithms::{
    AStar, AlgorithmStats, BidirectionalBFS, Dijkstra, EdgeWeightConfig, HeuristicFunction,
    ShortestPathAlgorithm, ShortestPathAlgorithmType,
};

/// Shortest Path Executor
///
/// Responsible for managing the execution lifecycle of shortest path queries and invoking the specific algorithms for their implementation.
pub struct ShortestPathExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    start_vertex_ids: Vec<VertexId>,
    end_vertex_ids: Vec<VertexId>,
    pub edge_direction: EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub max_depth: Option<usize>,
    algorithm_type: ShortestPathAlgorithmType,
    weight_config: EdgeWeightConfig,
    heuristic_config: HeuristicFunction,
    input_executor: Option<Box<ExecutorEnum<S>>>,
    pub shortest_paths: Vec<Path>,
    pub nodes_visited: usize,
    pub edges_traversed: usize,
    pub execution_time_ms: u64,
    pub max_depth_reached: usize,
    pub single_shortest: bool,
    pub limit: usize,
    space_name: String,
}

impl<S: StorageClient> std::fmt::Debug for ShortestPathExecutor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShortestPathExecutor")
            .field("base", &"BaseExecutor")
            .field("start_vertex_ids", &self.start_vertex_ids)
            .field("end_vertex_ids", &self.end_vertex_ids)
            .field("edge_direction", &self.edge_direction)
            .field("edge_types", &self.edge_types)
            .field("max_depth", &self.max_depth)
            .field("algorithm", &self.algorithm_type)
            .field("single_shortest", &self.single_shortest)
            .field("limit", &self.limit)
            .field("shortest_paths", &self.shortest_paths)
            .field("nodes_visited", &self.nodes_visited)
            .field("edges_traversed", &self.edges_traversed)
            .finish()
    }
}

impl<S: StorageClient> ShortestPathExecutor<S> {
    pub fn new(
        base_config: ExecutorConfig<S>,
        config: ShortestPathConfig,
        algorithm: ShortestPathAlgorithmType,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "ShortestPathExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            start_vertex_ids: config.start_vertex_ids,
            end_vertex_ids: vec![],
            edge_direction: config.direction,
            edge_types: config.edge_types,
            max_depth: None,
            algorithm_type: algorithm,
            weight_config: EdgeWeightConfig::Unweighted,
            heuristic_config: HeuristicFunction::Zero,
            input_executor: None,
            shortest_paths: Vec::new(),
            nodes_visited: 0,
            edges_traversed: 0,
            execution_time_ms: 0,
            max_depth_reached: 0,
            single_shortest: false,
            limit: usize::MAX,
            space_name: config.space_name,
        }
    }

    pub fn with_limits(mut self, single_shortest: bool, limit: usize) -> Self {
        self.single_shortest = single_shortest;
        self.limit = limit;
        self
    }

    pub fn with_weight_config(mut self, config: EdgeWeightConfig) -> Self {
        self.weight_config = config;
        self
    }

    pub fn with_heuristic_config(mut self, config: HeuristicFunction) -> Self {
        self.heuristic_config = config;
        self
    }

    pub fn get_algorithm(&self) -> ShortestPathAlgorithmType {
        self.algorithm_type.clone()
    }

    pub fn set_algorithm(&mut self, algorithm: ShortestPathAlgorithmType) {
        self.algorithm_type = algorithm;
    }

    pub fn get_start_vertex_ids(&self) -> &Vec<VertexId> {
        &self.start_vertex_ids
    }

    pub fn get_end_vertex_ids(&self) -> &Vec<VertexId> {
        &self.end_vertex_ids
    }

    pub fn set_start_vertex_ids(&mut self, ids: Vec<VertexId>) {
        self.start_vertex_ids = ids;
    }

    pub fn set_end_vertex_ids(&mut self, ids: Vec<VertexId>) {
        self.end_vertex_ids = ids;
    }

    /// Implement the shortest path algorithm
    fn execute_algorithm(&mut self) -> Result<Vec<Path>, QueryError> {
        let storage = self.base.storage.clone().expect("Storage not initialized");
        let edge_types = self.edge_types.as_deref();
        let space_name = &self.space_name;

        match self.algorithm_type {
            ShortestPathAlgorithmType::BFS => {
                let mut algorithm = BidirectionalBFS::new(storage.clone(), space_name.clone())
                    .with_edge_direction(self.edge_direction);
                let paths = algorithm.find_paths(
                    &self.start_vertex_ids,
                    &self.end_vertex_ids,
                    edge_types,
                    self.max_depth,
                    self.single_shortest,
                    self.limit,
                )?;
                self.update_stats(algorithm.stats());
                Ok(paths)
            }
            ShortestPathAlgorithmType::Dijkstra => {
                let mut algorithm = Dijkstra::new(storage.clone(), space_name.clone())
                    .with_edge_direction(self.edge_direction)
                    .with_weight_config(self.weight_config.clone());
                let paths = algorithm.find_paths(
                    &self.start_vertex_ids,
                    &self.end_vertex_ids,
                    edge_types,
                    self.max_depth,
                    self.single_shortest,
                    self.limit,
                )?;
                self.update_stats(algorithm.stats());
                Ok(paths)
            }
            ShortestPathAlgorithmType::AStar => {
                let mut algorithm = AStar::new(storage.clone(), space_name.clone())
                    .with_edge_direction(self.edge_direction)
                    .with_weight_config(self.weight_config.clone())
                    .with_heuristic(self.heuristic_config.clone());
                let paths = algorithm.find_paths(
                    &self.start_vertex_ids,
                    &self.end_vertex_ids,
                    edge_types,
                    self.max_depth,
                    self.single_shortest,
                    self.limit,
                )?;
                self.update_stats(algorithm.stats());
                Ok(paths)
            }
        }
    }

    /// Update executor statistics.
    fn update_stats(&mut self, algorithm_stats: &AlgorithmStats) {
        self.nodes_visited = algorithm_stats.nodes_visited;
        self.edges_traversed = algorithm_stats.edges_traversed;
        self.execution_time_ms = algorithm_stats.execution_time_ms;
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for ShortestPathExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for ShortestPathExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start_time = std::time::Instant::now();

        let paths = self.execute_algorithm()?;

        self.execution_time_ms = start_time.elapsed().as_millis() as u64;
        self.shortest_paths = paths.clone();

        // Update the maximum depth.
        for path in &paths {
            if path.steps.len() > self.max_depth_reached {
                self.max_depth_reached = path.steps.len();
            }
        }

        let rows: Vec<Vec<Value>> = paths.into_iter().map(|p| vec![Value::path(p)]).collect();
        let dataset = DataSet::from_rows(rows, vec!["path".to_string()]);
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> DBResult<()> {
        self.base.open()?;
        self.shortest_paths.clear();
        self.nodes_visited = 0;
        self.edges_traversed = 0;
        self.execution_time_ms = 0;
        self.max_depth_reached = 0;
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        self.base.close()?;
        self.shortest_paths.clear();
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send> HasStorage<S> for ShortestPathExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not initialized")
    }
}

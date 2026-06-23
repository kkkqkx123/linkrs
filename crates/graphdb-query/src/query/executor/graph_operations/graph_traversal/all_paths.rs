//! The AllPaths executor
//!
//! Implementation of AllPathsExecutor based on Nebula 3.8.0, using NPath linked list structure to optimize memory
//! Functional features:
//! Bidirectional BFS (Breadth-First Search) algorithm
//! Use the NPath linked list structure to share path prefixes.
//! Supports finding all paths (not just the shortest one).
//! Automatic loop detection to avoid circular paths.
//! Supports the use of `limit` and `offset` to control the number of results.
//! Support for the `withProp` function to return path attributes
//! Use a two-stage expansion approach (left expansion and right expansion).
//! Use heuristic expansion when the number of nodes exceeds a certain threshold.
//! CPU-intensive operations that are parallelized using Rayon.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;

use crate::core::error::{DBError, DBResult};
use crate::core::types::VertexId;
use crate::core::{Edge, NPath, Path, Value};
use crate::query::executor::base::{
    AllPathsConfig, BaseExecutor, EdgeDirection, ExecutionResult, Executor, ExecutorStats,
};
use crate::query::executor::utils::recursion_detector::ParallelConfig;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Auxiliary structure for removing duplicates from self-loop edges
#[derive(Debug, Default)]
struct SelfLoopDedup {
    seen: HashSet<(String, i64)>,
    with_loop: bool,
}

impl SelfLoopDedup {
    fn with_loop(with_loop: bool) -> Self {
        Self {
            seen: HashSet::new(),
            with_loop,
        }
    }

    fn should_include(&mut self, edge: &Edge) -> bool {
        let is_self_loop = edge.src == edge.dst;
        if is_self_loop {
            if self.with_loop {
                return true;
            }
            let key = (edge.edge_type.clone(), edge.ranking);
            self.seen.insert(key)
        } else {
            true
        }
    }
}

/// Unique identifier for a path based on its vertex sequence
type PathKey = Vec<VertexId>;

/// Caching of path results: Using NPath to reduce memory usage
#[derive(Debug, Clone)]
struct PathResultCache {
    /// Use NPath to store intermediate results and share prefixes.
    npaths: Vec<Arc<NPath>>,
    /// Track seen paths to avoid duplicates
    seen_paths: HashSet<PathKey>,
}

impl PathResultCache {
    fn new(limit: usize) -> Self {
        Self {
            npaths: Vec::with_capacity(limit.min(1024)), // Pre-allocate capacity, but not more than 1024 units, to avoid memory waste.
            seen_paths: HashSet::new(),
        }
    }

    fn len(&self) -> usize {
        self.npaths.len()
    }

    /// Generate a unique key for an NPath based on vertex sequence
    fn generate_path_key(npath: &NPath) -> PathKey {
        npath.iter_vertices().map(|v| v.vid).collect()
    }

    /// Add NPath to the cache with deduplication.
    /// Returns true if the path was added (not a duplicate), false otherwise.
    fn push(&mut self, npath: Arc<NPath>) -> bool {
        let key = Self::generate_path_key(&npath);
        if self.seen_paths.insert(key) {
            self.npaths.push(npath);
            true
        } else {
            false
        }
    }

    /// Batch conversion to Path format
    fn to_paths(&self) -> Vec<Path> {
        self.npaths.iter().map(|np| np.to_path()).collect()
    }

    /// Parallel batch conversion to Path format
    fn to_paths_parallel(&self) -> Vec<Path> {
        const BATCH_SIZE: usize = 1000;
        if self.npaths.len() < BATCH_SIZE {
            return self.to_paths();
        }

        self.npaths
            .par_chunks(BATCH_SIZE)
            .flat_map(|chunk| chunk.iter().map(|np| np.to_path()).collect::<Vec<_>>())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct AllPathsExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    left_start_ids: Vec<VertexId>,
    right_start_ids: Vec<VertexId>,
    pub edge_direction: EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub max_steps: usize,
    pub with_prop: bool,
    pub limit: usize,
    pub offset: usize,
    pub step_filter: Option<String>,
    pub filter: Option<String>,
    pub with_loop: bool,
    space_name: String,
    left_steps: usize,
    right_steps: usize,
    left_visited: HashSet<VertexId>,
    right_visited: HashSet<VertexId>,
    left_path_map: HashMap<VertexId, Arc<NPath>>,
    right_path_map: HashMap<VertexId, Arc<NPath>>,
    left_queue: VecDeque<(VertexId, Arc<NPath>)>,
    right_queue: VecDeque<(VertexId, Arc<NPath>)>,
    result_cache: PathResultCache,
    nodes_visited: usize,
    edges_traversed: usize,
    parallel_config: ParallelConfig,
}

impl<S: StorageClient> AllPathsExecutor<S> {
    pub fn new(
        base_config: crate::query::executor::base::ExecutorConfig<S>,
        config: AllPathsConfig,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "AllPathsExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            left_start_ids: config.left_start_ids,
            right_start_ids: config.right_start_ids,
            edge_direction: config.direction,
            edge_types: config.edge_types,
            max_steps: config.max_hops,
            with_prop: false,
            limit: usize::MAX,
            offset: 0,
            step_filter: None,
            filter: None,
            with_loop: false,
            space_name: config.space_name,
            left_steps: 0,
            right_steps: 0,
            left_visited: HashSet::new(),
            right_visited: HashSet::new(),
            left_path_map: HashMap::new(),
            right_path_map: HashMap::new(),
            left_queue: VecDeque::new(),
            right_queue: VecDeque::new(),
            result_cache: PathResultCache::new(usize::MAX),
            nodes_visited: 0,
            edges_traversed: 0,
            parallel_config: ParallelConfig::default(),
        }
    }

    /// Set whether to allow self-looping edges.
    pub fn with_loop(mut self, with_loop: bool) -> Self {
        self.with_loop = with_loop;
        self
    }

    /// Setting up parallel computing configurations
    pub fn with_parallel_config(mut self, config: ParallelConfig) -> Self {
        self.parallel_config = config;
        self
    }

    pub fn with_config(mut self, with_prop: bool, limit: usize, offset: usize) -> Self {
        self.with_prop = with_prop;
        self.limit = limit;
        self.offset = offset;
        self.result_cache = PathResultCache::new(limit);
        self
    }

    pub fn with_filters(mut self, step_filter: Option<String>, filter: Option<String>) -> Self {
        self.step_filter = step_filter;
        self.filter = filter;
        self
    }

    fn get_neighbors(
        &self,
        node_id: &VertexId,
        direction: EdgeDirection,
    ) -> DBResult<Vec<(VertexId, Edge)>> {
        let storage = self
            .base
            .storage
            .as_ref()
            .expect("AllPathsExecutor storage not set");
        let storage = storage.read();

        let edges = storage
            .get_node_edges(&self.space_name, node_id, direction)
            .map_err(|e| DBError::storage(e.to_string()))?;

        let filtered_edges = if let Some(ref edge_types) = self.edge_types {
            edges
                .into_iter()
                .filter(|edge| edge_types.contains(&edge.edge_type))
                .collect()
        } else {
            edges
        };

        let mut dedup = SelfLoopDedup::with_loop(self.with_loop);

        let neighbors = filtered_edges
            .into_iter()
            .filter(|edge| dedup.should_include(edge))
            .filter_map(|edge| match direction {
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
            })
            .collect();

        Ok(neighbors)
    }

    /// Leftward expansion – Using NPath to avoid path duplication
    fn expand_left(&mut self) -> DBResult<()> {
        while let Some((current_id, current_npath)) = self.left_queue.pop_front() {
            if self.left_visited.contains(&current_id) {
                continue;
            }
            self.left_visited.insert(current_id);
            self.left_path_map.insert(current_id, current_npath.clone());
            self.nodes_visited += 1;

            // Check whether the limit has been reached.
            if self.result_cache.len() >= self.limit {
                return Ok(());
            }

            let neighbors = self.get_neighbors(&current_id, EdgeDirection::Out)?;
            self.edges_traversed += neighbors.len();

            for (neighbor_id, edge) in neighbors {
                // Loop detection: Using the contains_vertex method from NPath
                if current_npath.contains_vertex(&neighbor_id) {
                    continue;
                }
                if self.left_visited.contains(&neighbor_id) {
                    continue;
                }

                let storage = self
                    .base
                    .storage
                    .as_ref()
                    .expect("AllPathsExecutor storage not set");
                let storage = storage.read();
                if let Ok(Some(neighbor_vertex)) =
                    storage.get_vertex(&self.space_name, &neighbor_id)
                {
                    // Using NPath expansion, O(1) operation, sharing prefixes
                    let new_npath = Arc::new(NPath::extend(
                        current_npath.clone(),
                        Arc::new(edge.clone()),
                        Arc::new(neighbor_vertex),
                    ));

                    // Check whether it intersects with the rightward search.
                    if let Some(right_npath) = self.right_path_map.get(&neighbor_id) {
                        // Construct the complete path
                        if let Some(full_path) = self.join_paths(&new_npath, right_npath) {
                            self.result_cache.push(Arc::new(full_path));
                        }
                    }

                    self.left_queue.push_back((neighbor_id, new_npath));
                }
            }
        }

        self.left_steps += 1;
        Ok(())
    }

    /// Rightward expansion – Using NPath to avoid path duplication
    fn expand_right(&mut self) -> DBResult<()> {
        while let Some((current_id, current_npath)) = self.right_queue.pop_front() {
            if self.right_visited.contains(&current_id) {
                continue;
            }
            self.right_visited.insert(current_id);
            self.right_path_map
                .insert(current_id, current_npath.clone());
            self.nodes_visited += 1;

            // Check whether the limit has been reached.
            if self.result_cache.len() >= self.limit {
                return Ok(());
            }

            let neighbors = self.get_neighbors(&current_id, EdgeDirection::In)?;
            self.edges_traversed += neighbors.len();

            for (neighbor_id, edge) in neighbors {
                // Loop detection
                if current_npath.contains_vertex(&neighbor_id) {
                    continue;
                }
                if self.right_visited.contains(&neighbor_id) {
                    continue;
                }

                let storage = self
                    .base
                    .storage
                    .as_ref()
                    .expect("AllPathsExecutor storage not set");
                let storage = storage.read();
                if let Ok(Some(neighbor_vertex)) =
                    storage.get_vertex(&self.space_name, &neighbor_id)
                {
                    // Using the NPath extension
                    let new_npath = Arc::new(NPath::extend(
                        current_npath.clone(),
                        Arc::new(edge.clone()),
                        Arc::new(neighbor_vertex),
                    ));

                    // Check whether it intersects with the leftward search.
                    if let Some(left_npath) = self.left_path_map.get(&neighbor_id) {
                        // Construct the complete path
                        if let Some(full_path) = self.join_paths(left_npath, &new_npath) {
                            self.result_cache.push(Arc::new(full_path));
                        }
                    }

                    self.right_queue.push_back((neighbor_id, new_npath));
                }
            }
        }

        self.right_steps += 1;
        Ok(())
    }

    /// Heuristic decision-making with extension
    fn should_expand_both(&self) -> bool {
        let left_size = self.left_visited.len();
        let right_size = self.right_visited.len();

        const PATH_THRESHOLD_SIZE: usize = 100;
        const PATH_THRESHOLD_RATIO: usize = 2;

        if left_size > PATH_THRESHOLD_SIZE && right_size > PATH_THRESHOLD_SIZE {
            if left_size > right_size && left_size / right_size > PATH_THRESHOLD_RATIO {
                return false;
            }
            if right_size > left_size && right_size / left_size > PATH_THRESHOLD_RATIO {
                return false;
            }
        }
        true
    }

    /// Connect the left and right paths
    /// Left path: From the starting point to the intersection point.
    /// Right path: From the end point to the intersection point (the direction needs to be reversed).
    fn join_paths(&self, left_path: &NPath, right_path: &NPath) -> Option<NPath> {
        use crate::core::{Edge, Vertex};
        use std::sync::Arc;

        let left_vertices: std::collections::HashSet<_> =
            left_path.iter_vertices().map(|v| v.vid).collect();
        let right_vertices: std::collections::HashSet<_> =
            right_path.iter_vertices().map(|v| v.vid).collect();

        let common: Vec<_> = left_vertices.intersection(&right_vertices).collect();
        if common.len() != 1 {
            return None;
        }

        let junction_id = *common[0];

        if left_path.end_vertex().vid != junction_id {
            return None;
        }
        if right_path.end_vertex().vid != junction_id {
            return None;
        }

        let total_length = left_path.len() + right_path.len();
        if total_length > self.max_steps {
            return None;
        }

        let mut full_path = left_path.clone();

        let right_steps: Vec<(Arc<Edge>, Arc<Vertex>)> = right_path
            .iter()
            .filter_map(|node| {
                node.edge()
                    .map(|edge| (edge.clone(), node.vertex().clone()))
            })
            .collect();

        for (edge, _vertex) in right_steps {
            let next_vid = edge.dst;
            let reversed_edge = Arc::new(Edge::new(
                full_path.end_vertex().vid,
                next_vid,
                edge.edge_type.clone(),
                edge.ranking,
                edge.props.clone(),
            ));
            if let Some(parent_npath) = right_path.iter().find(|n| n.vertex().vid == next_vid) {
                full_path = NPath::extend(
                    Arc::new(full_path),
                    reversed_edge,
                    parent_npath.vertex().clone(),
                );
            }
        }

        Some(full_path)
    }

    /// Initialize the queue
    fn initialize_queues(&mut self) -> DBResult<()> {
        let storage = self
            .base
            .storage
            .as_ref()
            .expect("AllPathsExecutor storage not set");
        let storage = storage.read();

        // Initialize the left queue
        for left_id in &self.left_start_ids {
            if let Ok(Some(vertex)) = storage.get_vertex(&self.space_name, left_id) {
                let npath = Arc::new(NPath::new(Arc::new(vertex)));
                self.left_queue.push_back((*left_id, npath));
            }
        }

        // Initialize the right queue
        for right_id in &self.right_start_ids {
            if let Ok(Some(vertex)) = storage.get_vertex(&self.space_name, right_id) {
                let npath = Arc::new(NPath::new(Arc::new(vertex)));
                self.right_queue.push_back((*right_id, npath));
            }
        }

        Ok(())
    }

    /// Perform bidirectional BFS (Breadth-First Search).
    fn execute_bidirectional(&mut self) -> DBResult<()> {
        self.initialize_queues()?;

        while self.left_steps + self.right_steps < self.max_steps {
            // Check to see if there are any additional nodes that can be expanded.
            if self.left_queue.is_empty() && self.right_queue.is_empty() {
                break;
            }

            // Heuristic extension of decision-making
            let expand_both = self.should_expand_both();

            if expand_both {
                // Bi-directional expansion
                if !self.left_queue.is_empty() {
                    self.expand_left()?;
                }
                if !self.right_queue.is_empty() {
                    self.expand_right()?;
                }
            } else {
                // Unilateral expansion: Select the side with fewer nodes.
                let left_size = self.left_visited.len();
                let right_size = self.right_visited.len();

                if left_size <= right_size && !self.left_queue.is_empty() {
                    self.expand_left()?;
                } else if !self.right_queue.is_empty() {
                    self.expand_right()?;
                }
            }

            // Check whether the limit has been reached.
            if self.result_cache.len() >= self.limit {
                break;
            }
        }

        Ok(())
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for AllPathsExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start_time = Instant::now();

        // Perform bidirectional BFS (Breadth-First Search).
        self.execute_bidirectional()?;

        // Convert to Path result
        let paths = if self.parallel_config.enable_parallel {
            self.result_cache.to_paths_parallel()
        } else {
            self.result_cache.to_paths()
        };

        // Applying the offset
        let paths: Vec<Path> = if self.offset > 0 && self.offset < paths.len() {
            paths.into_iter().skip(self.offset).collect()
        } else {
            paths
        };

        let execution_time = start_time.elapsed().as_millis() as u64;

        // Update statistical information
        self.base
            .get_stats_mut()
            .add_stat("nodes_visited".to_string(), self.nodes_visited.to_string());
        self.base.get_stats_mut().add_stat(
            "edges_traversed".to_string(),
            self.edges_traversed.to_string(),
        );
        self.base
            .get_stats_mut()
            .add_stat("execution_time_ms".to_string(), execution_time.to_string());
        self.base
            .get_stats_mut()
            .add_stat("paths_found".to_string(), paths.len().to_string());

        let rows: Vec<Vec<Value>> = paths.into_iter().map(|p| vec![Value::path(p)]).collect();
        let dataset = DataSet::from_rows(rows, vec!["path".to_string()]);
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
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
        &self.base.description
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Edge;
    use std::collections::HashMap;

    #[test]
    fn test_self_loop_dedup() {
        let mut dedup = SelfLoopDedup::with_loop(false);
        let edge = Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(1),
            "friend".to_string(),
            0,
            HashMap::new(),
        );

        assert!(dedup.should_include(&edge));
        assert!(!dedup.should_include(&edge)); // The second attempt should return “false”.
    }
}

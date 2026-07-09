//! BFS Shortest Path Executor
//!
//! Use a bidirectional breadth-first search algorithm to find the shortest path.
//! Referencing the Nebula-Graph implementation, it supports bidirectional BFS (Breadth-First Search) and path concatenation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::core::types::VertexId;
use crate::core::{Edge, EdgeDirection, Path, Value, Vertex};
use crate::query::executor::base::{BaseExecutor, ExecutorConfig};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::DataSet;
use crate::storage::StorageClient;
use parking_lot::RwLock;

/// BFS (Broadest First Search) shortest path configuration
pub struct BfsShortestPathConfig {
    pub steps: usize,
    pub edge_types: Vec<String>,
    pub with_cycle: bool,
    pub max_depth: Option<usize>,
    pub single_shortest: bool,
    pub limit: usize,
    pub start_vertex: VertexId,
    pub end_vertex: VertexId,
    pub space_name: String,
}

/// BFSShortestExecutor – The BFS shortest path executor
///
/// Use a bidirectional breadth-first search algorithm to find the shortest path.
/// Referencing the Nebula-Graph implementation, it supports bidirectional BFS (Breadth-First Search) and path concatenation.
pub struct BFSShortestExecutor<S: StorageClient + 'static> {
    base: BaseExecutor<S>,
    steps: usize,
    max_depth: Option<usize>,
    edge_types: Vec<String>,
    with_cycle: bool, // Is it allowed to have loops (repeated visits to the same vertex within the path)?
    with_loop: bool,  // Are self-loop edges allowed?
    single_shortest: bool,
    limit: usize,
    start_vertex: VertexId,
    end_vertex: VertexId,
    space_name: String,

    // Status of translation:
    step: usize,
    left_visited_vids: HashSet<VertexId>,
    right_visited_vids: HashSet<VertexId>,
    all_left_edges: Vec<HashMap<VertexId, Edge>>,
    all_right_edges: Vec<HashMap<VertexId, Edge>>,
    current_paths: Vec<Path>,
    terminate_early: bool,

    // Statistical information
    nodes_visited: usize,
    edges_traversed: usize,
    execution_time_ms: u64,
}

impl<S: StorageClient + 'static> BFSShortestExecutor<S> {
    pub fn new(base_config: ExecutorConfig<S>, config: BfsShortestPathConfig) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "BFSShortestExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            steps: config.steps,
            max_depth: config.max_depth,
            edge_types: config.edge_types,
            with_cycle: config.with_cycle,
            with_loop: false,
            single_shortest: config.single_shortest,
            limit: config.limit,
            start_vertex: config.start_vertex,
            end_vertex: config.end_vertex,
            space_name: config.space_name,
            step: 1,
            left_visited_vids: HashSet::new(),
            right_visited_vids: HashSet::new(),
            all_left_edges: Vec::new(),
            all_right_edges: Vec::new(),
            current_paths: Vec::new(),
            terminate_early: false,
            nodes_visited: 0,
            edges_traversed: 0,
            execution_time_ms: 0,
        }
    }

    /// Set whether to allow self-loop edges.
    pub fn with_loop(mut self, with_loop: bool) -> Self {
        self.with_loop = with_loop;
        self
    }

    pub fn steps(&self) -> usize {
        self.steps
    }

    pub fn max_depth(&self) -> Option<usize> {
        self.max_depth
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn with_cycle(&self) -> bool {
        self.with_cycle
    }

    pub fn single_shortest(&self) -> bool {
        self.single_shortest
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn nodes_visited(&self) -> usize {
        self.nodes_visited
    }

    pub fn edges_traversed(&self) -> usize {
        self.edges_traversed
    }

    pub fn execution_time_ms(&self) -> u64 {
        self.execution_time_ms
    }

    pub fn current_paths(&self) -> &[Path] {
        &self.current_paths
    }

    /// Building paths: Extracting edges from the input and constructing the set of vertices for the next step.
    fn build_path(
        &mut self,
        storage: &S,
        start_vids: &[VertexId],
        reverse: bool,
    ) -> DBResult<Vec<VertexId>> {
        let mut current_edges: HashMap<VertexId, Edge> = HashMap::new();
        let mut unique_dst: HashSet<VertexId> = HashSet::new();
        // Deduplication tracking from self-loop edges
        let mut seen_self_loops: HashSet<(String, i64)> = HashSet::new();

        // Reserving capacity to improve performance
        if reverse {
            self.right_visited_vids.reserve(start_vids.len());
        } else {
            self.left_visited_vids.reserve(start_vids.len());
        }

        for start_vid in start_vids {
            // Obtain the outgoing or incoming edges of the current vertex.
            let edges = if self.edge_types.is_empty() {
                if reverse {
                    storage.get_node_edges(&self.space_name, start_vid, EdgeDirection::In)?
                } else {
                    storage.get_node_edges(&self.space_name, start_vid, EdgeDirection::Out)?
                }
            } else {
                // Filter by edge type
                let all_edges = if reverse {
                    storage.get_node_edges(&self.space_name, start_vid, EdgeDirection::In)?
                } else {
                    storage.get_node_edges(&self.space_name, start_vid, EdgeDirection::Out)?
                };
                // Filter out edges of the specified type.
                all_edges
                    .into_iter()
                    .filter(|edge| self.edge_types.contains(&edge.edge_type))
                    .collect()
            };

            for edge in edges {
                self.edges_traversed += 1;
                let dst = if reverse { edge.src } else { edge.dst };

                let is_self_loop = edge.src == edge.dst;
                // If self-loop edges are not allowed, duplicates should be removed.
                if is_self_loop && !self.with_loop {
                    let key = (edge.edge_type.clone(), edge.ranking);
                    if !seen_self_loops.insert(key) {
                        continue; // Duplicate self-loop edges should be skipped.
                    }
                }

                // Check whether the page has been visited.
                let already_visited = if reverse {
                    self.right_visited_vids.contains(&dst)
                } else {
                    self.left_visited_vids.contains(&dst)
                };

                if already_visited {
                    continue;
                }

                // Check for acyclic constraints (unique vertices in the path).
                if !self.with_cycle {
                    let in_path = self.left_visited_vids.contains(&dst)
                        || self.right_visited_vids.contains(&dst);
                    if in_path {
                        continue;
                    }
                }

                if unique_dst.insert(dst) {
                    current_edges.insert(dst, edge);
                }
            }
        }

        // Save the edges of the current layer.
        if reverse {
            self.all_right_edges.push(current_edges);
        } else {
            self.all_left_edges.push(current_edges);
        }

        // Mark the newly discovered vertex as having been visited.
        let new_vids: Vec<VertexId> = unique_dst.into_iter().collect();
        let new_vids_count = new_vids.len();
        if reverse {
            self.right_visited_vids.extend(&new_vids);
        } else {
            self.left_visited_vids.extend(&new_vids);
        }

        self.nodes_visited += new_vids_count;

        Ok(new_vids)
    }

    /// Concatenate paths: Find the intersection point of the left and right paths and combine them into a complete path.
    /// Return a decision on whether the search should be terminated prematurely.
    fn conjunct_paths(&mut self, current_step: usize) -> DBResult<bool> {
        if self.all_left_edges.is_empty() || self.all_right_edges.is_empty() {
            return Ok(false);
        }

        let left_edges = self
            .all_left_edges
            .last()
            .expect("Left edges should not be empty");

        // Find the intersection point.
        let mut meet_vids: HashSet<VertexId> = HashSet::new();
        let mut odd_step = true;

        // First, try to match the right edge of the previous step.
        if current_step > 1 && current_step - 2 < self.all_right_edges.len() {
            let prev_right_edges = &self.all_right_edges[current_step - 2];
            for vid in left_edges.keys() {
                if prev_right_edges.contains_key(vid) {
                    meet_vids.insert(*vid);
                }
            }
        }

        // If it is not found, try to match it with the right edge of the current step.
        if meet_vids.is_empty() && !self.all_right_edges.is_empty() {
            odd_step = false;
            let right_edges = self
                .all_right_edges
                .last()
                .expect("Right edges should not be empty");
            for vid in left_edges.keys() {
                if right_edges.contains_key(vid) {
                    meet_vids.insert(*vid);
                }
            }
        }

        if meet_vids.is_empty() {
            return Ok(false);
        }

        // Construct a complete path for each intersection point.
        for meet_vid in meet_vids {
            if let Some(path) = self.create_path(&meet_vid, odd_step) {
                // Check whether there are any duplicate edges in the path (the vertices in the path are unique).
                if !self.with_cycle && path.has_duplicate_edges() {
                    continue;
                }

                self.current_paths.push(path);

                // If we are only looking for the shortest path, the search can be terminated once it is found.
                if self.single_shortest {
                    self.terminate_early = true;
                    return Ok(true);
                }

                // Check whether the limits have been reached.
                if self.current_paths.len() >= self.limit {
                    self.terminate_early = true;
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Create a complete path from the starting point to the ending point.
    fn create_path(&self, meet_vid: &VertexId, _odd_step: bool) -> Option<Path> {
        // Construct the path for the left half (from the starting point to the intersection point).
        let left_path = self.build_half_path(meet_vid, false)?;

        // Construct the path for the right half (from the endpoint to the intersection point).
        let right_path = self.build_half_path(meet_vid, true)?;

        // Concatenation path: Reverse the right half of the path and append it to the left half.
        let mut full_path = left_path;

        // Steps to reverse the right half of the path
        let mut reversed_steps: Vec<crate::core::vertex_edge_path::Step> =
            right_path.steps.into_iter().rev().collect();

        // Reverse the direction of each edge.
        for step in &mut reversed_steps {
            std::mem::swap(&mut step.edge.src, &mut step.edge.dst);
        }

        // Append to the full path
        full_path.steps.extend(reversed_steps);

        Some(full_path)
    }

    /// Constructing a half-path
    fn build_half_path(&self, meet_vid: &VertexId, reverse: bool) -> Option<Path> {
        let all_edges = if reverse {
            &self.all_right_edges
        } else {
            &self.all_left_edges
        };

        if all_edges.is_empty() {
            return Some(Path::new(Vertex::new(*meet_vid, vec![])));
        }

        let mut current_vid = *meet_vid;
        let mut steps: Vec<(Vertex, Edge)> = Vec::new();

        for edge_layer in all_edges.iter().rev() {
            if let Some(edge) = edge_layer.get(&current_vid) {
                let next_vid = if reverse { edge.dst } else { edge.src };
                steps.push((Vertex::new(next_vid, vec![]), edge.clone()));
                current_vid = next_vid;
            } else {
                break;
            }
        }

        if steps.is_empty() {
            return Some(Path::new(Vertex::new(*meet_vid, vec![])));
        }

        let mut path = Path::new(steps.last()?.0.clone());
        for (vertex, edge) in steps.iter().rev() {
            path.add_step(crate::core::vertex_edge_path::Step {
                dst: Box::new(vertex.clone()),
                edge: Box::new(edge.clone()),
            });
        }

        Some(path)
    }
}

impl<S: StorageClient + 'static> Executor<S> for BFSShortestExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start_time = std::time::Instant::now();

        // Reset the status
        self.step = 1;
        self.left_visited_vids.clear();
        self.right_visited_vids.clear();
        self.all_left_edges.clear();
        self.all_right_edges.clear();
        self.current_paths.clear();
        self.terminate_early = false;

        // Initialization: Add the starting point and the ending point to the set of visited locations.
        self.left_visited_vids.insert(self.start_vertex);
        self.right_visited_vids.insert(self.end_vertex);

        // Bidirectional BFS main loop
        let max_steps = self.steps;
        let mut terminate_early = false;

        for current_step in 1..=max_steps {
            if terminate_early {
                break;
            }

            // Expand from the starting direction.
            let left_vids: Vec<VertexId> = if current_step == 1 {
                vec![self.start_vertex]
            } else {
                let last_left_edges = self.all_left_edges.last();
                match last_left_edges {
                    Some(edges) => edges.keys().cloned().collect(),
                    None => Vec::new(),
                }
            };

            let right_vids: Vec<VertexId> = if current_step == 1 {
                vec![self.end_vertex]
            } else {
                let last_right_edges = self.all_right_edges.last();
                match last_right_edges {
                    Some(edges) => edges.keys().cloned().collect(),
                    None => Vec::new(),
                }
            };

            let left_has_vids = !left_vids.is_empty();
            let right_has_vids = !right_vids.is_empty();

            // Expand from the starting direction.
            if left_has_vids {
                let storage = self.get_storage().clone();
                let storage_guard = storage.read();
                self.build_path(&storage_guard, &left_vids, false)?;
            }

            // away
            if right_has_vids {
                let storage = self.get_storage().clone();
                let storage_guard = storage.read();
                self.build_path(&storage_guard, &right_vids, true)?;
            }

            // Check for intersections and splice paths
            let should_terminate = self.conjunct_paths(current_step)?;
            if should_terminate {
                terminate_early = true;
            }
        }

        let execution_time = start_time.elapsed().as_millis() as u64;
        self.execution_time_ms = execution_time;

        let rows: Vec<Vec<Value>> = self
            .current_paths
            .clone()
            .into_iter()
            .map(|p| vec![Value::path(p)])
            .collect();

        let dataset = DataSet::from_rows(rows, vec!["path".to_string()]);
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> DBResult<()> {
        self.step = 1;
        self.left_visited_vids.clear();
        self.right_visited_vids.clear();
        self.all_left_edges.clear();
        self.all_right_edges.clear();
        self.current_paths.clear();
        self.terminate_early = false;
        self.nodes_visited = 0;
        self.edges_traversed = 0;
        self.execution_time_ms = 0;
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

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + 'static> HasStorage<S> for BFSShortestExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not set")
    }
}

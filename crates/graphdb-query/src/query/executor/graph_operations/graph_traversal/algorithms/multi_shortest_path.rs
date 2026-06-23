//! multi-source shortest path algorithm
//!
//! Support multiple groups of start and end points to find the shortest path at the same time

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::core::error::{DBError, DBResult};
use crate::core::types::VertexId;
use crate::core::{Edge, Path, Step, Value, Vertex};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{
    BaseExecutor, DBResult as ExecDBResult, EdgeDirection, ExecutionResult,
    Executor as BaseExecutorTrait, ExecutorStats, HasStorage, InputExecutor,
    MultiShortestPathConfig,
};
use crate::query::DataSet;
use crate::storage::StorageClient;
use parking_lot::RwLock;

use super::types::{
    cleanup_termination_map, create_termination_map, is_termination_complete, mark_path_found,
    AlgorithmStats, Interims, SelfLoopDedup, TerminationMap,
};

/// Multi-source shortest path executor
///
/// Handle multiple (src, dst) path lookup requests simultaneously
/// Supports single/multiple shortest paths using bi-directional BFS algorithm
pub struct MultiShortestPathExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    start_vids: Vec<VertexId>,
    end_vids: Vec<VertexId>,
    termination_map: TerminationMap,
    edge_direction: EdgeDirection,
    edge_types: Option<Vec<String>>,
    max_steps: usize,
    single_shortest: bool,
    limit: usize,
    step: usize,
    space_name: String,
    history_left_paths: Interims,
    history_right_paths: Interims,
    left_paths: Interims,
    right_paths: Interims,
    pre_right_paths: Interims,
    result_paths: Vec<Path>,
    stats: AlgorithmStats,
    left_input: Option<Box<ExecutorEnum<S>>>,
    right_input: Option<Box<ExecutorEnum<S>>>,
    found_count: usize,
}

impl<S: StorageClient> std::fmt::Debug for MultiShortestPathExecutor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiShortestPathExecutor")
            .field("base", &"BaseExecutor")
            .field("start_vids", &self.start_vids)
            .field("end_vids", &self.end_vids)
            .field("max_steps", &self.max_steps)
            .field("single_shortest", &self.single_shortest)
            .field("limit", &self.limit)
            .field("step", &self.step)
            .field("result_paths", &self.result_paths.len())
            .finish()
    }
}

impl<S: StorageClient> MultiShortestPathExecutor<S> {
    pub fn new(
        base_config: crate::query::executor::base::ExecutorConfig<S>,
        config: MultiShortestPathConfig,
    ) -> Self {
        let termination_map = create_termination_map(&config.start_vids, &[]);

        Self {
            base: BaseExecutor::new(
                base_config.id,
                "MultiShortestPathExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            start_vids: config.start_vids,
            end_vids: vec![],
            termination_map,
            edge_direction: config.direction,
            edge_types: config.edge_types,
            max_steps: config.max_steps,
            space_name: config.space_name,
            single_shortest: false,
            limit: usize::MAX,
            step: 1,
            history_left_paths: HashMap::new(),
            history_right_paths: HashMap::new(),
            left_paths: HashMap::new(),
            right_paths: HashMap::new(),
            pre_right_paths: HashMap::new(),
            result_paths: Vec::new(),
            stats: AlgorithmStats::new(),
            left_input: None,
            right_input: None,
            found_count: 0,
        }
    }

    pub fn with_limits(mut self, single_shortest: bool, limit: usize) -> Self {
        self.single_shortest = single_shortest;
        self.limit = limit;
        self
    }

    pub fn with_inputs(
        mut self,
        left_input: Box<ExecutorEnum<S>>,
        right_input: Box<ExecutorEnum<S>>,
    ) -> Self {
        self.left_input = Some(left_input);
        self.right_input = Some(right_input);
        self
    }

    fn init(&mut self) {
        for src in &self.start_vids {
            let path = Path::new(Vertex::with_vid(*src));
            let mut src_map = HashMap::new();
            src_map.insert(*src, vec![path.clone()]);
            self.history_left_paths.insert(*src, src_map);
        }

        for dst in &self.end_vids {
            let path = Path::new(Vertex::with_vid(*dst));
            let mut dst_map = HashMap::new();
            dst_map.insert(*dst, vec![path.clone()]);
            self.history_right_paths.insert(*dst, dst_map.clone());
            self.pre_right_paths.insert(*dst, dst_map);
        }
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
            .ok_or_else(|| DBError::storage("Storage not set".to_string()))?;
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

        let mut dedup = SelfLoopDedup::new();

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

    /// Creating a new path (extending an existing path)
    fn create_paths(paths: &[Path], edge: &Edge) -> Vec<Path> {
        paths
            .iter()
            .map(|p| {
                let mut new_path = p.clone();
                let dst_vertex = Vertex::with_vid(edge.dst);
                new_path.steps.push(Step::new(
                    dst_vertex,
                    edge.edge_type.clone(),
                    edge.edge_type.clone(),
                    edge.ranking,
                ));
                new_path
            })
            .collect()
    }

    fn build_path(&mut self, reverse: bool) -> DBResult<()> {
        let history_paths = if reverse {
            &self.history_right_paths
        } else {
            &self.history_left_paths
        };

        let expand_vids: Vec<VertexId> = if self.step == 1 {
            if reverse {
                self.end_vids.clone()
            } else {
                self.start_vids.clone()
            }
        } else {
            history_paths.keys().cloned().collect()
        };

        let mut all_neighbors: Vec<(VertexId, Vec<(VertexId, Edge)>)> = Vec::new();
        for vid in &expand_vids {
            let neighbors = self.get_neighbors(vid, self.edge_direction)?;
            self.stats.increment_edges_traversed(neighbors.len());
            all_neighbors.push((*vid, neighbors));
        }

        for (vid, neighbors) in all_neighbors {
            for (neighbor_id, edge) in neighbors {
                if neighbor_id == vid {
                    continue;
                }

                let current_paths = if reverse {
                    &mut self.right_paths
                } else {
                    &mut self.left_paths
                };

                if self.step == 1 {
                    let src_vertex = Vertex::with_vid(vid);
                    let dst_vertex = Vertex::with_vid(neighbor_id);
                    let path = Path {
                        src: Box::new(src_vertex),
                        steps: vec![Step::new(
                            dst_vertex,
                            edge.edge_type.clone(),
                            edge.edge_type.clone(),
                            edge.ranking,
                        )],
                    };

                    let entry = current_paths
                        .entry(neighbor_id)
                        .or_insert_with(HashMap::new);
                    let src_paths = entry.entry(vid).or_insert_with(Vec::new);
                    src_paths.push(path);
                } else {
                    if let Some(pre_paths) = history_paths.get(&vid) {
                        for (src_id, paths) in pre_paths {
                            if let Some(history_dst) = history_paths.get(&neighbor_id) {
                                if history_dst.contains_key(src_id) {
                                    continue;
                                }
                            }

                            let new_paths = Self::create_paths(paths, &edge);

                            let entry = current_paths
                                .entry(neighbor_id)
                                .or_insert_with(HashMap::new);
                            let src_paths = entry.entry(*src_id).or_insert_with(Vec::new);
                            src_paths.extend(new_paths);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn conjunct_path(&mut self, odd_step: bool) -> DBResult<bool> {
        let right_paths = if odd_step {
            &self.pre_right_paths
        } else {
            &self.right_paths
        };

        let mut path_pairs: Vec<(VertexId, VertexId, Vec<Path>, Vec<Path>)> = Vec::new();

        for (meet_vid, left_src_map) in &self.left_paths {
            if let Some(right_src_map) = right_paths.get(meet_vid) {
                for (left_src, left_paths) in left_src_map {
                    for (right_src, right_paths) in right_src_map {
                        if self.is_valid_pair(left_src, right_src) {
                            path_pairs.push((
                                *left_src,
                                *right_src,
                                left_paths.clone(),
                                right_paths.clone(),
                            ));
                        }
                    }
                }
            }
        }

        for (left_src, right_src, left_paths, right_paths) in path_pairs {
            self.build_result_paths(&left_paths, &right_paths, &left_src, &right_src)?;

            if self.single_shortest {
                mark_path_found(&mut self.termination_map, &left_src, &right_src);
            }
        }

        if self.single_shortest {
            cleanup_termination_map(&mut self.termination_map);
        }

        if is_termination_complete(&self.termination_map) {
            return Ok(true);
        }

        if self.found_count >= self.limit {
            return Ok(true);
        }

        if self.step * 2 > self.max_steps {
            return Ok(true);
        }

        Ok(false)
    }

    fn is_valid_pair(&self, src: &VertexId, dst: &VertexId) -> bool {
        if let Some(pairs) = self.termination_map.get(src) {
            pairs.iter().any(|(d, found)| d == dst && *found)
        } else {
            false
        }
    }

    fn build_result_paths(
        &mut self,
        left_paths: &[Path],
        right_paths: &[Path],
        _src: &VertexId,
        _dst: &VertexId,
    ) -> DBResult<()> {
        for left_path in left_paths {
            for right_path in right_paths {
                let mut full_path = left_path.clone();
                let mut reversed_right = right_path.clone();
                reversed_right.reverse();

                full_path.steps.extend(reversed_right.steps);

                // Check for repeating edges
                if self.has_duplicate_edges(&full_path) {
                    continue;
                }

                self.result_paths.push(full_path);
                self.found_count += 1;

                if self.found_count >= self.limit {
                    return Ok(());
                }

                if self.single_shortest {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn has_duplicate_edges(&self, path: &Path) -> bool {
        let mut edge_set = HashSet::new();

        for step in &path.steps {
            let edge_key = format!("{}_{}_{}", step.src_vid(), step.dst_vid(), step.ranking());
            if !edge_set.insert(edge_key) {
                return true;
            }
        }

        false
    }

    fn update_history(&mut self) {
        for (dst, src_map) in &self.left_paths {
            let history_entry = self.history_left_paths.entry(*dst).or_default();
            for (src, paths) in src_map {
                let src_entry = history_entry.entry(*src).or_default();
                src_entry.extend(paths.clone());
            }
        }

        for (dst, src_map) in &self.right_paths {
            let history_entry = self.history_right_paths.entry(*dst).or_default();
            for (src, paths) in src_map {
                let src_entry = history_entry.entry(*src).or_default();
                src_entry.extend(paths.clone());
            }
        }

        self.pre_right_paths = self.right_paths.clone();

        self.left_paths.clear();
        self.right_paths.clear();
    }

    pub fn execute_multi_path(&mut self) -> DBResult<Vec<Path>> {
        let start_time = Instant::now();

        self.init();

        loop {
            self.build_path(false)?;

            self.build_path(true)?;

            if self.conjunct_path(true)? {
                break;
            }

            if self.conjunct_path(false)? {
                break;
            }

            self.update_history();

            self.step += 1;

            if self.step * 2 > self.max_steps {
                break;
            }
        }

        self.stats
            .set_execution_time(start_time.elapsed().as_millis() as u64);

        Ok(self.result_paths.clone())
    }
}

impl<S: StorageClient + Send + 'static> BaseExecutorTrait<S> for MultiShortestPathExecutor<S> {
    fn execute(&mut self) -> ExecDBResult<ExecutionResult> {
        let paths = self
            .execute_multi_path()
            .map_err(|e| DBError::query(e.to_string()))?;

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
        "Multi-source shortest path executor"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for MultiShortestPathExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for MultiShortestPathExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        if self.left_input.is_none() {
            self.left_input = Some(Box::new(input));
        } else if self.right_input.is_none() {
            self.right_input = Some(Box::new(input));
        }
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.left_input.as_ref().map(|b| b.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;
    use crate::core::{Path, Vertex};
    use crate::query::executor::base::ExecutorConfig;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;

    #[test]
    fn test_termination_map_creation() {
        let start_vids = vec![VertexId::from_string("a"), VertexId::from_string("b")];
        let end_vids = vec![VertexId::from_string("c"), VertexId::from_string("d")];

        let map = create_termination_map(&start_vids, &end_vids);

        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&VertexId::from_string("a")));
        assert!(map.contains_key(&VertexId::from_string("b")));

        let a_pairs = map
            .get(&VertexId::from_string("a"))
            .expect("Failed to get pairs for 'a'");
        assert_eq!(a_pairs.len(), 2);
    }

    #[test]
    fn test_mark_path_found() {
        let start_vids = vec![VertexId::from_string("a")];
        let end_vids = vec![VertexId::from_string("b")];

        let mut map = create_termination_map(&start_vids, &end_vids);

        assert!(mark_path_found(
            &mut map,
            &VertexId::from_string("a"),
            &VertexId::from_string("b")
        ));

        let pairs = map
            .get(&VertexId::from_string("a"))
            .expect("Failed to get pairs for 'a'");
        assert!(!pairs[0].1); // “The word ‘found’ should be marked as ‘false’.”
    }

    #[test]
    fn test_cleanup_termination_map() {
        let start_vids = vec![VertexId::from_string("a")];
        let end_vids = vec![VertexId::from_string("b"), VertexId::from_string("c")];

        let mut map = create_termination_map(&start_vids, &end_vids);
        mark_path_found(&mut map, &VertexId::from_string("a"), &VertexId::from_string("b"));
        cleanup_termination_map(&mut map);

        assert_eq!(map.len(), 1);
        let pairs = map
            .get(&VertexId::from_string("a"))
            .expect("Failed to get pairs for 'a'");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, VertexId::from_string("c"));
    }

    #[test]
    fn test_create_paths() {
        let path = Path::new(Vertex::with_vid(VertexId::from_string("a")));

        let edge = Edge::new(
            VertexId::from_string("a"),
            VertexId::from_string("b"),
            "edge".to_string(),
            0,
            HashMap::new(),
        );

        let new_paths = MultiShortestPathExecutor::<MockStorage>::create_paths(&[path], &edge);

        assert_eq!(new_paths.len(), 1);
        assert_eq!(new_paths[0].steps.len(), 1);
    }

    #[test]
    fn test_has_duplicate_edges() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = MultiShortestPathConfig {
            start_vids: vec![VertexId::from_string("a")],
            direction: EdgeDirection::Out,
            edge_types: None,
            max_steps: 10,
            space_name: String::new(),
        };

        let base_config = ExecutorConfig::new(1, storage, expr_context);
        let executor = MultiShortestPathExecutor::new(base_config, config);

        // Create a path
        let path = Path {
            src: Box::new(Vertex::with_vid(VertexId::from_string("a"))),
            steps: vec![
                Step::new(
                    Vertex::with_vid(VertexId::from_string("b")),
                    "e".to_string(),
                    "e".to_string(),
                    0,
                ),
                Step::new(
                    Vertex::with_vid(VertexId::from_string("c")),
                    "e".to_string(),
                    "e".to_string(),
                    0,
                ),
            ],
        };

        // This test requires actual edge data; the processing process needs to be simplified.
        // The actual tests should be carried out during the integration testing phase.
        assert!(!executor.has_duplicate_edges(&path));
    }
}

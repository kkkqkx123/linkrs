use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{Edge, EdgeDirection, NPath, Path, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;

pub(crate) struct BidirBfsConfig<'a> {
    pub(crate) space_name: &'a str,
    pub(crate) edge_type_filter: Option<&'a [String]>,
    pub(crate) max_depth: usize,
    pub(crate) single_shortest: bool,
    pub(crate) limit: usize,
    pub(crate) direction: EdgeDirection,
}

pub(crate) fn path_endpoint_pairs(
    row: &[Value],
    layout: Arc<SlotLayout>,
    start_vertices: &[Value],
    target_vertices: &[Value],
    target_expression: Option<&Expression>,
) -> Result<Vec<(Value, Value)>, QueryError> {
    let context = ValueRowContext::new(row.to_vec(), layout.clone());
    let row_start = context.get_variable("vid").or_else(|| row.first().cloned());
    let row_target = if let Some(expression) = target_expression {
        let mut expression_context = ValueRowContext::new(row.to_vec(), layout);
        Some(
            ExpressionEvaluator::evaluate(expression, &mut expression_context).map_err(
                |error| QueryError::execution(format!("Path target evaluation failed: {error}")),
            )?,
        )
    } else {
        context
            .get_variable("dst_vid")
            .or_else(|| row.get(1).cloned())
    };
    let starts: Vec<Value> = if start_vertices.is_empty() {
        row_start.into_iter().collect()
    } else {
        start_vertices.to_vec()
    };
    let targets: Vec<Value> = if target_vertices.is_empty() {
        row_target.into_iter().collect()
    } else {
        target_vertices.to_vec()
    };
    Ok(starts
        .into_iter()
        .flat_map(|start| {
            targets
                .iter()
                .cloned()
                .map(move |target| (start.clone(), target))
        })
        .collect())
}

pub(crate) fn bidir_bfs_shortest_path(
    storage: &dyn QueryStorage,
    start_id: &VertexId,
    end_id: &VertexId,
    cfg: BidirBfsConfig,
    cancel_token: Option<&AtomicBool>,
) -> Result<Vec<Path>, QueryError> {
    let mut result_paths = Vec::new();

    let mut left_visited: HashMap<VertexId, Arc<NPath>> = HashMap::new();
    let mut right_visited: HashMap<VertexId, Arc<NPath>> = HashMap::new();
    let mut left_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();
    let mut right_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();

    if let Ok(Some(start_vertex)) = storage.get_vertex(cfg.space_name, start_id) {
        let np = Arc::new(NPath::new(Arc::new(start_vertex)));
        left_queue.push_back((*start_id, np.clone()));
        left_visited.insert(*start_id, np);
    }
    if let Ok(Some(end_vertex)) = storage.get_vertex(cfg.space_name, end_id) {
        let np = Arc::new(NPath::new(Arc::new(end_vertex)));
        right_queue.push_back((*end_id, np.clone()));
        right_visited.insert(*end_id, np);
    }

    let dir_out = EdgeDirection::Out;
    let dir_in = EdgeDirection::In;
    let forward_dir = match cfg.direction {
        EdgeDirection::Out => dir_out,
        EdgeDirection::In => dir_in,
        EdgeDirection::Both => dir_out,
    };
    let backward_dir = match cfg.direction {
        EdgeDirection::Out => dir_in,
        EdgeDirection::In => dir_out,
        EdgeDirection::Both => dir_in,
    };

    while !left_queue.is_empty() && !right_queue.is_empty() {
        if let Some(token) = cancel_token {
            if token.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(QueryError::execution("Query cancelled".to_string()));
            }
        }
        if cfg.single_shortest && !result_paths.is_empty() {
            break;
        }
        if result_paths.len() >= cfg.limit {
            break;
        }

        let left_level = left_queue.len();
        let mut left_next: Vec<(VertexId, Arc<NPath>)> = Vec::new();
        for _ in 0..left_level {
            if let Some((current_id, current_npath)) = left_queue.pop_front() {
                if current_npath.len() >= cfg.max_depth {
                    continue;
                }
                if let Ok(edges) = storage.get_node_edges(cfg.space_name, &current_id, forward_dir) {
                    let filtered: Vec<&Edge> = if let Some(types) = cfg.edge_type_filter {
                        edges
                            .iter()
                            .filter(|e| types.contains(&e.edge_type))
                            .collect()
                    } else {
                        edges.iter().collect()
                    };
                    for edge in &filtered {
                        let neighbor_id = edge.dst();
                        if left_visited.contains_key(neighbor_id) {
                            continue;
                        }
                        if let Ok(Some(neighbor_vertex)) =
                            storage.get_vertex(cfg.space_name, neighbor_id)
                        {
                            let new_npath = Arc::new(NPath::extend(
                                current_npath.clone(),
                                Arc::new((*edge).clone()),
                                Arc::new(neighbor_vertex),
                            ));
                            left_next.push((*neighbor_id, new_npath.clone()));
                            left_visited.insert(*neighbor_id, new_npath);
                        }
                    }
                }
            }
        }
        for (id, np) in left_next.drain(..) {
            left_queue.push_back((id, np));
        }

        if cfg.single_shortest && !result_paths.is_empty() {
            break;
        }
        if result_paths.len() >= cfg.limit {
            break;
        }

        let right_level = right_queue.len();
        let mut right_next: Vec<(VertexId, Arc<NPath>)> = Vec::new();
        for _ in 0..right_level {
            if let Some((current_id, current_npath)) = right_queue.pop_front() {
                if current_npath.len() >= cfg.max_depth {
                    continue;
                }

                if let Some(left_npath) = left_visited.get(&current_id) {
                    let total_len = left_npath.len() + current_npath.len();
                    if total_len <= cfg.max_depth {
                        let mut left_path = left_npath.to_path();
                        let mut right_path = current_npath.to_path();
                        right_path.reverse();
                        left_path.steps.extend(right_path.steps);
                        result_paths.push(left_path);
                        if cfg.single_shortest || result_paths.len() >= cfg.limit {
                            break;
                        }
                    }
                    continue;
                }

                if let Ok(edges) = storage.get_node_edges(cfg.space_name, &current_id, backward_dir) {
                    let filtered: Vec<&Edge> = if let Some(types) = cfg.edge_type_filter {
                        edges
                            .iter()
                            .filter(|e| types.contains(&e.edge_type))
                            .collect()
                    } else {
                        edges.iter().collect()
                    };
                    for edge in &filtered {
                        let neighbor_id = if edge.dst() == &current_id {
                            edge.src()
                        } else {
                            edge.dst()
                        };
                        if right_visited.contains_key(neighbor_id) {
                            continue;
                        }
                        if let Ok(Some(neighbor_vertex)) =
                            storage.get_vertex(cfg.space_name, neighbor_id)
                        {
                            let new_npath = Arc::new(NPath::extend(
                                current_npath.clone(),
                                Arc::new((*edge).clone()),
                                Arc::new(neighbor_vertex),
                            ));
                            right_next.push((*neighbor_id, new_npath.clone()));
                            right_visited.insert(*neighbor_id, new_npath);
                        }
                    }
                }
            }
        }
        for (id, np) in right_next.drain(..) {
            if let Some(left_npath) = left_visited.get(&id) {
                let total_len = left_npath.len() + np.len();
                if total_len <= cfg.max_depth {
                    let mut left_path = left_npath.to_path();
                    let mut right_path = np.to_path();
                    right_path.reverse();
                    left_path.steps.extend(right_path.steps);
                    result_paths.push(left_path);
                    if cfg.single_shortest || result_paths.len() >= cfg.limit {
                        break;
                    }
                }
            } else {
                right_queue.push_back((id, np));
            }
        }
    }

    if cfg.single_shortest && !result_paths.is_empty() {
        result_paths.sort_by_key(|a| a.steps.len());
        result_paths.truncate(1);
    }
    result_paths.truncate(cfg.limit);
    Ok(result_paths)
}

pub(crate) struct AllPathsConfig<'a> {
    pub(crate) space_name: &'a str,
    pub(crate) edge_types: &'a [String],
    pub(crate) direction: EdgeDirection,
    pub(crate) min_depth: usize,
    pub(crate) max_depth: usize,
    pub(crate) acyclic: bool,
    pub(crate) result_cap: usize,
}

pub(crate) fn enumerate_all_paths(
    storage: &dyn QueryStorage,
    start_id: &VertexId,
    end_id: &VertexId,
    cfg: AllPathsConfig<'_>,
    cancel_token: Option<&AtomicBool>,
) -> Result<Vec<Path>, QueryError> {
    let Some(start_vertex) = storage
        .get_vertex(cfg.space_name, start_id)
        .map_err(|error| QueryError::execution(format!("Failed to read start vertex: {error}")))?
    else {
        return Ok(Vec::new());
    };
    let mut initial_visited = HashSet::new();
    initial_visited.insert(*start_id);
    let mut stack = vec![(
        *start_id,
        Arc::new(NPath::new(Arc::new(start_vertex))),
        initial_visited,
    )];
    let mut paths = Vec::new();

    while let Some((current_id, current_path, visited)) = stack.pop() {
        if let Some(token) = cancel_token {
            if token.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(QueryError::execution("Query cancelled".to_string()));
            }
        }
        let depth = current_path.len();
        if current_id == *end_id && depth >= cfg.min_depth {
            paths.push(current_path.to_path());
            if paths.len() >= cfg.result_cap {
                break;
            }
        }
        if depth >= cfg.max_depth || current_id == *end_id {
            continue;
        }
        let edges = storage
            .get_node_edges(cfg.space_name, &current_id, cfg.direction)
            .map_err(|error| {
                QueryError::execution(format!("Failed to read path edges: {error}"))
            })?;
        for edge in edges {
            if !cfg.edge_types.is_empty() && !cfg.edge_types.contains(&edge.edge_type) {
                continue;
            }
            let next_id = match cfg.direction {
                EdgeDirection::Out => *edge.dst(),
                EdgeDirection::In => *edge.src(),
                EdgeDirection::Both => {
                    if edge.src() == &current_id {
                        *edge.dst()
                    } else {
                        *edge.src()
                    }
                }
            };
            if cfg.acyclic && visited.contains(&next_id) {
                continue;
            }
            let Some(vertex) = storage
                .get_vertex(cfg.space_name, &next_id)
                .map_err(|error| {
                    QueryError::execution(format!("Failed to read path vertex: {error}"))
                })?
            else {
                continue;
            };
            let mut next_visited = visited.clone();
            next_visited.insert(next_id);
            stack.push((
                next_id,
                Arc::new(NPath::extend(
                    current_path.clone(),
                    Arc::new(edge),
                    Arc::new(vertex),
                )),
                next_visited,
            ));
        }
    }
    Ok(paths)
}

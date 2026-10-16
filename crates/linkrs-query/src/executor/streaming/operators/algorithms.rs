use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::context::ValueRowContext;
use crate::executor::streaming::query_registry::CancelToken;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::traversal::graph_reader::{edge_key, EdgeKey};
use crate::planning::plan::core::nodes::traversal::path_algorithms::PathUniqueness;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::{EdgeDirection, NPath, Path, Value, Vertex};

/// Materialize one path endpoint into a tagged vertex.
///
/// Tagged values pass through untouched. Bare ids need a label: the plan
/// override wins, otherwise the role tag derived from the edge schemas.
/// A bare id with no resolvable label is an explicit error rather than a
/// silently skipped pair; a storage read failure surfaces instead of
/// masquerading as a missing vertex.
pub(crate) fn materialize_path_endpoint(
    storage: &dyn QueryStorage,
    space_name: &str,
    value: &Value,
    override_tag: &str,
    role_tag: Option<&str>,
) -> Result<Option<Vertex>, QueryError> {
    if let Value::Vertex(vertex) = value {
        return Ok(Some((**vertex).clone()));
    }
    let Ok(vid) = VertexId::try_from(value) else {
        return Ok(None);
    };
    let tag = if !override_tag.is_empty() {
        override_tag.to_string()
    } else if let Some(tag) = role_tag {
        tag.to_string()
    } else {
        return Err(QueryError::execution(
            "Path endpoint is a bare id: OVER must list edge types whose schema resolves one label per path endpoint"
                .to_string(),
        ));
    };
    storage
        .get_vertex(space_name, &tag, &vid)
        .map_err(|error| QueryError::execution(format!("Failed to read path endpoint: {error}")))
}

/// Derive the (start, end) endpoint labels for a label-less path search.
///
/// A plan-provided label wins for both roles. Otherwise every listed edge
/// type contributes its source label to the start role and its destination
/// label to the end role, oriented by the traversal direction, and each
/// role must resolve to exactly one label across all edge types.
/// `Both` traversal needs that edge type's endpoints to share one label.
/// An empty edge type list, unknown edge types, untyped schemas or an
/// ambiguous role yield `None` rather than a guessed label; callers only
/// need a label when an endpoint arrives as a bare id.
pub(crate) fn path_endpoint_tags(
    storage: &dyn QueryStorage,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    override_tag: &str,
) -> Option<(String, String)> {
    if !override_tag.is_empty() {
        return Some((override_tag.to_string(), override_tag.to_string()));
    }
    if edge_types.is_empty() {
        return None;
    }
    let mut start_tags: Vec<String> = Vec::new();
    let mut end_tags: Vec<String> = Vec::new();
    for edge_type in edge_types {
        let (src_tag, dst_tag) =
            crate::executor::traversal::graph_reader::resolve_edge_endpoint_tags(
                storage, space_name, edge_type,
            )?;
        let (start, end) = match direction {
            EdgeDirection::Out => (src_tag, dst_tag),
            EdgeDirection::In => (dst_tag, src_tag),
            EdgeDirection::Both => {
                if src_tag != dst_tag {
                    return None;
                }
                (src_tag.clone(), dst_tag)
            }
        };
        push_unique_label(&mut start_tags, start);
        push_unique_label(&mut end_tags, end);
    }
    match (start_tags.as_slice(), end_tags.as_slice()) {
        ([start], [end]) => Some((start.clone(), end.clone())),
        _ => None,
    }
}

fn push_unique_label(labels: &mut Vec<String>, label: String) {
    if !label.is_empty() && !labels.iter().any(|kept| kept == &label) {
        labels.push(label);
    }
}

pub(crate) struct BidirBfsConfig<'a> {
    pub(crate) space_name: &'a str,
    pub(crate) vertex_tag: &'a str,
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
    start_vertex: &Vertex,
    end_vertex: &Vertex,
    cfg: BidirBfsConfig,
    cancel_token: Option<&CancelToken>,
) -> Result<Vec<Path>, QueryError> {
    let mut result_paths = Vec::new();
    let mut best_len: Option<usize> = None;
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut left_depth: usize = 0;
    let mut right_depth: usize = 0;

    let mut left_visited: HashMap<VertexId, Arc<NPath>> = HashMap::new();
    let mut right_visited: HashMap<VertexId, Arc<NPath>> = HashMap::new();
    let mut left_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();
    let mut right_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();

    let start_id = start_vertex.vid;
    let end_id = end_vertex.vid;
    let np = Arc::new(NPath::new(Arc::new(start_vertex.clone())));
    left_queue.push_back((start_id, np.clone()));
    left_visited.insert(start_id, np);
    let np = Arc::new(NPath::new(Arc::new(end_vertex.clone())));
    right_queue.push_back((end_id, np.clone()));
    right_visited.insert(end_id, np);

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
            if token.is_cancelled() {
                return Err(QueryError::execution("Query cancelled".to_string()));
            }
        }
        if cfg.single_shortest && !result_paths.is_empty() {
            break;
        }
        if result_paths.len() >= cfg.limit {
            break;
        }
        // When collecting all shortest paths, stop as soon as both sides
        // have been expanded past the best length found so far: any future
        // meeting would be strictly longer than the best path.
        if !cfg.single_shortest {
            if let Some(best) = best_len {
                if best <= left_depth && best <= right_depth {
                    break;
                }
            }
        }

        let left_level = left_queue.len();
        let mut left_next: Vec<(VertexId, Arc<NPath>)> = Vec::new();
        for _ in 0..left_level {
            if let Some((current_id, current_npath)) = left_queue.pop_front() {
                if current_npath.len() >= cfg.max_depth {
                    continue;
                }
                let edges = storage.get_node_edges(
                    cfg.space_name,
                    &current_id,
                    forward_dir,
                    cfg.edge_type_filter.unwrap_or(&[]),
                )?;
                for edge in &edges {
                    let neighbor_id = edge.dst();
                    if left_visited.contains_key(neighbor_id) {
                        continue;
                    }
                    let Some(neighbor_tag) =
                        crate::executor::traversal::graph_reader::resolve_neighbor_tag(
                            storage,
                            cfg.space_name,
                            edge,
                            neighbor_id,
                            cfg.vertex_tag,
                        )
                    else {
                        continue;
                    };
                    // An invisible neighbor is legitimately skipped, but a read
                    // failure must abort: treating it as "no such vertex"
                    // would silently drop paths from the result.
                    if let Some(neighbor_vertex) =
                        storage.get_vertex(cfg.space_name, &neighbor_tag, neighbor_id)?
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
        for (id, np) in left_next.drain(..) {
            left_queue.push_back((id, np));
        }
        left_depth += 1;

        if cfg.single_shortest && !result_paths.is_empty() {
            break;
        }
        if result_paths.len() >= cfg.limit {
            break;
        }
        if !cfg.single_shortest {
            if let Some(best) = best_len {
                if best <= left_depth && best <= right_depth {
                    break;
                }
            }
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
                        if push_shortest_path(
                            left_path,
                            &mut best_len,
                            &mut seen_paths,
                            &mut result_paths,
                            cfg.limit,
                        ) {
                            break;
                        }
                    }
                    continue;
                }

                let edges = storage.get_node_edges(
                    cfg.space_name,
                    &current_id,
                    backward_dir,
                    cfg.edge_type_filter.unwrap_or(&[]),
                )?;
                for edge in &edges {
                    let neighbor_id = if edge.dst() == &current_id {
                        edge.src()
                    } else {
                        edge.dst()
                    };
                    if right_visited.contains_key(neighbor_id) {
                        continue;
                    }
                    let Some(neighbor_tag) =
                        crate::executor::traversal::graph_reader::resolve_neighbor_tag(
                            storage,
                            cfg.space_name,
                            edge,
                            neighbor_id,
                            cfg.vertex_tag,
                        )
                    else {
                        continue;
                    };
                    if let Some(neighbor_vertex) =
                        storage.get_vertex(cfg.space_name, &neighbor_tag, neighbor_id)?
                    {
                        // The right side walks backward from the end vertex,
                        // so store the edge flipped: the resulting path then
                        // keeps edge endpoints and step dsts consistent.
                        let mut backward_edge = (*edge).clone();
                        std::mem::swap(&mut backward_edge.src, &mut backward_edge.dst);
                        let new_npath = Arc::new(NPath::extend(
                            current_npath.clone(),
                            Arc::new(backward_edge),
                            Arc::new(neighbor_vertex),
                        ));
                        right_next.push((*neighbor_id, new_npath.clone()));
                        right_visited.insert(*neighbor_id, new_npath);
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
                    if push_shortest_path(
                        left_path,
                        &mut best_len,
                        &mut seen_paths,
                        &mut result_paths,
                        cfg.limit,
                    ) {
                        break;
                    }
                }
            } else {
                right_queue.push_back((id, np));
            }
        }
        right_depth += 1;
    }

    if cfg.single_shortest && !result_paths.is_empty() {
        result_paths.sort_by_key(|a| a.steps.len());
        result_paths.truncate(1);
    }
    result_paths.truncate(cfg.limit);
    Ok(result_paths)
}

/// Add a candidate path to the result set, keeping only paths of the
/// minimal length found so far. Returns `true` when the result limit is
/// reached and the caller should stop searching.
fn push_shortest_path(
    path: Path,
    best_len: &mut Option<usize>,
    seen: &mut HashSet<String>,
    result: &mut Vec<Path>,
    limit: usize,
) -> bool {
    let total = path.steps.len();
    match *best_len {
        Some(best) if total > best => return false,
        Some(best) if total < best => {
            *best_len = Some(total);
            result.clear();
        }
        None => *best_len = Some(total),
        Some(_) => {}
    }
    if seen.insert(format!("{:?}", path)) {
        result.push(path);
    }
    result.len() >= limit
}

pub(crate) struct AllPathsConfig<'a> {
    pub(crate) space_name: &'a str,
    pub(crate) vertex_tag: &'a str,
    pub(crate) edge_types: &'a [String],
    pub(crate) direction: EdgeDirection,
    pub(crate) min_depth: usize,
    pub(crate) max_depth: usize,
    pub(crate) path_uniqueness: PathUniqueness,
    pub(crate) result_cap: usize,
}

/// Heap entry for the weighted (Dijkstra) shortest path. Ordered by ascending
/// cost, then ascending depth; `Ord` inverts both so the `BinaryHeap`
/// (a max-heap) pops the cheapest, then fewest-hop entry first.
struct WeightedPathItem {
    cost: f64,
    depth: usize,
    vertex_id: VertexId,
    npath: Arc<NPath>,
}

impl PartialEq for WeightedPathItem {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost && self.depth == other.depth
    }
}
impl Eq for WeightedPathItem {}
impl PartialOrd for WeightedPathItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for WeightedPathItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.depth.cmp(&self.depth))
    }
}

/// Weighted shortest path (Dijkstra) between two vertices.
///
/// Runs from `start_vertex`, finalizing the cheapest path to
/// `end_vertex` within `cfg.max_depth` hops. The edge weight is read by
/// `read_edge_weight` (shared with the MATCH `*WEIGHTED` runtime). Ties on
/// cost resolve to the fewer-hop path; ties on both resolve to whichever
/// the heap finalizes first, keeping the result deterministic.
pub(crate) fn weighted_shortest_path(
    storage: &dyn QueryStorage,
    start_vertex: &Vertex,
    end_vertex: &Vertex,
    space_name: &str,
    vertex_tag: &str,
    edge_type_filter: Option<&[String]>,
    direction: EdgeDirection,
    max_depth: usize,
    weight_prop: &str,
    cancel_token: Option<&CancelToken>,
) -> Result<Option<Path>, QueryError> {
    if let Some(token) = cancel_token {
        if token.is_cancelled() {
            return Err(QueryError::execution("Query cancelled".to_string()));
        }
    }

    let start_id = start_vertex.vid;
    let end_id = end_vertex.vid;

    let mut finalized: HashSet<VertexId> = HashSet::new();
    let mut dist: HashMap<VertexId, f64> = HashMap::new();
    let mut heap: BinaryHeap<WeightedPathItem> = BinaryHeap::new();
    let np = Arc::new(NPath::new(Arc::new(start_vertex.clone())));
    dist.insert(start_id, 0.0);
    heap.push(WeightedPathItem {
        cost: 0.0,
        depth: 0,
        vertex_id: start_id,
        npath: np,
    });

    while let Some(top) = heap.pop() {
        if let Some(token) = cancel_token {
            if token.is_cancelled() {
                return Err(QueryError::execution("Query cancelled".to_string()));
            }
        }
        // Skip stale heap entries superseded by a cheaper finalized cost.
        if finalized.contains(&top.vertex_id) {
            continue;
        }
        finalized.insert(top.vertex_id);

        if top.vertex_id == end_id {
            return Ok(Some(top.npath.to_path()));
        }
        if top.depth >= max_depth {
            continue;
        }

        let edges = storage.get_node_edges(
            space_name,
            &top.vertex_id,
            direction,
            edge_type_filter.unwrap_or(&[]),
        )?;
        for edge in &edges {
            let neighbor_id = if direction == EdgeDirection::In {
                edge.src()
            } else if direction == EdgeDirection::Out {
                edge.dst()
            } else if edge.dst() == &top.vertex_id {
                edge.src()
            } else {
                edge.dst()
            };
            if finalized.contains(neighbor_id) {
                continue;
            }
            let next_cost = top.cost
                + crate::executor::traversal::weighted::read_edge_weight(edge, weight_prop);
            let better = match dist.get(neighbor_id) {
                Some(old) => next_cost < *old,
                None => true,
            };
            if !better {
                continue;
            }
            let Some(neighbor_tag) = crate::executor::traversal::graph_reader::resolve_neighbor_tag(
                storage,
                space_name,
                edge,
                neighbor_id,
                vertex_tag,
            ) else {
                continue;
            };
            if let Some(neighbor_vertex) =
                storage.get_vertex(space_name, &neighbor_tag, neighbor_id)?
            {
                dist.insert(*neighbor_id, next_cost);
                heap.push(WeightedPathItem {
                    cost: next_cost,
                    depth: top.depth + 1,
                    vertex_id: *neighbor_id,
                    npath: Arc::new(NPath::extend(
                        top.npath.clone(),
                        Arc::new((*edge).clone()),
                        Arc::new(neighbor_vertex),
                    )),
                });
            }
        }
    }
    Ok(None)
}

pub(crate) fn enumerate_all_paths(
    storage: &dyn QueryStorage,
    start_vertex: &Vertex,
    end_vertex: &Vertex,
    cfg: AllPathsConfig<'_>,
    cancel_token: Option<&CancelToken>,
) -> Result<Vec<Path>, QueryError> {
    let start_id = start_vertex.vid;
    let end_id = end_vertex.vid;
    let mut stack = vec![WalkState {
        current_id: start_id,
        path: Arc::new(NPath::new(Arc::new(start_vertex.clone()))),
        visited_vertices: HashSet::from([start_id]),
        visited_edges: HashSet::new(),
    }];
    let mut paths = Vec::new();

    while let Some(state) = stack.pop() {
        if let Some(token) = cancel_token {
            if token.is_cancelled() {
                return Err(QueryError::execution("Query cancelled".to_string()));
            }
        }
        let depth = state.path.len();
        if state.current_id == end_id && depth >= cfg.min_depth {
            paths.push(state.path.to_path());
            if paths.len() >= cfg.result_cap {
                break;
            }
        }
        if depth >= cfg.max_depth {
            continue;
        }
        let edges = storage
            .get_node_edges(
                cfg.space_name,
                &state.current_id,
                cfg.direction,
                cfg.edge_types,
            )
            .map_err(|error| {
                QueryError::execution(format!("Failed to read path edges: {error}"))
            })?;
        for edge in edges {
            let next_id = match cfg.direction {
                EdgeDirection::Out => *edge.dst(),
                EdgeDirection::In => *edge.src(),
                EdgeDirection::Both => {
                    if edge.src() == &state.current_id {
                        *edge.dst()
                    } else {
                        *edge.src()
                    }
                }
            };
            // A self-loop lands back on the current vertex; a normal edge
            // may revisit an earlier vertex. The repeat rules are governed
            // by the WITH LOOP / WITH TRAIL / WITH WALK policy.
            let is_self_loop = edge.src() == edge.dst();
            let repeats_vertex = state.visited_vertices.contains(&next_id);
            let key = edge_key(&edge);
            match cfg.path_uniqueness {
                PathUniqueness::VertexUnique => {
                    if repeats_vertex {
                        continue;
                    }
                }
                PathUniqueness::AllowSelfLoop => {
                    if repeats_vertex && !is_self_loop {
                        continue;
                    }
                }
                PathUniqueness::EdgeUnique => {
                    if state.visited_edges.contains(&key) {
                        continue;
                    }
                }
                PathUniqueness::Free => {}
            }
            // A self-loop adds no new reachable vertex, so under policies
            // with no edge-level constraint it is emitted as a terminal
            // extension instead of being pushed back on the stack (which
            // would loop on the same edge forever). Under EdgeUnique the
            // burned edge bounds the repetition, so the hop continues
            // like any other.
            let terminal_extension = is_self_loop
                && matches!(
                    cfg.path_uniqueness,
                    PathUniqueness::AllowSelfLoop | PathUniqueness::Free
                );
            // A path ends at the target: normal edges leaving the target are
            // not followed. A self-loop on the target is still taken as a
            // terminal extension, otherwise WITH LOOP could never surface a
            // path that uses one.
            if state.current_id == end_id && !terminal_extension {
                continue;
            }
            let Some(neighbor_tag) = crate::executor::traversal::graph_reader::resolve_neighbor_tag(
                storage,
                cfg.space_name,
                &edge,
                &next_id,
                cfg.vertex_tag,
            ) else {
                continue;
            };
            let Some(vertex) = storage
                .get_vertex(cfg.space_name, &neighbor_tag, &next_id)
                .map_err(|error| {
                    QueryError::execution(format!("Failed to read path vertex: {error}"))
                })?
            else {
                continue;
            };
            let next_path = Arc::new(NPath::extend(
                state.path.clone(),
                Arc::new(edge),
                Arc::new(vertex),
            ));
            if terminal_extension {
                if next_id == end_id && depth + 1 >= cfg.min_depth {
                    paths.push(next_path.to_path());
                    if paths.len() >= cfg.result_cap {
                        break;
                    }
                }
                continue;
            }
            let mut next_vertices = state.visited_vertices.clone();
            next_vertices.insert(next_id);
            let mut next_edges = state.visited_edges.clone();
            next_edges.insert(key);
            stack.push(WalkState {
                current_id: next_id,
                path: next_path,
                visited_vertices: next_vertices,
                visited_edges: next_edges,
            });
        }
    }
    Ok(paths)
}

/// Enumeration state of one candidate path: the current vertex plus the
/// bookkeeping the repeat policy prunes with. `visited_vertices` backs the
/// vertex-unique policies, `visited_edges` backs the edge-unique one.
struct WalkState {
    current_id: VertexId,
    path: Arc<NPath>,
    visited_vertices: HashSet<VertexId>,
    visited_edges: HashSet<EdgeKey>,
}

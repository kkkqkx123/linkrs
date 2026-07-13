use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{Edge, EdgeDirection, NPath, Path, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::traversal::config::TraversalConfig;
use crate::query::executor::traversal::graph_reader::TraversalGraphReader;
use crate::query::executor::traversal::runtime::TraversalRuntime;
use crate::storage::StorageClient;

// ── Helper struct ──

struct BidirBfsConfig<'a> {
    space_name: &'a str,
    edge_type_filter: Option<&'a [String]>,
    max_depth: usize,
    single_shortest: bool,
    limit: usize,
}

// ── Helper functions ──

fn row_passes_filter(row: &[Value], col_names: &[String], filter: &Option<Expression>) -> bool {
    let Some(expr) = filter else {
        return true;
    };

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut context = ValueRowContext::new(row.to_vec(), layout);
    matches!(
        ExpressionEvaluator::evaluate(expr, &mut context),
        Ok(Value::Bool(true))
    )
}

fn path_endpoint_pairs(
    row: &[Value],
    layout: Arc<SlotLayout>,
    start_vertices: &[Value],
    target_vertices: &[Value],
    target_expression: Option<&Expression>,
) -> Result<Vec<(Value, Value)>, QueryError> {
    let context = ValueRowContext::new_with_layout(row.to_vec(), layout.clone());
    let row_start = context.get_variable("vid").or_else(|| row.first().cloned());
    let row_target = if let Some(expression) = target_expression {
        let mut expression_context = ValueRowContext::new_with_layout(row.to_vec(), layout);
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

fn bidir_bfs_shortest_path(
    storage: &dyn StorageClient,
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
                if let Ok(edges) = storage.get_node_edges(cfg.space_name, &current_id, dir_out) {
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

                if let Ok(edges) = storage.get_node_edges(cfg.space_name, &current_id, dir_in) {
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

struct AllPathsConfig<'a> {
    space_name: &'a str,
    edge_types: &'a [String],
    direction: EdgeDirection,
    min_depth: usize,
    max_depth: usize,
    acyclic: bool,
    result_cap: usize,
}

fn enumerate_all_paths(
    storage: &dyn StorageClient,
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

fn expand_on_chunk(
    chunk: DataChunk,
    reader: &dyn StorageClient,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    filter_expr: &Option<Expression>,
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<Option<DataChunk>, QueryError> {
    let col_names = chunk.col_names();

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
        let context = ValueRowContext::new_with_layout(row.clone(), chunk.get_layout());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let config = TraversalConfig::expand(space_name.to_string(), direction);
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config);
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            if let Ok(Some(vertex)) = reader.get_vertex(space_name, &vid) {
                runtime.seed_from_vertex(vertex);
            } else {
                continue;
            }

            while let Some(event) = runtime.next_event() {
                let mut out_row = row.clone();
                out_row.push(Value::Vertex(Box::new(event.vertex)));
                out_row.push(Value::String(edge_types.join("/")));
                out_row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                let mut out_col_names = col_names.clone();
                out_col_names.push("_expand_vertex".to_string());
                out_col_names.push("_expand_edge_type".to_string());
                out_col_names.push("_expand_direction".to_string());
                if row_passes_filter(&out_row, &out_col_names, filter_expr) {
                    out_rows.push(out_row);
                }
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    let mut new_cols: Vec<ColumnInfo> = col_names
        .iter()
        .map(|n| ColumnInfo {
            name: n.clone(),
            data_type: "string".to_string(),
        })
        .collect();
    new_cols.push(ColumnInfo {
        name: "_expand_vertex".to_string(),
        data_type: "vertex".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_expand_edge_type".to_string(),
        data_type: "string".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_expand_direction".to_string(),
        data_type: "string".to_string(),
    });
    let schema = Arc::new(Schema::new(new_cols));
    Ok(Some(DataChunk::new(out_rows, schema)))
}

fn traverse_on_chunk(
    chunk: DataChunk,
    reader: &dyn StorageClient,
    config: &TraversalConfig,
    visited: &mut HashSet<String>,
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<Option<DataChunk>, QueryError> {
    let col_names = chunk.col_names();
    let edge_type = config.edge_types.first().map(|s| s.as_str()).unwrap_or("");
    let dir_str = match config.direction {
        EdgeDirection::Out => "out",
        EdgeDirection::In => "in",
        EdgeDirection::Both => "both",
    };

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
        let context = ValueRowContext::new_with_layout(row.clone(), chunk.get_layout());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));
        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config.clone());
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            if let Ok(Some(vertex)) = reader.get_vertex(&config.space_name, &vid) {
                runtime.seed_from_vertex(vertex);
            } else {
                continue;
            }

            while let Some(event) = runtime.next_event() {
                let nid_str = format!("{:?}", event.vertex.vid());
                if visited.contains(&nid_str) {
                    continue;
                }
                visited.insert(nid_str);

                let mut out_row = row.clone();
                out_row.push(Value::Vertex(Box::new(event.vertex)));
                out_row.push(Value::String(edge_type.to_string()));
                out_row.push(Value::String(dir_str.to_string()));
                out_row.push(Value::BigInt(event.depth as i64));
                out_rows.push(out_row);
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    let mut new_cols: Vec<ColumnInfo> = col_names
        .iter()
        .map(|n| ColumnInfo {
            name: n.clone(),
            data_type: "string".to_string(),
        })
        .collect();
    new_cols.push(ColumnInfo {
        name: "_traverse_vertex".to_string(),
        data_type: "vertex".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_traverse_edge_type".to_string(),
        data_type: "string".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_traverse_direction".to_string(),
        data_type: "string".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_traverse_depth".to_string(),
        data_type: "bigint".to_string(),
    });
    let schema = Arc::new(Schema::new(new_cols));
    Ok(Some(DataChunk::new(out_rows, schema)))
}

#[derive(Debug)]
pub enum GraphOperator {
    Expand {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
    },
    ExpandAll {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
    },
    Traverse {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: HashSet<String>,
    },
    TraverseAll {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: HashSet<String>,
    },
    BiExpand {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
    BiTraverse {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        visited: HashSet<String>,
    },
    ShortestPath {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    BFSShortest {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        allow_cycles: bool,
        allow_loops: bool,
        frontier: Vec<Vec<Value>>,
        visited: HashSet<String>,
    },
    AllPaths {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: usize,
        max_depth: usize,
        acyclic: bool,
        limit: Option<usize>,
        offset: usize,
        filter: Option<Expression>,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    },
    MultiShortestPath {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertices: Vec<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        left_vertex_column: String,
        right_vertex_column: String,
        single_shortest: bool,
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    },
    Subgraph {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        steps: u32,
        direction: EdgeDirection,
        edge_types: Vec<String>,
    },
}

impl GraphOperator {
    pub fn bind_runtime(&mut self, runtime: &super::super::runtime::ExecutionRuntime) {
        let storage = runtime.storage.clone();
        let space_name = runtime.query_id().space_name.unwrap_or_default();
        match self {
            Self::Expand {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::ExpandAll {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::Traverse {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::TraverseAll {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::BiExpand {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::BiTraverse {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::ShortestPath {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::BFSShortest {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::AllPaths {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::MultiShortestPath {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::Subgraph {
                storage: target_storage,
                space_name: target_space,
                ..
            } => {
                *target_storage = storage;
                *target_space = space_name;
            }
        }
    }

    pub fn from_spec(
        spec: &super::super::operator_spec::GraphSpec,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
    ) -> Self {
        match spec {
            super::super::operator_spec::GraphSpec::Expand {
                edge_types,
                direction,
                filter_expr,
            } => Self::Expand {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
            },
            super::super::operator_spec::GraphSpec::ExpandAll {
                edge_types,
                direction,
                filter_expr,
            } => Self::ExpandAll {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
            },
            super::super::operator_spec::GraphSpec::Traverse {
                edge_types,
                direction,
                min_depth,
                max_depth,
                filter_expr,
            } => Self::Traverse {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                filter_expr: filter_expr.clone(),
                visited: std::collections::HashSet::new(),
            },
            super::super::operator_spec::GraphSpec::BiExpand {
                edge_types,
                direction,
            } => Self::BiExpand {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
            },
            super::super::operator_spec::GraphSpec::BiTraverse {
                edge_types,
                direction,
                min_depth,
                max_depth,
            } => Self::BiTraverse {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                visited: std::collections::HashSet::new(),
            },
            super::super::operator_spec::GraphSpec::ShortestPath {
                target_vertex,
                edge_types,
                direction,
                max_depth,
                start_vertices,
                target_vertices,
            } => Self::ShortestPath {
                storage: storage.clone(),
                space_name: space_name.clone(),
                target_vertex: target_vertex.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                start_vertices: start_vertices.clone(),
                target_vertices: target_vertices.clone(),
            },
            super::super::operator_spec::GraphSpec::BFSShortest {
                target_vertex,
                edge_types,
                direction,
                max_depth,
                allow_cycles,
                allow_loops,
            } => Self::BFSShortest {
                storage: storage.clone(),
                space_name: space_name.clone(),
                target_vertex: target_vertex.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                allow_cycles: *allow_cycles,
                allow_loops: *allow_loops,
                frontier: Vec::new(),
                visited: std::collections::HashSet::new(),
            },
            super::super::operator_spec::GraphSpec::AllPaths {
                target_vertex,
                edge_types,
                direction,
                min_depth,
                max_depth,
                acyclic,
                limit,
                offset,
                filter,
                start_vertices,
                target_vertices,
            } => Self::AllPaths {
                storage: storage.clone(),
                space_name: space_name.clone(),
                target_vertex: target_vertex.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                acyclic: *acyclic,
                limit: *limit,
                offset: *offset,
                filter: filter.clone(),
                start_vertices: start_vertices.clone(),
                target_vertices: target_vertices.clone(),
                all_paths: Vec::new(),
                result_iter: None,
            },
            super::super::operator_spec::GraphSpec::MultiShortestPath {
                target_vertices,
                edge_types,
                direction,
                max_depth,
                left_vertex_column,
                right_vertex_column,
                single_shortest,
            } => Self::MultiShortestPath {
                storage,
                space_name,
                target_vertices: target_vertices.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                left_vertex_column: left_vertex_column.clone(),
                right_vertex_column: right_vertex_column.clone(),
                single_shortest: *single_shortest,
                all_paths: Vec::new(),
                result_iter: None,
            },
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Expand { .. }
            | Self::ExpandAll { .. }
            | Self::Traverse { .. }
            | Self::TraverseAll { .. }
            | Self::BiExpand { .. }
            | Self::BiTraverse { .. }
            | Self::ShortestPath { .. }
            | Self::BFSShortest { .. }
            | Self::AllPaths { .. }
            | Self::MultiShortestPath { .. }
            | Self::Subgraph { .. } => {
                input.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());

        match self {
            Self::Expand {
                storage,
                space_name,
                edge_types,
                direction,
                filter_expr,
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("Expand not opened".to_string()));
                }

                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        expand_on_chunk(
                            chunk,
                            &*reader,
                            space_name,
                            edge_types.as_slice(),
                            *direction,
                            filter_expr,
                            cancel_token,
                        )
                    } else {
                        let mut new_cols: Vec<ColumnInfo> = chunk
                            .schema
                            .columns
                            .iter()
                            .map(|c| ColumnInfo {
                                name: c.name.clone(),
                                data_type: c.data_type.clone(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_expand_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        let mut rows = chunk.rows;
                        for row in rows.iter_mut() {
                            row.push(Value::String(edge_types.join("/")));
                            row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                        }
                        let out_col_names = schema
                            .columns
                            .iter()
                            .map(|c| c.name.clone())
                            .collect::<Vec<_>>();
                        rows.retain(|row| row_passes_filter(row, &out_col_names, filter_expr));
                        Ok(Some(DataChunk::new(rows, schema)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::ExpandAll {
                storage,
                space_name,
                edge_types,
                direction,
                filter_expr,
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("ExpandAll not opened".to_string()));
                }

                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        expand_on_chunk(
                            chunk,
                            &*reader,
                            space_name,
                            edge_types.as_slice(),
                            *direction,
                            filter_expr,
                            cancel_token,
                        )
                    } else {
                        let mut new_cols: Vec<ColumnInfo> = chunk
                            .schema
                            .columns
                            .iter()
                            .map(|c| ColumnInfo {
                                name: c.name.clone(),
                                data_type: c.data_type.clone(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_expand_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        let mut rows = chunk.rows;
                        for row in rows.iter_mut() {
                            row.push(Value::String(edge_types.join("/")));
                            row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                        }
                        Ok(Some(DataChunk::new(rows, schema)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::Traverse {
                storage,
                space_name,
                edge_types,
                direction,
                min_depth,
                max_depth,
                visited,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("Traverse not opened".to_string()));
                }

                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let tc = TraversalConfig::traverse(
                            space_name.clone(),
                            *direction,
                            *min_depth,
                            *max_depth,
                            edge_types.clone(),
                        );
                        traverse_on_chunk(chunk, &*reader, &tc, visited, cancel_token)
                    } else {
                        let mut new_cols: Vec<ColumnInfo> = chunk
                            .schema
                            .columns
                            .iter()
                            .map(|c| ColumnInfo {
                                name: c.name.clone(),
                                data_type: c.data_type.clone(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_traverse_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_depth".to_string(),
                            data_type: "bigint".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        let mut rows = chunk.rows;
                        for row in rows.iter_mut() {
                            row.push(Value::String(edge_types.join("/")));
                            row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                            row.push(Value::BigInt(1));
                        }
                        Ok(Some(DataChunk::new(rows, schema)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::TraverseAll { .. } => input.advance(),

            Self::BiExpand {
                storage,
                space_name,
                edge_types,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("BiExpand not opened".to_string()));
                }
                if let Some(chunk) = input.advance()? {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let dir = EdgeDirection::Both;
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let context =
                                ValueRowContext::new_with_layout(row.clone(), chunk.get_layout());
                            let vid_val = context
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                if let Ok(edges) = reader.get_node_edges(space_name, &vid, dir) {
                                    for e in &edges {
                                        let edge_type_matches = edge_types.is_empty()
                                            || edge_types.contains(&"both".to_string())
                                            || edge_types.contains(&e.edge_type);
                                        if !edge_type_matches {
                                            continue;
                                        }
                                        let neighbor_id =
                                            if e.src() == &vid { *e.dst() } else { *e.src() };
                                        if let Ok(Some(vertex)) =
                                            reader.get_vertex(space_name, &neighbor_id)
                                        {
                                            let mut out_row = row.clone();
                                            out_row.push(Value::Vertex(Box::new(vertex)));
                                            out_row.push(Value::String(e.edge_type.clone()));
                                            out_row.push(Value::String("both".to_string()));
                                            out_rows.push(out_row);
                                        }
                                    }
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_expand_vertex".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::BiTraverse {
                storage,
                space_name,
                edge_types,
                min_depth,
                max_depth,
                visited,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("BiTraverse not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let dir = EdgeDirection::Both;
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new_with_layout(row.clone(), chunk.get_layout());
                            let vid_val = ctx
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                let mut frontier = vec![(vid, 0u32)];
                                let mut local_visited = HashSet::new();
                                local_visited.insert(vid);

                                while let Some((current, depth)) = frontier.pop() {
                                    base.ensure_not_cancelled()?;
                                    if depth >= *max_depth {
                                        continue;
                                    }
                                    if let Ok(edges) =
                                        reader.get_node_edges(space_name, &current, dir)
                                    {
                                        for e in &edges {
                                            let edge_type_matches = edge_types.is_empty()
                                                || edge_types.contains(&"both".to_string())
                                                || edge_types.contains(&e.edge_type);
                                            if !edge_type_matches {
                                                continue;
                                            }
                                            let nid = if e.src() == &current {
                                                *e.dst()
                                            } else {
                                                *e.src()
                                            };
                                            let nid_str = format!("{:?}", nid);
                                            if visited.contains(&nid_str)
                                                || local_visited.contains(&nid)
                                            {
                                                continue;
                                            }
                                            local_visited.insert(nid);
                                            visited.insert(nid_str);

                                            if depth + 1 >= *min_depth {
                                                if let Ok(Some(vertex)) =
                                                    reader.get_vertex(space_name, &nid)
                                                {
                                                    let mut out_row = row.clone();
                                                    out_row.push(Value::Vertex(Box::new(vertex)));
                                                    out_row
                                                        .push(Value::String(edge_types.join("/")));
                                                    out_row.push(Value::String("both".to_string()));
                                                    out_row.push(Value::BigInt((depth + 1) as i64));
                                                    out_rows.push(out_row);
                                                }
                                            }
                                            frontier.push((nid, depth + 1));
                                        }
                                    }
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_traverse_vertex".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_depth".to_string(),
                            data_type: "bigint".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::ShortestPath {
                storage,
                space_name,
                target_vertex,
                edge_types,
                max_depth,
                start_vertices,
                target_vertices,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("ShortestPath not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            for (src_val, dst_val) in path_endpoint_pairs(
                                row,
                                chunk.get_layout(),
                                start_vertices,
                                target_vertices,
                                target_vertex.as_ref(),
                            )? {
                                let (Ok(src_vid), Ok(dst_vid)) =
                                    (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                                else {
                                    continue;
                                };
                                let et_ref: Option<&[String]> = if edge_types.is_empty()
                                    || edge_types.contains(&"both".to_string())
                                {
                                    None
                                } else {
                                    Some(edge_types.as_slice())
                                };

                                let cancel_token =
                                    base.runtime.as_ref().map(|rt| rt.cancel_token());
                                let paths = bidir_bfs_shortest_path(
                                    &*reader,
                                    &src_vid,
                                    &dst_vid,
                                    BidirBfsConfig {
                                        space_name,
                                        edge_type_filter: et_ref,
                                        max_depth: *max_depth,
                                        single_shortest: true,
                                        limit: 1,
                                    },
                                    cancel_token.as_deref(),
                                )?;

                                for path in &paths {
                                    base.ensure_not_cancelled()?;
                                    let mut out_row = row.clone();
                                    out_row.push(Value::Path(Box::new(path.clone())));
                                    out_rows.push(out_row);
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_shortest_path".to_string(),
                            data_type: "path".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::BFSShortest {
                storage,
                space_name,
                edge_types,
                max_depth,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("BFSShortest not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new_with_layout(row.clone(), chunk.get_layout());
                            let src_val = ctx
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = ctx
                                .get_variable("dst_vid")
                                .or_else(|| row.get(1).cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));

                            if let (Ok(src_vid), Ok(dst_vid)) =
                                (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                            {
                                let et_ref: Option<&[String]> = if edge_types.is_empty()
                                    || edge_types.contains(&"both".to_string())
                                {
                                    None
                                } else {
                                    Some(edge_types.as_slice())
                                };

                                let cancel_token =
                                    base.runtime.as_ref().map(|rt| rt.cancel_token());
                                let paths = bidir_bfs_shortest_path(
                                    &*reader,
                                    &src_vid,
                                    &dst_vid,
                                    BidirBfsConfig {
                                        space_name,
                                        edge_type_filter: et_ref,
                                        max_depth: *max_depth,
                                        single_shortest: true,
                                        limit: 1,
                                    },
                                    cancel_token.as_deref(),
                                )?;

                                for path in &paths {
                                    base.ensure_not_cancelled()?;
                                    let mut out_row = row.clone();
                                    out_row.push(Value::Path(Box::new(path.clone())));
                                    out_rows.push(out_row);
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_bfs_shortest".to_string(),
                            data_type: "path".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::AllPaths {
                storage,
                space_name,
                target_vertex,
                edge_types,
                direction,
                min_depth,
                max_depth,
                acyclic,
                limit,
                offset,
                filter,
                start_vertices,
                target_vertices,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("AllPaths not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            for (src_val, dst_val) in path_endpoint_pairs(
                                row,
                                chunk.get_layout(),
                                start_vertices,
                                target_vertices,
                                target_vertex.as_ref(),
                            )? {
                                let (Ok(src_vid), Ok(dst_vid)) =
                                    (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                                else {
                                    continue;
                                };
                                let cancel_token =
                                    base.runtime.as_ref().map(|rt| rt.cancel_token());
                                let result_cap =
                                    limit.unwrap_or(usize::MAX).saturating_add(*offset);
                                let paths = enumerate_all_paths(
                                    &*reader,
                                    &src_vid,
                                    &dst_vid,
                                    AllPathsConfig {
                                        space_name,
                                        edge_types,
                                        direction: *direction,
                                        min_depth: *min_depth,
                                        max_depth: *max_depth,
                                        acyclic: *acyclic,
                                        result_cap,
                                    },
                                    cancel_token.as_deref(),
                                )?;

                                for path in paths
                                    .into_iter()
                                    .skip(*offset)
                                    .take(limit.unwrap_or(usize::MAX))
                                {
                                    base.ensure_not_cancelled()?;
                                    let mut out_row = row.clone();
                                    out_row.push(Value::Path(Box::new(path)));
                                    if row_passes_filter(&out_row, &col_names, filter) {
                                        out_rows.push(out_row);
                                    }
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_all_paths".to_string(),
                            data_type: "path".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::MultiShortestPath {
                storage,
                space_name,
                target_vertices,
                edge_types,
                max_depth,
                left_vertex_column,
                right_vertex_column,
                single_shortest,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "MultiShortestPath not opened".to_string(),
                    ));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new_with_layout(row.clone(), chunk.get_layout());
                            let src_val = (!left_vertex_column.is_empty())
                                .then(|| ctx.get_variable(left_vertex_column))
                                .flatten()
                                .or_else(|| ctx.get_variable("vid"))
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let mut dst_values = Vec::new();
                            if let Some(value) = (!right_vertex_column.is_empty())
                                .then(|| ctx.get_variable(right_vertex_column))
                                .flatten()
                                .or_else(|| ctx.get_variable("dst_vid"))
                                .or_else(|| row.get(1).cloned())
                            {
                                dst_values.push(value);
                            }
                            for expression in target_vertices.iter() {
                                let mut expression_context = ValueRowContext::new_with_layout(
                                    row.clone(),
                                    chunk.get_layout(),
                                );
                                dst_values.push(
                                    ExpressionEvaluator::evaluate(
                                        expression,
                                        &mut expression_context,
                                    )
                                    .map_err(|error| {
                                        QueryError::execution(format!(
                                            "MultiShortestPath target evaluation failed: {error}"
                                        ))
                                    })?,
                                );
                            }
                            if let Ok(src_vid) = VertexId::try_from(&src_val) {
                                let et_ref: Option<&[String]> = if edge_types.is_empty()
                                    || edge_types.contains(&"both".to_string())
                                {
                                    None
                                } else {
                                    Some(edge_types.as_slice())
                                };

                                for dst_value in dst_values {
                                    let Ok(dst_vid) = VertexId::try_from(&dst_value) else {
                                        continue;
                                    };
                                    base.ensure_not_cancelled()?;
                                    let cancel_token =
                                        base.runtime.as_ref().map(|rt| rt.cancel_token());
                                    let paths = bidir_bfs_shortest_path(
                                        &*reader,
                                        &src_vid,
                                        &dst_vid,
                                        BidirBfsConfig {
                                            space_name,
                                            edge_type_filter: et_ref,
                                            max_depth: *max_depth,
                                            single_shortest: *single_shortest,
                                            limit: if *single_shortest { 1 } else { 10 },
                                        },
                                        cancel_token.as_deref(),
                                    )?;
                                    for path in &paths {
                                        base.ensure_not_cancelled()?;
                                        let mut out_row = row.clone();
                                        out_row.push(Value::Path(Box::new(path.clone())));
                                        out_rows.push(out_row);
                                    }
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_multi_shortest_path".to_string(),
                            data_type: "path".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::Subgraph {
                storage,
                space_name,
                steps,
                direction,
                edge_types,
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("Subgraph not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new_with_layout(row.clone(), chunk.get_layout());
                            let vid_val = ctx
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));

                            if let Ok(seed_vid) = VertexId::try_from(&vid_val) {
                                let mut visited: HashSet<VertexId> = HashSet::new();
                                let mut history_edges: Vec<(Edge, u32)> = Vec::new();
                                let mut frontier = vec![(seed_vid, 0u32)];
                                visited.insert(seed_vid);

                                while let Some((current, current_step)) = frontier.pop() {
                                    base.ensure_not_cancelled()?;
                                    if current_step >= *steps {
                                        continue;
                                    }
                                    if let Ok(edges) =
                                        reader.get_node_edges(space_name, &current, *direction)
                                    {
                                        let et_set: HashSet<String> =
                                            edge_types.iter().cloned().collect();
                                        for e in &edges {
                                            if !edge_types.is_empty()
                                                && !et_set.contains(&e.edge_type)
                                            {
                                                continue;
                                            }
                                            let neighbor_id = match direction {
                                                EdgeDirection::Out => *e.dst(),
                                                EdgeDirection::In => *e.src(),
                                                EdgeDirection::Both => {
                                                    if e.src() == &current {
                                                        *e.dst()
                                                    } else {
                                                        *e.src()
                                                    }
                                                }
                                            };
                                            history_edges.push((e.clone(), current_step + 1));
                                            if visited.insert(neighbor_id)
                                                && current_step + 1 < *steps
                                            {
                                                frontier.push((neighbor_id, current_step + 1));
                                            }
                                        }
                                    }
                                }

                                for (edge, _step) in &history_edges {
                                    let mut out_row = row.clone();
                                    let src_vertex = reader
                                        .get_vertex(space_name, &edge.src)
                                        .ok()
                                        .flatten()
                                        .unwrap_or_else(|| {
                                            crate::core::vertex_edge_path::Vertex::with_vid(
                                                edge.src,
                                            )
                                        });
                                    let dst_vertex = reader
                                        .get_vertex(space_name, &edge.dst)
                                        .ok()
                                        .flatten()
                                        .unwrap_or_else(|| {
                                            crate::core::vertex_edge_path::Vertex::with_vid(
                                                edge.dst,
                                            )
                                        });
                                    out_row.push(Value::Vertex(Box::new(src_vertex)));
                                    out_row.push(Value::Vertex(Box::new(dst_vertex)));
                                    out_row.push(Value::String(edge.edge_type.clone()));
                                    out_rows.push(out_row);
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_subgraph_src".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_subgraph_dst".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_subgraph_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            match self {
                Self::Expand { .. }
                | Self::ExpandAll { .. }
                | Self::Traverse { .. }
                | Self::TraverseAll { .. }
                | Self::BiExpand { .. }
                | Self::BiTraverse { .. }
                | Self::ShortestPath { .. }
                | Self::BFSShortest { .. }
                | Self::AllPaths { .. }
                | Self::MultiShortestPath { .. }
                | Self::Subgraph { .. } => {
                    input.stop()?;
                    base.lifecycle.mark_stopped();
                }
            }
        }
        Ok(())
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            match self {
                Self::Expand { .. }
                | Self::ExpandAll { .. }
                | Self::Traverse { .. }
                | Self::TraverseAll { .. }
                | Self::BiExpand { .. }
                | Self::BiTraverse { .. }
                | Self::ShortestPath { .. }
                | Self::BFSShortest { .. }
                | Self::AllPaths { .. }
                | Self::MultiShortestPath { .. }
                | Self::Subgraph { .. } => {
                    input.close()?;
                    base.lifecycle.mark_closed();
                }
            }
        }
        Ok(())
    }
}

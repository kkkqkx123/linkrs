use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{Edge, EdgeDirection, NPath, Path, Value, Vertex};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::traversal::config::{TraversalConfig, VisitedPolicy};
use crate::query::executor::traversal::graph_reader::TraversalGraphReader;
use crate::query::executor::traversal::runtime::TraversalRuntime;
use crate::storage::StorageClient;

fn direction_from_str(dir: &str) -> crate::core::EdgeDirection {
    match dir.to_lowercase().as_str() {
        "out" | "outgoing" => crate::core::EdgeDirection::Out,
        "in" | "incoming" => crate::core::EdgeDirection::In,
        _ => crate::core::EdgeDirection::Both,
    }
}

fn row_passes_filter(row: &[Value], col_names: &[String], filter: &Option<Expression>) -> bool {
    let Some(expr) = filter else {
        return true;
    };

    let mut context = ValueRowContext::new(row.to_vec(), col_names.to_vec());
    matches!(
        ExpressionEvaluator::evaluate(expr, &mut context),
        Ok(Value::Bool(true))
    )
}

fn bidir_bfs_shortest_path(
    storage: &dyn StorageClient,
    space_name: &str,
    start_id: &VertexId,
    end_id: &VertexId,
    edge_type_filter: Option<&[String]>,
    max_depth: usize,
    single_shortest: bool,
    limit: usize,
) -> Result<Vec<Path>, QueryError> {
    let mut result_paths = Vec::new();

    let mut left_visited: HashMap<VertexId, Arc<NPath>> = HashMap::new();
    let mut right_visited: HashMap<VertexId, Arc<NPath>> = HashMap::new();
    let mut left_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();
    let mut right_queue: VecDeque<(VertexId, Arc<NPath>)> = VecDeque::new();

    if let Ok(Some(start_vertex)) = storage.get_vertex(space_name, start_id) {
        let np = Arc::new(NPath::new(Arc::new(start_vertex)));
        left_queue.push_back((*start_id, np.clone()));
        left_visited.insert(*start_id, np);
    }
    if let Ok(Some(end_vertex)) = storage.get_vertex(space_name, end_id) {
        let np = Arc::new(NPath::new(Arc::new(end_vertex)));
        right_queue.push_back((*end_id, np.clone()));
        right_visited.insert(*end_id, np);
    }

    let dir_out = EdgeDirection::Out;
    let dir_in = EdgeDirection::In;

    while !left_queue.is_empty() && !right_queue.is_empty() {
        if single_shortest && !result_paths.is_empty() {
            break;
        }
        if result_paths.len() >= limit {
            break;
        }

        let left_level = left_queue.len();
        let mut left_next: Vec<(VertexId, Arc<NPath>)> = Vec::new();
        for _ in 0..left_level {
            if let Some((current_id, current_npath)) = left_queue.pop_front() {
                if current_npath.len() >= max_depth {
                    continue;
                }
                if let Ok(edges) = storage.get_node_edges(space_name, &current_id, dir_out) {
                    let filtered: Vec<&Edge> = if let Some(types) = edge_type_filter {
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
                            storage.get_vertex(space_name, neighbor_id)
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

        if single_shortest && !result_paths.is_empty() {
            break;
        }
        if result_paths.len() >= limit {
            break;
        }

        let right_level = right_queue.len();
        let mut right_next: Vec<(VertexId, Arc<NPath>)> = Vec::new();
        for _ in 0..right_level {
            if let Some((current_id, current_npath)) = right_queue.pop_front() {
                if current_npath.len() >= max_depth {
                    continue;
                }

                if let Some(left_npath) = left_visited.get(&current_id) {
                    let total_len = left_npath.len() + current_npath.len();
                    if total_len <= max_depth {
                        let mut left_path = left_npath.to_path();
                        let mut right_path = current_npath.to_path();
                        right_path.reverse();
                        left_path.steps.extend(right_path.steps);
                        result_paths.push(left_path);
                        if single_shortest || result_paths.len() >= limit {
                            break;
                        }
                    }
                    continue;
                }

                if let Ok(edges) = storage.get_node_edges(space_name, &current_id, dir_in) {
                    let filtered: Vec<&Edge> = if let Some(types) = edge_type_filter {
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
                            storage.get_vertex(space_name, neighbor_id)
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
                if total_len <= max_depth {
                    let mut left_path = left_npath.to_path();
                    let mut right_path = np.to_path();
                    right_path.reverse();
                    left_path.steps.extend(right_path.steps);
                    result_paths.push(left_path);
                    if single_shortest || result_paths.len() >= limit {
                        break;
                    }
                }
            } else {
                right_queue.push_back((id, np));
            }
        }
    }

    if single_shortest && !result_paths.is_empty() {
        result_paths.sort_by_key(|a| a.steps.len());
        result_paths.truncate(1);
    }
    result_paths.truncate(limit);
    Ok(result_paths)
}

// ============ Expand ============

pub fn open_expand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Expand { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_expand".to_string(),
        )),
    }
}

fn expand_on_chunk(
    chunk: DataChunk,
    reader: &dyn StorageClient,
    space_name: &str,
    edge_type: &str,
    direction: &str,
    filter_expr: &Option<Expression>,
) -> Result<Option<DataChunk>, QueryError> {
    let dir = direction_from_str(direction);
    let col_names = chunk.col_names();
    let greader = TraversalGraphReader::new(reader);

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), col_names.clone());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let config = TraversalConfig::expand(
                space_name.to_string(),
                dir,
            );
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config);

            if let Ok(Some(vertex)) = reader.get_vertex(space_name, &vid) {
                runtime.seed_from_vertex(vertex);
            } else {
                continue;
            }

            while let Some(event) = runtime.next_event() {
                let mut out_row = row.clone();
                out_row.push(Value::Vertex(Box::new(event.vertex)));
                out_row.push(Value::String(edge_type.to_string()));
                out_row.push(Value::String(direction.to_string()));
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

pub fn next_expand(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Expand {
            input,
            storage,
            space_name,
            edge_type,
            direction,
            filter_expr,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("Expand not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    expand_on_chunk(
                        chunk, &*reader, space_name, edge_type, direction, filter_expr,
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
                        row.push(Value::String(edge_type.clone()));
                        row.push(Value::String(direction.clone()));
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_expand".to_string(),
        )),
    }
}

pub fn stop_expand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Expand { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_expand".to_string(),
        )),
    }
}

pub fn close_expand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Expand { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_expand".to_string(),
        )),
    }
}

// ============ ExpandAll ============

pub fn open_expandall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ExpandAll { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_expandall".to_string(),
        )),
    }
}

pub fn next_expandall(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ExpandAll {
            input,
            storage,
            space_name,
            edge_type,
            direction,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("ExpandAll not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    expand_on_chunk(
                        chunk, &*reader, space_name, edge_type, direction, &None,
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
                        row.push(Value::String(edge_type.clone()));
                        row.push(Value::String(direction.clone()));
                    }
                    Ok(Some(DataChunk::new(rows, schema)))
                }
            } else {
                Ok(None)
            }
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_expandall".to_string(),
        )),
    }
}

pub fn stop_expandall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ExpandAll { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_expandall".to_string(),
        )),
    }
}

pub fn close_expandall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ExpandAll { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_expandall".to_string(),
        )),
    }
}

// ============ Traverse ============

pub fn open_traverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Traverse { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_traverse".to_string(),
        )),
    }
}

fn traverse_on_chunk(
    chunk: DataChunk,
    reader: &dyn StorageClient,
    space_name: &str,
    edge_type: &str,
    direction: &str,
    min_depth: u32,
    max_depth: u32,
    visited: &mut HashSet<String>,
) -> Result<Option<DataChunk>, QueryError> {
    let dir = direction_from_str(direction);
    let col_names = chunk.col_names();

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), col_names.clone());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));
        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let config = TraversalConfig::traverse(
                space_name.to_string(),
                dir,
                min_depth,
                max_depth,
                vec![edge_type.to_string()],
            );
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config);

            if let Ok(Some(vertex)) = reader.get_vertex(space_name, &vid) {
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
                out_row.push(Value::String(direction.to_string()));
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

pub fn next_traverse(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Traverse {
            input,
            storage,
            space_name,
            edge_type,
            direction,
            min_depth,
            max_depth,
            visited,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("Traverse not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    traverse_on_chunk(
                        chunk,
                        &*reader,
                        space_name,
                        edge_type,
                        direction,
                        *min_depth,
                        *max_depth,
                        visited,
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
                        row.push(Value::String(edge_type.clone()));
                        row.push(Value::String(direction.clone()));
                        row.push(Value::BigInt(1));
                    }
                    Ok(Some(DataChunk::new(rows, schema)))
                }
            } else {
                Ok(None)
            }
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_traverse".to_string(),
        )),
    }
}

pub fn stop_traverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Traverse { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_traverse".to_string(),
        )),
    }
}

pub fn close_traverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Traverse { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_traverse".to_string(),
        )),
    }
}

// ============ TraverseAll ============

pub fn open_traverseall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TraverseAll { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_traverseall".to_string(),
        )),
    }
}

pub fn next_traverseall(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TraverseAll { input, .. } => input.next(),
        _ => Err(QueryError::execution(
            "Type mismatch in next_traverseall".to_string(),
        )),
    }
}

pub fn stop_traverseall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TraverseAll { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_traverseall".to_string(),
        )),
    }
}

pub fn close_traverseall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TraverseAll { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_traverseall".to_string(),
        )),
    }
}

pub fn open_appendvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AppendVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_appendvertices".to_string(),
        )),
    }
}

pub fn next_appendvertices(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    use crate::query::executor::expression::evaluator::ExpressionEvaluator;

    match executor {
        StreamingExecutor::AppendVertices {
            input,
            vertex_properties,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution(
                    "AppendVertices not opened".to_string(),
                ));
            }
            if let Some(chunk) = input.next()? {
                let col_names = chunk.col_names();
                let mut result_rows = Vec::new();
                for row in chunk.rows {
                    let mut new_row = row.clone();
                    let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
                    for (_prop_name, expr) in vertex_properties.iter() {
                        match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                            Ok(val) => new_row.push(val),
                            Err(_) => new_row.push(Value::Null(crate::core::NullType::Null)),
                        }
                    }
                    result_rows.push(new_row);
                }
                Ok(Some(DataChunk::from_rows(result_rows)))
            } else {
                Ok(None)
            }
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_appendvertices".to_string(),
        )),
    }
}

pub fn stop_appendvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AppendVertices { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_appendvertices".to_string(),
        )),
    }
}

pub fn close_appendvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AppendVertices { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_appendvertices".to_string(),
        )),
    }
}

pub fn open_biexpand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiExpand { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_biexpand".to_string(),
        )),
    }
}

pub fn next_biexpand(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BiExpand {
            input,
            storage,
            space_name,
            edge_type,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BiExpand not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let dir = direction_from_str("both");
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let context = ValueRowContext::new(row.clone(), col_names.clone());
                        let vid_val = context
                            .get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        if let Ok(vid) = VertexId::try_from(&vid_val) {
                            if let Ok(edges) = reader.get_node_edges(space_name, &vid, dir) {
                                for e in &edges {
                                    let edge_type_matches = edge_type.is_empty()
                                        || *edge_type == "both"
                                        || e.edge_type == *edge_type;
                                    if !edge_type_matches {
                                        continue;
                                    }
                                    let neighbor_id = if e.src() == &vid {
                                        e.dst().clone()
                                    } else {
                                        e.src().clone()
                                    };
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_biexpand".to_string(),
        )),
    }
}

pub fn stop_biexpand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiExpand { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_biexpand".to_string(),
        )),
    }
}

pub fn close_biexpand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiExpand { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_biexpand".to_string(),
        )),
    }
}

pub fn open_bitraverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiTraverse { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_bitraverse".to_string(),
        )),
    }
}

pub fn next_bitraverse(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BiTraverse {
            input,
            storage,
            space_name,
            edge_type,
            min_depth,
            max_depth,
            visited,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BiTraverse not opened".to_string()));
            }
            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let dir = direction_from_str("both");
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let ctx = ValueRowContext::new(row.clone(), col_names.clone());
                        let vid_val = ctx
                            .get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        if let Ok(vid) = VertexId::try_from(&vid_val) {
                            let mut frontier = vec![(vid, 0u32)];
                            let mut local_visited = HashSet::new();
                            local_visited.insert(vid);

                            while let Some((current, depth)) = frontier.pop() {
                                if depth >= *max_depth {
                                    continue;
                                }
                                if let Ok(edges) = reader.get_node_edges(space_name, &current, dir)
                                {
                                    for e in &edges {
                                        let edge_type_matches = edge_type.is_empty()
                                            || *edge_type == "both"
                                            || e.edge_type == *edge_type;
                                        if !edge_type_matches {
                                            continue;
                                        }
                                        let nid = if e.src() == &current {
                                            e.dst().clone()
                                        } else {
                                            e.src().clone()
                                        };
                                        let nid_str = format!("{:?}", nid);
                                        if visited.contains(&nid_str)
                                            || local_visited.contains(&nid)
                                        {
                                            continue;
                                        }
                                        local_visited.insert(nid.clone());
                                        visited.insert(nid_str);

                                        if depth + 1 >= *min_depth {
                                            if let Ok(Some(vertex)) =
                                                reader.get_vertex(space_name, &nid)
                                            {
                                                let mut out_row = row.clone();
                                                out_row.push(Value::Vertex(Box::new(vertex)));
                                                out_row.push(Value::String(edge_type.clone()));
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_bitraverse".to_string(),
        )),
    }
}

pub fn stop_bitraverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiTraverse { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_bitraverse".to_string(),
        )),
    }
}

pub fn close_bitraverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiTraverse { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_bitraverse".to_string(),
        )),
    }
}

// ============ ShortestPath (Bidirectional BFS) ============

pub fn open_shortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShortestPath { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_shortestpath".to_string(),
        )),
    }
}

pub fn next_shortestpath(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ShortestPath {
            input,
            storage,
            space_name,
            edge_type,
            direction: _direction,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("ShortestPath not opened".to_string()));
            }
            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let ctx = ValueRowContext::new(row.clone(), col_names.clone());
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
                            let et_filter: Option<Vec<String>> =
                                if edge_type.is_empty() || edge_type == "both" {
                                    None
                                } else {
                                    Some(vec![edge_type.clone()])
                                };
                            let et_ref = et_filter.as_ref().map(|v| v.as_slice());

                            let paths = bidir_bfs_shortest_path(
                                &*reader, space_name, &src_vid, &dst_vid, et_ref, 10, false, 100,
                            )?;

                            for path in &paths {
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_shortestpath".to_string(),
        )),
    }
}

pub fn stop_shortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShortestPath { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_shortestpath".to_string(),
        )),
    }
}

pub fn close_shortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShortestPath { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_shortestpath".to_string(),
        )),
    }
}

// ============ BFSShortest ============

pub fn open_bfsshortest(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BFSShortest { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_bfsshortest".to_string(),
        )),
    }
}

pub fn next_bfsshortest(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BFSShortest {
            input,
            storage,
            space_name,
            edge_type,
            direction: _direction,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BFSShortest not opened".to_string()));
            }
            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let ctx = ValueRowContext::new(row.clone(), col_names.clone());
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
                            let et_filter: Option<Vec<String>> =
                                if edge_type.is_empty() || edge_type == "both" {
                                    None
                                } else {
                                    Some(vec![edge_type.clone()])
                                };
                            let et_ref = et_filter.as_ref().map(|v| v.as_slice());

                            let paths = bidir_bfs_shortest_path(
                                &*reader, space_name, &src_vid, &dst_vid, et_ref, 20, true, 10,
                            )?;

                            for path in &paths {
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_bfsshortest".to_string(),
        )),
    }
}

pub fn stop_bfsshortest(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BFSShortest { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_bfsshortest".to_string(),
        )),
    }
}

pub fn close_bfsshortest(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BFSShortest { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_bfsshortest".to_string(),
        )),
    }
}

// ============ AllPaths ============

pub fn open_allpaths(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AllPaths { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_allpaths".to_string(),
        )),
    }
}

pub fn next_allpaths(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::AllPaths {
            input,
            storage,
            space_name,
            edge_type,
            direction: _direction,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("AllPaths not opened".to_string()));
            }
            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let ctx = ValueRowContext::new(row.clone(), col_names.clone());
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
                            let et_filter: Option<Vec<String>> =
                                if edge_type.is_empty() || edge_type == "both" {
                                    None
                                } else {
                                    Some(vec![edge_type.clone()])
                                };
                            let et_ref = et_filter.as_ref().map(|v| v.as_slice());

                            let paths = bidir_bfs_shortest_path(
                                &*reader, space_name, &src_vid, &dst_vid, et_ref, 10, false, 100,
                            )?;

                            for path in &paths {
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_allpaths".to_string(),
        )),
    }
}

pub fn stop_allpaths(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AllPaths { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_allpaths".to_string(),
        )),
    }
}

pub fn close_allpaths(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AllPaths { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_allpaths".to_string(),
        )),
    }
}

// ============ MultiShortestPath ============

pub fn open_multishortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_multishortestpath".to_string(),
        )),
    }
}

pub fn next_multishortestpath(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    use crate::query::executor::expression::evaluator::ExpressionEvaluator;

    match executor {
        StreamingExecutor::MultiShortestPath {
            input,
            storage,
            space_name,
            target_vertices,
            edge_type,
            direction: _direction,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution(
                    "MultiShortestPath not opened".to_string(),
                ));
            }
            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();

                    let mut dst_vids = Vec::new();
                    for expr in target_vertices.iter() {
                        let mut ctx = ValueRowContext::new(vec![], col_names.clone());
                        if let Ok(val) = ExpressionEvaluator::evaluate(expr, &mut ctx) {
                            if let Ok(vid) = VertexId::try_from(&val) {
                                dst_vids.push(vid);
                            }
                        }
                    }

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let ctx = ValueRowContext::new(row.clone(), col_names.clone());
                        let src_val = ctx
                            .get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        if let Ok(src_vid) = VertexId::try_from(&src_val) {
                            let et_filter: Option<Vec<String>> =
                                if edge_type.is_empty() || edge_type == "both" {
                                    None
                                } else {
                                    Some(vec![edge_type.clone()])
                                };
                            let et_ref = et_filter.as_ref().map(|v| v.as_slice());

                            for dst_vid in &dst_vids {
                                let paths = bidir_bfs_shortest_path(
                                    &*reader, space_name, &src_vid, dst_vid, et_ref, 10, true, 10,
                                )?;
                                for path in &paths {
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_multishortestpath".to_string(),
        )),
    }
}

pub fn stop_multishortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_multishortestpath".to_string(),
        )),
    }
}

pub fn close_multishortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_multishortestpath".to_string(),
        )),
    }
}

// ============ Subgraph ============

pub fn open_subgraph(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Subgraph { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_subgraph".to_string(),
        )),
    }
}

pub fn next_subgraph(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Subgraph {
            input,
            storage,
            space_name,
            steps,
            direction,
            edge_types,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("Subgraph not opened".to_string()));
            }
            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let dir = direction_from_str(direction);
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let ctx = ValueRowContext::new(row.clone(), col_names.clone());
                        let vid_val = ctx
                            .get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));

                        if let Ok(seed_vid) = VertexId::try_from(&vid_val) {
                            let mut visited: HashSet<VertexId> = HashSet::new();
                            let mut history_edges: Vec<(crate::core::Edge, u32)> = Vec::new();
                            let mut frontier = vec![(seed_vid, 0u32)];
                            visited.insert(seed_vid);

                            while let Some((current, current_step)) = frontier.pop() {
                                if current_step >= *steps {
                                    continue;
                                }
                                if let Ok(edges) = reader.get_node_edges(space_name, &current, dir)
                                {
                                    let et_set: HashSet<String> =
                                        edge_types.iter().cloned().collect();
                                    for e in &edges {
                                        if !edge_types.is_empty() && !et_set.contains(&e.edge_type)
                                        {
                                            continue;
                                        }
                                        let neighbor_id = match dir {
                                            crate::core::EdgeDirection::Out => e.dst().clone(),
                                            crate::core::EdgeDirection::In => e.src().clone(),
                                            crate::core::EdgeDirection::Both => {
                                                if e.src() == &current {
                                                    e.dst().clone()
                                                } else {
                                                    e.src().clone()
                                                }
                                            }
                                        };
                                        history_edges.push((e.clone(), current_step + 1));
                                        if visited.insert(neighbor_id) && current_step + 1 < *steps
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
                                        crate::core::vertex_edge_path::Vertex::with_vid(edge.src)
                                    });
                                let dst_vertex = reader
                                    .get_vertex(space_name, &edge.dst)
                                    .ok()
                                    .flatten()
                                    .unwrap_or_else(|| {
                                        crate::core::vertex_edge_path::Vertex::with_vid(edge.dst)
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
        _ => Err(QueryError::execution(
            "Type mismatch in next_subgraph".to_string(),
        )),
    }
}

pub fn stop_subgraph(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Subgraph { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_subgraph".to_string(),
        )),
    }
}

pub fn close_subgraph(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Subgraph { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_subgraph".to_string(),
        )),
    }
}

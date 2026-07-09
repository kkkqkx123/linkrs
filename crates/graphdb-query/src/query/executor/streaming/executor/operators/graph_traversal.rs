use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::Value;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::storage::StorageClient;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;

fn direction_from_str(dir: &str) -> crate::core::EdgeDirection {
    match dir.to_lowercase().as_str() {
        "out" | "outgoing" => crate::core::EdgeDirection::Out,
        "in" | "incoming" => crate::core::EdgeDirection::In,
        _ => crate::core::EdgeDirection::Both,
    }
}

fn read_neighbor(
    storage: &dyn StorageClient,
    space_name: &str,
    vertex_id: &VertexId,
    direction: crate::core::EdgeDirection,
) -> Vec<crate::core::vertex_edge_path::Vertex> {
    let mut result = Vec::new();
    if let Ok(edges) = storage.get_node_edges(space_name, vertex_id, direction) {
        for e in &edges {
            let neighbor_id = match direction {
                crate::core::EdgeDirection::Out => e.dst(),
                crate::core::EdgeDirection::In => e.src(),
                crate::core::EdgeDirection::Both => {
                    if e.src() == vertex_id { e.dst() } else { e.src() }
                }
            };
            if let Ok(Some(vertex)) = storage.get_vertex(space_name, neighbor_id) {
                result.push(vertex);
            }
        }
    }
    result
}

// ============ Expand ============

pub fn open_expand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Expand { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_expand".to_string())),
    }
}

pub fn next_expand(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Expand {
            input,
            storage,
            space_name,
            edge_type,
            direction,
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
                    let dir = direction_from_str(direction);
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let context = ValueRowContext::new(row.clone(), col_names.clone());
                        let vid_val = context.get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));

                        if let Ok(vid) = VertexId::try_from(&vid_val) {
                            let neighbors = read_neighbor(&*reader, space_name, &vid, dir);
                            for neighbor in neighbors {
                                let mut out_row = row.clone();
                                out_row.push(Value::Vertex(Box::new(neighbor)));
                                out_row.push(Value::String(edge_type.clone()));
                                out_row.push(Value::String(direction.clone()));
                                out_rows.push(out_row);
                            }
                        }
                    }

                    if out_rows.is_empty() {
                        return Ok(None);
                    }

                    let mut new_cols: Vec<ColumnInfo> = col_names.iter().map(|n| ColumnInfo {
                        name: n.clone(),
                        data_type: "string".to_string(),
                    }).collect();
                    new_cols.push(ColumnInfo { name: "_expand_vertex".to_string(), data_type: "vertex".to_string() });
                    new_cols.push(ColumnInfo { name: "_expand_edge_type".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_expand_direction".to_string(), data_type: "string".to_string() });
                    let schema = Arc::new(Schema::new(new_cols));
                    Ok(Some(DataChunk::new(out_rows, schema)))
                } else {
                    let mut new_cols: Vec<ColumnInfo> = chunk.schema.columns.iter().map(|c| ColumnInfo {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                    }).collect();
                    new_cols.push(ColumnInfo { name: "_expand_edge_type".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_expand_direction".to_string(), data_type: "string".to_string() });
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
        _ => Err(QueryError::execution("Type mismatch in next_expand".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_expand".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_expand".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_expandall".to_string())),
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
                    let dir = direction_from_str(direction);
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let context = ValueRowContext::new(row.clone(), col_names.clone());
                        let vid_val = context.get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        if let Ok(vid) = VertexId::try_from(&vid_val) {
                            let neighbors = read_neighbor(&*reader, space_name, &vid, dir);
                            for neighbor in neighbors {
                                let mut out_row = row.clone();
                                out_row.push(Value::Vertex(Box::new(neighbor)));
                                out_row.push(Value::String(edge_type.clone()));
                                out_row.push(Value::String(direction.clone()));
                                out_rows.push(out_row);
                            }
                        }
                    }

                    if out_rows.is_empty() {
                        return Ok(None);
                    }

                    let mut new_cols: Vec<ColumnInfo> = col_names.iter().map(|n| ColumnInfo {
                        name: n.clone(),
                        data_type: "string".to_string(),
                    }).collect();
                    new_cols.push(ColumnInfo { name: "_expand_vertex".to_string(), data_type: "vertex".to_string() });
                    new_cols.push(ColumnInfo { name: "_expand_edge_type".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_expand_direction".to_string(), data_type: "string".to_string() });
                    let schema = Arc::new(Schema::new(new_cols));
                    Ok(Some(DataChunk::new(out_rows, schema)))
                } else {
                    let mut new_cols: Vec<ColumnInfo> = chunk.schema.columns.iter().map(|c| ColumnInfo {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                    }).collect();
                    new_cols.push(ColumnInfo { name: "_expand_edge_type".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_expand_direction".to_string(), data_type: "string".to_string() });
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
        _ => Err(QueryError::execution("Type mismatch in next_expandall".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_expandall".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_expandall".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_traverse".to_string())),
    }
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
                    let dir = direction_from_str(direction);
                    let col_names = chunk.col_names();

                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        let context = ValueRowContext::new(row.clone(), col_names.clone());
                        let vid_val = context.get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        if let Ok(vid) = VertexId::try_from(&vid_val) {
                            let mut frontier = vec![(vid, 0u32)];
                            let mut local_visited = std::collections::HashSet::new();
                            local_visited.insert(vid.clone());

                            while let Some((current, depth)) = frontier.pop() {
                                if depth >= *max_depth {
                                    continue;
                                }
                                if let Ok(edges) = reader.get_node_edges(space_name, &current, dir) {
                                    for e in &edges {
                                        let nid = match dir {
                                            crate::core::EdgeDirection::Out => e.dst().clone(),
                                            crate::core::EdgeDirection::In => e.src().clone(),
                                            crate::core::EdgeDirection::Both => {
                                                if e.src() == &current { e.dst().clone() } else { e.src().clone() }
                                            }
                                        };
                                        let nid_str = format!("{:?}", nid);
                                        if visited.contains(&nid_str) || local_visited.contains(&nid) {
                                            continue;
                                        }
                                        local_visited.insert(nid.clone());
                                        visited.insert(nid_str);

                                        if depth + 1 >= *min_depth {
                                            if let Ok(Some(vertex)) = reader.get_vertex(space_name, &nid) {
                                                let mut out_row = row.clone();
                                                out_row.push(Value::Vertex(Box::new(vertex)));
                                                out_row.push(Value::String(edge_type.clone()));
                                                out_row.push(Value::String(direction.clone()));
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

                    let mut new_cols: Vec<ColumnInfo> = col_names.iter().map(|n| ColumnInfo {
                        name: n.clone(),
                        data_type: "string".to_string(),
                    }).collect();
                    new_cols.push(ColumnInfo { name: "_traverse_vertex".to_string(), data_type: "vertex".to_string() });
                    new_cols.push(ColumnInfo { name: "_traverse_edge_type".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_traverse_direction".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_traverse_depth".to_string(), data_type: "bigint".to_string() });
                    let schema = Arc::new(Schema::new(new_cols));
                    Ok(Some(DataChunk::new(out_rows, schema)))
                } else {
                    let mut new_cols: Vec<ColumnInfo> = chunk.schema.columns.iter().map(|c| ColumnInfo {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                    }).collect();
                    new_cols.push(ColumnInfo { name: "_traverse_edge_type".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_traverse_direction".to_string(), data_type: "string".to_string() });
                    new_cols.push(ColumnInfo { name: "_traverse_depth".to_string(), data_type: "bigint".to_string() });
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
        _ => Err(QueryError::execution("Type mismatch in next_traverse".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_traverse".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_traverse".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_traverseall".to_string())),
    }
}

pub fn next_traverseall(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TraverseAll { input, .. } => {
            input.next()
        }
        _ => Err(QueryError::execution("Type mismatch in next_traverseall".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_traverseall".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_traverseall".to_string())),
    }
}

// Remaining operators (AppendVertices, BiExpand, BiTraverse, ShortestPath, BFSShortest, AllPaths, MultiShortestPath)
// retain their existing stub/passthrough behavior. Full algorithms integration is Phase B+.

pub fn open_appendvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AppendVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_appendvertices".to_string())),
    }
}

pub fn next_appendvertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::AppendVertices { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("AppendVertices not opened".to_string()));
            }
            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                Ok(Some(chunk))
            } else {
                Ok(None)
            }
        }
        _ => Err(QueryError::execution("Type mismatch in next_appendvertices".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_appendvertices".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_appendvertices".to_string())),
    }
}

pub fn open_biexpand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiExpand { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_biexpand".to_string())),
    }
}

pub fn next_biexpand(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BiExpand { input, .. } => {
            input.next()
        }
        _ => Err(QueryError::execution("Type mismatch in next_biexpand".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_biexpand".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_biexpand".to_string())),
    }
}

pub fn open_bitraverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiTraverse { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_bitraverse".to_string())),
    }
}

pub fn next_bitraverse(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BiTraverse { input, .. } => {
            input.next()
        }
        _ => Err(QueryError::execution("Type mismatch in next_bitraverse".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_bitraverse".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_bitraverse".to_string())),
    }
}

pub fn open_shortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShortestPath { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_shortestpath".to_string())),
    }
}

pub fn next_shortestpath(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ShortestPath { input, .. } => {
            input.next()
        }
        _ => Err(QueryError::execution("Type mismatch in next_shortestpath".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_shortestpath".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_shortestpath".to_string())),
    }
}

pub fn open_bfsshortest(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BFSShortest { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_bfsshortest".to_string())),
    }
}

pub fn next_bfsshortest(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BFSShortest { input, .. } => {
            input.next()
        }
        _ => Err(QueryError::execution("Type mismatch in next_bfsshortest".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_bfsshortest".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_bfsshortest".to_string())),
    }
}

pub fn open_allpaths(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AllPaths { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_allpaths".to_string())),
    }
}

pub fn next_allpaths(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::AllPaths { input, .. } => {
            input.next()
        }
        _ => Err(QueryError::execution("Type mismatch in next_allpaths".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_allpaths".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_allpaths".to_string())),
    }
}

pub fn open_multishortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_multishortestpath".to_string())),
    }
}

pub fn next_multishortestpath(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath { input, .. } => {
            input.next()
        }
        _ => Err(QueryError::execution("Type mismatch in next_multishortestpath".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in stop_multishortestpath".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_multishortestpath".to_string())),
    }
}

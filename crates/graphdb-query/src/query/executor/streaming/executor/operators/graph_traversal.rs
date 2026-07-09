//! Graph traversal operator implementations
//!
//! Includes: Expand, ExpandAll, Traverse, TraverseAll, AppendVertices,
//! BiExpand, BiTraverse, ShortestPath, BFSShortest, AllPaths, MultiShortestPath
//!
//! Note: Full traversal logic requires storage integration. These implementations
//! provide the operator framework and state machine; neighbor lookup will be
//! plugged in when storage integration is complete.

use std::sync::Arc;
use crate::core::error::QueryError;
use crate::core::{NullType, Value};
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use super::super::StreamingExecutor;

/// Add metadata columns to a DataChunk by creating a new schema.
fn add_column_names(chunk: &mut DataChunk, extra_names: &[&str]) {
    let mut names: Vec<String> = chunk.schema.columns.iter().map(|c| c.name.clone()).collect();
    for n in extra_names {
        names.push(n.to_string());
    }
    let new_cols: Vec<ColumnInfo> = names.iter().map(|n| ColumnInfo {
        name: n.clone(),
        data_type: "string".to_string(),
    }).collect();
    chunk.schema = Arc::new(Schema::new(new_cols));
}

// ============ Expand ============
// Takes input vertex rows and appends edge expansion columns.
// Without storage, acts as passthrough with state validation.

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
        StreamingExecutor::Expand { input, opened, edge_type, direction, .. } => {
            if !*opened {
                return Err(QueryError::execution("Expand not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                add_column_names(&mut chunk, &["_expand_edge_type", "_expand_direction"]);
                for row in chunk.rows.iter_mut() {
                    row.push(Value::String(edge_type.clone()));
                    row.push(Value::String(direction.clone()));
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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
        StreamingExecutor::ExpandAll { input, opened, edge_type, direction, .. } => {
            if !*opened {
                return Err(QueryError::execution("ExpandAll not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                add_column_names(&mut chunk, &["_expandall_edge_type", "_expandall_direction"]);
                for row in chunk.rows.iter_mut() {
                    row.push(Value::String(edge_type.clone()));
                    row.push(Value::String(direction.clone()));
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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
// Multi-step graph traversal with depth tracking.
// Maintains visited set and depth bounds.

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
            input, opened, edge_type, direction,
            min_depth, max_depth, visited, ..
        } => {
            if !*opened {
                return Err(QueryError::execution("Traverse not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                // Track visited vertices (using first column as vertex ID)
                for row in &chunk.rows {
                    if let Some(Value::String(vid)) = row.first() {
                        visited.insert(vid.clone());
                    }
                }
                add_column_names(&mut chunk, &["_traverse_edge_type", "_traverse_direction", "_min_depth", "_max_depth"]);
                for row in chunk.rows.iter_mut() {
                    row.push(Value::String(edge_type.clone()));
                    row.push(Value::String(direction.clone()));
                    row.push(Value::BigInt(*min_depth as i64));
                    row.push(Value::BigInt(*max_depth as i64));
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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
        StreamingExecutor::TraverseAll {
            input, opened, visited, ..
        } => {
            if !*opened {
                return Err(QueryError::execution("TraverseAll not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                for row in &chunk.rows {
                    if let Some(Value::String(vid)) = row.first() {
                        visited.insert(vid.clone());
                    }
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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

// ============ AppendVertices ============

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
        StreamingExecutor::AppendVertices { input, opened, vertex_properties, .. } => {
            if !*opened {
                return Err(QueryError::execution("AppendVertices not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                let prop_count = vertex_properties.len();
                let extra: Vec<&str> = vertex_properties.iter().map(|(name, _)| name.as_str()).collect();
                add_column_names(&mut chunk, &extra);
                for row in chunk.rows.iter_mut() {
                    for _ in 0..prop_count {
                        row.push(Value::Null(NullType::Null));
                    }
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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

// ============ BiExpand ============

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
        StreamingExecutor::BiExpand { input, opened, edge_type, .. } => {
            if !*opened {
                return Err(QueryError::execution("BiExpand not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                add_column_names(&mut chunk, &["_biexpand_edge_type"]);
                for row in chunk.rows.iter_mut() {
                    row.push(Value::String(edge_type.clone()));
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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

// ============ BiTraverse ============

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
        StreamingExecutor::BiTraverse {
            input, opened, visited, ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BiTraverse not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                for row in &chunk.rows {
                    if let Some(Value::String(vid)) = row.first() {
                        visited.insert(vid.clone());
                    }
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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

// ============ ShortestPath ============
// Finds shortest path between source and target vertices using BFS.
// Without storage, operates as identity on input.

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
        StreamingExecutor::ShortestPath {
            input, opened, edge_type, direction, ..
        } => {
            if !*opened {
                return Err(QueryError::execution("ShortestPath not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                add_column_names(&mut chunk, &["_shortestpath_edge_type", "_shortestpath_direction"]);
                for row in chunk.rows.iter_mut() {
                    row.push(Value::String(edge_type.clone()));
                    row.push(Value::String(direction.clone()));
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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

// ============ BFSShortest ============
// BFS-based shortest path finding. Maintains frontier and visited sets.

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
        StreamingExecutor::BFSShortest {
            input, opened, edge_type, direction,
            frontier, visited, ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BFSShortest not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                for row in &chunk.rows {
                    if let Some(Value::String(vid)) = row.first() {
                        if visited.insert(vid.clone()) {
                            frontier.push(row.clone());
                        }
                    }
                }
                add_column_names(&mut chunk, &["_bfs_edge_type", "_bfs_direction", "_frontier_size"]);
                for row in chunk.rows.iter_mut() {
                    row.push(Value::String(edge_type.clone()));
                    row.push(Value::String(direction.clone()));
                    row.push(Value::BigInt(frontier.len() as i64));
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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

// ============ AllPaths ============

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
        StreamingExecutor::AllPaths {
            input, opened, edge_type, direction,
            all_paths, result_iter, ..
        } => {
            if !*opened {
                return Err(QueryError::execution("AllPaths not opened".to_string()));
            }

            // If we have buffered results, drain them first
            if let Some(iter) = result_iter {
                let rows: Vec<Vec<Value>> = iter.collect();
                if !rows.is_empty() {
                    let schema = Arc::new(Schema::new(vec![
                        ColumnInfo { name: "_path_node".to_string(), data_type: "string".to_string() },
                        ColumnInfo { name: "_allpaths_edge_type".to_string(), data_type: "string".to_string() },
                        ColumnInfo { name: "_allpaths_direction".to_string(), data_type: "string".to_string() },
                    ]));
                    return Ok(Some(DataChunk::new(rows, schema)));
                }
            }

            // Process new input chunk
            let chunk = input.next()?;
            if let Some(mut chunk) = chunk {
                for row in &chunk.rows {
                    all_paths.push(row.clone());
                }
                add_column_names(&mut chunk, &["_allpaths_edge_type", "_allpaths_direction"]);
                for row in chunk.rows.iter_mut() {
                    row.push(Value::String(edge_type.clone()));
                    row.push(Value::String(direction.clone()));
                }
                return Ok(Some(chunk));
            }
            Ok(None)
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

// ============ MultiShortestPath ============

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
        StreamingExecutor::MultiShortestPath {
            input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("MultiShortestPath not opened".to_string()));
            }

            let chunk = input.next()?;
            if let Some(chunk) = chunk {
                return Ok(Some(chunk));
            }
            Ok(None)
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

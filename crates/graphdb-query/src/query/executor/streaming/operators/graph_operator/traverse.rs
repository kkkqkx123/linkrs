use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::traversal::config::TraversalConfig;
use crate::storage::StorageClient;

use super::common;
use super::super::visited_set::VisitedSet;

pub(super) fn handle_traverse(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    min_depth: u32,
    max_depth: u32,
    visited: &mut VisitedSet,
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !base.lifecycle.is_opened() {
        return Err(QueryError::execution("Traverse not opened".to_string()));
    }

    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
    let chunk = input.advance()?;
    if let Some(chunk) = chunk {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            let tc = TraversalConfig::traverse(
                space_name.to_string(),
                direction,
                min_depth,
                max_depth,
                edge_types.to_vec(),
            );
            common::traverse_on_chunk(chunk, &*reader, &tc, visited, cancel_token)
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

pub(super) fn handle_traverse_all(
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    input.advance()
}

pub(super) fn handle_bi_expand(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
    space_name: &str,
    edge_types: &[String],
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
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
                let context = ValueRowContext::new(row.clone(), chunk.get_layout());
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

pub(super) fn handle_bi_traverse(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
    space_name: &str,
    edge_types: &[String],
    min_depth: u32,
    max_depth: u32,
    visited: &mut VisitedSet,
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
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
                let ctx = ValueRowContext::new(row.clone(), chunk.get_layout());
                let vid_val = ctx
                    .get_variable("vid")
                    .or_else(|| row.first().cloned())
                    .unwrap_or(Value::Null(crate::core::NullType::Null));
                if let Ok(vid) = VertexId::try_from(&vid_val) {
                    let mut frontier = vec![(vid, 0u32)];
                    let mut local_visited = VisitedSet::new();
                    local_visited.insert(vid);

                    while let Some((current, depth)) = frontier.pop() {
                        base.ensure_not_cancelled()?;
                        if depth >= max_depth {
                            continue;
                        }
                        if let Ok(edges) = reader.get_node_edges(space_name, &current, dir) {
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
                                if local_visited.contains(&nid)
                                    || !visited.insert(nid)
                                {
                                    continue;
                                }
                                local_visited.insert(nid);

                                if depth + 1 >= min_depth {
                                    if let Ok(Some(vertex)) =
                                        reader.get_vertex(space_name, &nid)
                                    {
                                        let mut out_row = row.clone();
                                        out_row.push(Value::Vertex(Box::new(vertex)));
                                        out_row.push(Value::String(edge_types.join("/")));
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

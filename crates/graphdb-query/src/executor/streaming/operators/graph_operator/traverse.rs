use std::sync::Arc;

use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::context::ValueRowContext;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::traversal::config::TraversalConfig;
use graphdb_core::error::QueryError;
use graphdb_core::types::storage_ids::VertexId;
use graphdb_core::{EdgeDirection, Value};

use super::super::visited_set::VisitedSet;
use super::common;
use super::{GraphOperator, GraphOperatorKind};

pub(super) fn handle_traverse(
    op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let GraphOperatorKind::Traverse {
        storage,
        space_name,
        edge_types,
        direction,
        min_depth,
        max_depth,
        visited,
        ..
    } = &mut op.kind
    else {
        unreachable!("traverse::handle_traverse called for a non-traverse graph source")
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let edge_types = &*edge_types;
    let direction = *direction;
    let min_depth = *min_depth;
    let max_depth = *max_depth;
    let visited = &mut *visited;

    let cancel_token = op.runtime.as_ref().map(|rt| rt.cancel_token());
    while let Some(chunk) = input.advance()? {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            let tc = TraversalConfig::traverse(
                space_name.to_string(),
                direction,
                min_depth,
                max_depth,
                edge_types.to_vec(),
            );
            if let Some(output) = common::traverse_on_chunk(
                chunk,
                Arc::clone(&op.output_layout),
                &*reader,
                &tc,
                visited,
                cancel_token.clone(),
            )? {
                return Ok(Some(output));
            }
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
            let _schema = Arc::new(Schema::new(new_cols));
            let mut rows = common::visible_rows(&chunk)
                .map(|(_, row)| row.clone())
                .collect::<Vec<_>>();
            for row in rows.iter_mut() {
                row.push(Value::string(edge_types.join("/")));
                row.push(Value::string(format!("{:?}", direction).to_lowercase()));
                row.push(Value::BigInt(1));
            }
            if !rows.is_empty() {
                return Ok(Some(DataChunk::new_with_layout(
                    rows,
                    Arc::clone(&op.output_layout),
                )));
            }
        }
    }
    Ok(None)
}

pub(super) fn handle_traverse_all(
    _op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    input.advance()
}

pub(super) fn handle_bi_expand(
    op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let GraphOperatorKind::BiExpand {
        storage,
        space_name,
        edge_types,
        ..
    } = &mut op.kind
    else {
        unreachable!("traverse::handle_bi_expand called for a non-bi-expand graph source")
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let edge_types = &*edge_types;
    while let Some(chunk) = input.advance()? {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            let dir = EdgeDirection::Both;
            let col_names = chunk.col_names();

            let mut out_rows = Vec::new();
            for (_, row) in common::visible_rows(&chunk) {
                if let Some(rt) = op.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                let context = ValueRowContext::new(row.clone(), chunk.get_layout());
                let vid_val = context
                    .get_variable("vid")
                    .or_else(|| row.first().cloned())
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                if let Ok(vid) = VertexId::try_from(&vid_val) {
                    if let Ok(edges) = reader.get_node_edges(space_name, &vid, dir) {
                        for e in &edges {
                            let edge_type_matches = edge_types.is_empty()
                                || edge_types.contains(&"both".to_string())
                                || edge_types.contains(&e.edge_type);
                            if !edge_type_matches {
                                continue;
                            }
                            let neighbor_id = if e.src() == &vid { *e.dst() } else { *e.src() };
                            if let Ok(Some(vertex)) = reader.get_vertex(space_name, &neighbor_id) {
                                let mut out_row = row.clone();
                                out_row.push(Value::Vertex(Box::new(vertex)));
                                out_row.push(Value::string(e.edge_type.clone()));
                                out_row.push(Value::string("both"));
                                out_rows.push(out_row);
                            }
                        }
                    }
                }
            }

            if out_rows.is_empty() {
                continue;
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
            let _schema = Arc::new(Schema::new(new_cols));
            return Ok(Some(DataChunk::new_with_layout(
                out_rows,
                Arc::clone(&op.output_layout),
            )));
        } else {
            if !chunk.is_empty() {
                return Ok(Some(chunk));
            }
        }
    }
    Ok(None)
}

pub(super) fn handle_bi_traverse(
    op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let GraphOperatorKind::BiTraverse {
        storage,
        space_name,
        edge_types,
        min_depth,
        max_depth,
        visited,
        ..
    } = &mut op.kind
    else {
        unreachable!("traverse::handle_bi_traverse called for a non-bi-traverse graph source")
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let edge_types = &*edge_types;
    let min_depth = *min_depth;
    let max_depth = *max_depth;
    let visited = &mut *visited;
    while let Some(chunk) = input.advance()? {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            let dir = EdgeDirection::Both;
            let col_names = chunk.col_names();

            let mut out_rows = Vec::new();
            for (_, row) in common::visible_rows(&chunk) {
                if let Some(rt) = op.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                let ctx = ValueRowContext::new(row.clone(), chunk.get_layout());
                let vid_val = ctx
                    .get_variable("vid")
                    .or_else(|| row.first().cloned())
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                if let Ok(vid) = VertexId::try_from(&vid_val) {
                    let mut frontier = vec![(vid, 0u32)];
                    let mut local_visited = VisitedSet::new();
                    local_visited.insert(vid);

                    while let Some((current, depth)) = frontier.pop() {
                        if let Some(rt) = op.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
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
                                if local_visited.contains(&nid) || !visited.insert(nid) {
                                    continue;
                                }
                                local_visited.insert(nid);

                                if depth + 1 >= min_depth {
                                    if let Ok(Some(vertex)) = reader.get_vertex(space_name, &nid) {
                                        let mut out_row = row.clone();
                                        out_row.push(Value::Vertex(Box::new(vertex)));
                                        out_row.push(Value::string(edge_types.join("/")));
                                        out_row.push(Value::string("both"));
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
                continue;
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
            let _schema = Arc::new(Schema::new(new_cols));
            return Ok(Some(DataChunk::new_with_layout(
                out_rows,
                Arc::clone(&op.output_layout),
            )));
        } else {
            if !chunk.is_empty() {
                return Ok(Some(chunk));
            }
        }
    }
    Ok(None)
}

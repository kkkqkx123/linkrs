use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::{Edge, EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;

use super::{GraphOperator, GraphOperatorKind};

pub(super) fn handle(
    op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let GraphOperatorKind::Subgraph {
        storage,
        space_name,
        steps,
        direction,
        edge_types,
    } = &mut op.kind
    else {
        unreachable!("subgraph::handle called for a non-subgraph graph source")
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let steps = *steps;
    let direction = *direction;
    let edge_types = &*edge_types;
    while let Some(mut chunk) = input.advance()? {
        chunk.materialize_selection_by("Subgraph");
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            let col_names = chunk.col_names();

            let mut out_rows = Vec::new();
            for row in &chunk.rows {
                if let Some(rt) = op.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                let ctx = ValueRowContext::new(row.clone(), chunk.get_layout());
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
                        if let Some(rt) = op.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        if current_step >= steps {
                            continue;
                        }
                        if let Ok(edges) = reader.get_node_edges(space_name, &current, direction) {
                            let et_set: HashSet<String> = edge_types.iter().cloned().collect();
                            for e in &edges {
                                if !edge_types.is_empty() && !et_set.contains(&e.edge_type) {
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
                                if visited.insert(neighbor_id) && current_step + 1 < steps {
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
                        out_row.push(Value::string(edge.edge_type.clone()));
                        out_rows.push(out_row);
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

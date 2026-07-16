use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::storage::QueryStorage;

use super::super::algorithms::{bidir_bfs_shortest_path, path_endpoint_pairs, BidirBfsConfig};

pub(super) fn handle_shortest_path(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    target_vertex: &Option<Expression>,
    edge_types: &[String],
    direction: EdgeDirection,
    max_depth: usize,
    start_vertices: &[Value],
    target_vertices: &[Value],
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
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
                    let et_ref: Option<&[String]> =
                        if edge_types.is_empty() || edge_types.contains(&"both".to_string()) {
                            None
                        } else {
                            Some(edge_types)
                        };

                    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                    let paths = bidir_bfs_shortest_path(
                        &*reader,
                        &src_vid,
                        &dst_vid,
                        BidirBfsConfig {
                            space_name,
                            edge_type_filter: et_ref,
                            max_depth,
                            single_shortest: true,
                            limit: 1,
                            direction,
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

pub(super) fn handle_bfs_shortest(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    max_depth: usize,
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
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
                let ctx = ValueRowContext::new(row.clone(), chunk.get_layout());
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
                    let et_ref: Option<&[String]> =
                        if edge_types.is_empty() || edge_types.contains(&"both".to_string()) {
                            None
                        } else {
                            Some(edge_types)
                        };

                    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                    let paths = bidir_bfs_shortest_path(
                        &*reader,
                        &src_vid,
                        &dst_vid,
                        BidirBfsConfig {
                            space_name,
                            edge_type_filter: et_ref,
                            max_depth,
                            single_shortest: true,
                            limit: 1,
                            direction,
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

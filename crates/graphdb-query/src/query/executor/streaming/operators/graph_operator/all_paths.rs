use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::storage::QueryStorage;

use super::super::algorithms::{
    bidir_bfs_shortest_path, enumerate_all_paths, path_endpoint_pairs, AllPathsConfig,
    BidirBfsConfig,
};
use super::common;

pub(super) fn handle_all_paths(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    target_vertex: &Option<Expression>,
    edge_types: &[String],
    direction: EdgeDirection,
    min_depth: usize,
    max_depth: usize,
    acyclic: bool,
    limit: &Option<usize>,
    offset: usize,
    filter: &Option<Expression>,
    start_vertices: &[Value],
    target_vertices: &[Value],
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
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
                    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                    let result_cap = limit.unwrap_or(usize::MAX).saturating_add(offset);
                    let paths = enumerate_all_paths(
                        &*reader,
                        &src_vid,
                        &dst_vid,
                        AllPathsConfig {
                            space_name,
                            edge_types,
                            direction,
                            min_depth,
                            max_depth,
                            acyclic,
                            result_cap,
                        },
                        cancel_token.as_deref(),
                    )?;

                    for path in paths
                        .into_iter()
                        .skip(offset)
                        .take(limit.unwrap_or(usize::MAX))
                    {
                        base.ensure_not_cancelled()?;
                        let mut out_row = row.clone();
                        out_row.push(Value::Path(Box::new(path)));
                        if common::row_passes_filter(&out_row, &col_names, filter) {
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

pub(super) fn handle_multi_shortest_path(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    target_vertices: &[Expression],
    edge_types: &[String],
    max_depth: usize,
    left_vertex_column: &str,
    right_vertex_column: &str,
    single_shortest: bool,
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
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
                let ctx = ValueRowContext::new(row.clone(), chunk.get_layout());
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
                    let mut expression_context =
                        ValueRowContext::new(row.clone(), chunk.get_layout());
                    dst_values.push(
                        ExpressionEvaluator::evaluate(expression, &mut expression_context)
                            .map_err(|error| {
                                QueryError::execution(format!(
                                    "MultiShortestPath target evaluation failed: {error}"
                                ))
                            })?,
                    );
                }
                if let Ok(src_vid) = VertexId::try_from(&src_val) {
                    let et_ref: Option<&[String]> =
                        if edge_types.is_empty() || edge_types.contains(&"both".to_string()) {
                            None
                        } else {
                            Some(edge_types)
                        };

                    for dst_value in dst_values {
                        let Ok(dst_vid) = VertexId::try_from(&dst_value) else {
                            continue;
                        };
                        base.ensure_not_cancelled()?;
                        let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                        let paths = bidir_bfs_shortest_path(
                            &*reader,
                            &src_vid,
                            &dst_vid,
                            BidirBfsConfig {
                                space_name,
                                edge_type_filter: et_ref,
                                max_depth,
                                single_shortest,
                                limit: if single_shortest { 1 } else { 10 },
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

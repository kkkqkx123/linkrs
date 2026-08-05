use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::storage::QueryStorage;

use super::common;
use super::{ExpandCtx, GraphCtx};

pub(super) fn handle(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    filter_expr: &Option<Expression>,
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !base.lifecycle.is_opened() {
        return Err(QueryError::execution("Expand not opened".to_string()));
    }

    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
    while let Some(mut chunk) = input.advance()? {
        chunk.materialize_selection();
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            if let Some(output) = common::expand_on_chunk(
                chunk,
                Arc::clone(&base.output_layout),
                &*reader,
                Vec::new(),
                1,
                &mut ExpandCtx {
                    space_name,
                    edge_types,
                    direction,
                    filter_expr,
                    col_names_template: Vec::new(),
                    cancel_token: cancel_token.clone(),
                },
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
                name: "_expand_edge".to_string(),
                data_type: "edge".to_string(),
            });
            new_cols.push(ColumnInfo {
                name: "_expand_dst".to_string(),
                data_type: "vertex".to_string(),
            });
            let schema = Arc::new(Schema::new(new_cols));
            let mut rows = chunk.rows;
            for row in rows.iter_mut() {
                row.push(Value::Null(crate::core::NullType::Null));
                row.push(Value::Vertex(Box::default()));
            }
            let out_col_names = schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>();
            rows.retain(|row| common::row_passes_filter(row, &out_col_names, filter_expr));
            if !rows.is_empty() {
                return Ok(Some(DataChunk::new_with_layout(
                    rows,
                    Arc::clone(&base.output_layout),
                )));
            }
        }
    }
    Ok(None)
}

pub(super) fn handle_all(
    filter_expr: &Option<Expression>,
    col_names: Vec<String>,
    src_vids: Vec<Value>,
    step_limit: u32,
    count_only: bool,
    emit_raw_ids: bool,
    ctx: &mut GraphCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let storage = ctx.storage;
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let base = &mut *ctx.base;
    let input = &mut *ctx.input;
    if !base.lifecycle.is_opened() {
        return Err(QueryError::execution("ExpandAll not opened".to_string()));
    }

    let use_fast_path = step_limit == 1
        && filter_expr.is_none()
        && src_vids.is_empty()
        && !ctx.is_recursive;

    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
    while let Some(mut chunk) = input.advance()? {
        chunk.materialize_selection();
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();

            if count_only && use_fast_path {
                let count = common::expand_count_only(
                    chunk,
                    &*reader,
                    src_vids.clone(),
                    &mut ExpandCtx {
                        space_name,
                        edge_types,
                        direction,
                        filter_expr,
                        col_names_template: col_names.clone(),
                        cancel_token: cancel_token.clone(),
                    },
                )?;
                if count > 0 {
                    let mut out_row = Vec::with_capacity(1);
                    out_row.push(Value::BigInt(count));
                    return Ok(Some(DataChunk::new_with_layout(
                        vec![out_row],
                        Arc::clone(&base.output_layout),
                    )));
                }
                continue;
            }

            let expand_result = if use_fast_path {
                common::expand_single_step(
                    chunk,
                    Arc::clone(&base.output_layout),
                    &*reader,
                    src_vids.clone(),
                    emit_raw_ids,
                    &mut ExpandCtx {
                        space_name,
                        edge_types,
                        direction,
                        filter_expr,
                        col_names_template: col_names.clone(),
                        cancel_token: cancel_token.clone(),
                    },
                )?
            } else {
                common::expand_on_chunk(
                    chunk,
                    Arc::clone(&base.output_layout),
                    &*reader,
                    src_vids.clone(),
                    step_limit,
                    &mut ExpandCtx {
                        space_name,
                        edge_types,
                        direction,
                        filter_expr,
                        col_names_template: col_names.clone(),
                        cancel_token: cancel_token.clone(),
                    },
                )?
            };
            if let Some(output) = expand_result {
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
                name: "_expand_edge".to_string(),
                data_type: "edge".to_string(),
            });
            new_cols.push(ColumnInfo {
                name: "_expand_dst".to_string(),
                data_type: "vertex".to_string(),
            });
            let _schema = Arc::new(Schema::new(new_cols));
            let mut rows = chunk.rows;
            for row in rows.iter_mut() {
                row.push(Value::Null(crate::core::NullType::Null));
                row.push(Value::Vertex(Box::default()));
            }
            if !rows.is_empty() {
                return Ok(Some(DataChunk::new_with_layout(
                    rows,
                    Arc::clone(&base.output_layout),
                )));
            }
        }
    }
    Ok(None)
}

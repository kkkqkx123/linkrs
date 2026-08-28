use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::executor::StreamingExecutor;
use graphdb_core::error::QueryError;
use graphdb_core::Value;

use super::common;
use super::{ExpandCtx, GraphOperator, GraphOperatorKind};

pub(super) fn handle(
    op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let GraphOperatorKind::Expand {
        storage,
        space_name,
        edge_types,
        direction,
        filter_expr,
    } = &mut op.kind
    else {
        unreachable!("expand::handle called for a non-expand graph source")
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let edge_types = &*edge_types;
    let direction = *direction;
    let filter_expr = &*filter_expr;
    let cancel_token = op.runtime.as_ref().map(|rt| rt.cancel_token());
    while let Some(chunk) = input.advance()? {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            if let Some(output) = common::expand_on_chunk(
                chunk,
                Arc::clone(&op.output_layout),
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
            let mut rows = common::visible_rows(&chunk)
                .map(|(_, row)| row.clone())
                .collect::<Vec<_>>();
            for row in rows.iter_mut() {
                row.push(Value::Null(graphdb_core::NullType::Null));
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
                    Arc::clone(&op.output_layout),
                )));
            }
        }
    }
    Ok(None)
}

pub(super) fn handle_all(
    op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let GraphOperatorKind::ExpandAll {
        storage,
        space_name,
        edge_types,
        direction,
        filter_expr,
        col_names,
        src_vids,
        step_limit,
        count_only,
        emit_raw_ids,
        lightweight_source,
    } = &mut op.kind
    else {
        unreachable!("expand::handle_all called for a non-expand-all graph source")
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let edge_types = &*edge_types;
    let direction = *direction;
    let filter_expr = &*filter_expr;
    let col_names = col_names.clone();
    let src_vids = src_vids.clone();
    let step_limit = *step_limit;
    let count_only = *count_only;
    let emit_raw_ids = *emit_raw_ids;
    let lightweight_source = *lightweight_source;

    let use_fast_path =
        step_limit == 1 && filter_expr.is_none() && src_vids.is_empty() && !emit_raw_ids;

    let cancel_token = op.runtime.as_ref().map(|rt| rt.cancel_token());
    while let Some(chunk) = input.advance()? {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();

            if count_only {
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
                    let out_row = vec![Value::BigInt(count)];
                    return Ok(Some(DataChunk::new_with_layout(
                        vec![out_row],
                        Arc::clone(&op.output_layout),
                    )));
                }
                continue;
            }

            let expand_result = if use_fast_path {
                common::expand_single_step(
                    chunk,
                    Arc::clone(&op.output_layout),
                    &*reader,
                    src_vids.clone(),
                    emit_raw_ids,
                    lightweight_source,
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
                    Arc::clone(&op.output_layout),
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
            let mut rows = common::visible_rows(&chunk)
                .map(|(_, row)| row.clone())
                .collect::<Vec<_>>();
            for row in rows.iter_mut() {
                row.push(Value::Null(graphdb_core::NullType::Null));
                row.push(Value::Vertex(Box::default()));
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

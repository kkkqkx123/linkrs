use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::executor::StreamingExecutor;
use linkrs_core::error::QueryError;
use linkrs_core::Value;

use super::common;
use super::{ExpandCtx, GraphOperator, GraphOperatorKind};

pub(super) fn handle(
    op: &mut GraphOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let GraphOperatorKind::Expand {
        storage,
        space_name,
        dst_tag,
        edge_types,
        direction,
        filter_expr,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "expand::handle called for a non-expand graph source".to_string(),
        ));
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let dst_tag = &*dst_tag;
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
                    dst_tag,
                    edge_types,
                    direction,
                    filter_expr,
                    col_names_template: Vec::new(),
                    cancel_token: cancel_token.clone(),
                    path_semantic: None,
                    edge_required_props: None,
                    dst_required_props: None,
                    closed_loop: false,
                    skip_rows: false,
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
                row.push(Value::Null(linkrs_core::NullType::Null));
                row.push(Value::Null(linkrs_core::NullType::Null));
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
        dst_tag,
        edge_types,
        direction,
        filter_expr,
        col_names,
        src_vids,
        step_limit,
        step_limits,
        count_only,
        emit_raw_ids,
        lightweight_source,
        path_semantic,
        edge_required_props,
        dst_required_props,
        closed_loop,
        skip_rows,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "expand::handle_all called for a non-expand-all graph source".to_string(),
        ));
    };
    let storage = &*storage;
    let space_name = &*space_name;
    let dst_tag = &*dst_tag;
    let edge_types = &*edge_types;
    let direction = *direction;
    let filter_expr = &*filter_expr;
    let col_names = col_names.clone();
    let src_vids = src_vids.clone();
    let step_limit = *step_limit;
    let step_limits = step_limits.clone();
    let count_only = *count_only;
    let emit_raw_ids = *emit_raw_ids;
    let lightweight_source = *lightweight_source;
    let path_semantic = path_semantic.clone();
    let edge_required_props = edge_required_props.clone();
    let dst_required_props = dst_required_props.clone();
    let closed_loop = *closed_loop;
    let mut skip_rows = *skip_rows;
    if skip_rows
        && !crate::executor::streaming::chunk::use_columnar_path(&op.runtime)
    {
        skip_rows = false;
    }

    // The raw-id fast path (`emit_raw_ids`) is handled inside
    // `expand_single_step`; it is not a reason to fall back to the generic
    // runtime path. Only a real filter, literal seed ids or a path semantic
    // require the generic walk.
    let use_fast_path =
        step_limit == 1 && step_limits.is_none() && filter_expr.is_none() && src_vids.is_empty() && path_semantic.is_none();

    let cancel_token = op.runtime.as_ref().map(|rt| rt.cancel_token());
    while let Some(chunk) = input.advance()? {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();

            // The degree-batch count path ignores per-path repeat rules,
            // so it only applies when no semantic constrains the walk.
            if count_only && path_semantic.is_none() {
                let count = common::expand_count_only(
                    chunk,
                    &*reader,
                    src_vids.clone(),
                    &mut ExpandCtx {
                        space_name,
                        dst_tag,
                        edge_types,
                        direction,
                        filter_expr,
                        col_names_template: col_names.clone(),
                        cancel_token: cancel_token.clone(),
                        path_semantic: path_semantic.clone(),
                        edge_required_props: edge_required_props.clone(),
                        dst_required_props: dst_required_props.clone(),
                        closed_loop,
                        skip_rows,
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

            let mut ctx = ExpandCtx {
                space_name,
                dst_tag,
                edge_types,
                direction,
                filter_expr,
                col_names_template: col_names.clone(),
                cancel_token: cancel_token.clone(),
                path_semantic: path_semantic.clone(),
                edge_required_props: edge_required_props.clone(),
                dst_required_props: dst_required_props.clone(),
                closed_loop,
                skip_rows,
            };
            let expand_result = if let Some(limits) = step_limits.clone() {
                let var_eligible = !limits.is_empty()
                    && src_vids.is_empty()
                    && filter_expr.is_none()
                    && matches!(
                        path_semantic,
                        None | Some(crate::parser::ast::pattern::PathSemantic::Walk)
                    )
                    && closed_loop
                    && common::is_closed_loop_storage(
                        &*reader,
                        space_name,
                        edge_types,
                        direction,
                        dst_tag,
                    );
                if var_eligible {
                    common::expand_variable_frontier(
                        chunk,
                        Arc::clone(&op.output_layout),
                        &*reader,
                        src_vids.clone(),
                        &limits,
                        &mut ctx,
                    )?
                } else {
                    common::expand_variable_row_union(
                        chunk,
                        Arc::clone(&op.output_layout),
                        &*reader,
                        src_vids.clone(),
                        &limits,
                        &mut ctx,
                    )?
                }
            } else if !use_fast_path && step_limit > 1 {
                let multi_eligible = src_vids.is_empty()
                    && filter_expr.is_none()
                    && matches!(
                        path_semantic,
                        None | Some(crate::parser::ast::pattern::PathSemantic::Walk)
                    )
                    && closed_loop
                    && common::is_closed_loop_storage(
                        &*reader,
                        space_name,
                        edge_types,
                        direction,
                        dst_tag,
                    );
                if multi_eligible {
                    common::expand_multi_hop_frontier(
                        chunk,
                        Arc::clone(&op.output_layout),
                        &*reader,
                        src_vids.clone(),
                        step_limit,
                        &mut ctx,
                    )?
                } else {
                    common::expand_on_chunk(
                        chunk,
                        Arc::clone(&op.output_layout),
                        &*reader,
                        src_vids.clone(),
                        step_limit,
                        &mut ctx,
                    )?
                }
            } else if use_fast_path {
                if !emit_raw_ids
                    && closed_loop
                    && common::is_closed_loop_storage(
                        &*reader,
                        space_name,
                        edge_types,
                        direction,
                        dst_tag,
                    ) {
                    match common::expand_single_step_columnar(
                        chunk,
                        Arc::clone(&op.output_layout),
                        &*reader,
                        src_vids.clone(),
                        &mut ctx,
                    )? {
                        Some(out) => Some(out),
                        None => None,
                    }
                } else {
                    common::expand_single_step(
                        chunk,
                        Arc::clone(&op.output_layout),
                        &*reader,
                        src_vids.clone(),
                        emit_raw_ids,
                        lightweight_source,
                        &mut ctx,
                    )?
                }
            } else {
                common::expand_on_chunk(
                    chunk,
                    Arc::clone(&op.output_layout),
                    &*reader,
                    src_vids.clone(),
                    step_limit,
                    &mut ctx,
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
                row.push(Value::Null(linkrs_core::NullType::Null));
                row.push(Value::Null(linkrs_core::NullType::Null));
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

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::SplitRowContext;
use crate::query::executor::streaming::operators::base::OperatorLifecycle;
use crate::query::executor::streaming::slot::SlotLayout;

use super::{build_combined_names, close_common, evaluate_join_key, JoinCtx, JoinKeyValue};

pub(super) fn next_hash_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side_hash: &mut HashMap<JoinKeyValue, Vec<Vec<Value>>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    ctx: &mut JoinCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let memory_tracker = &mut *ctx.memory_tracker;
    let right_col_names = &mut *ctx.right_col_names;
    let base = &mut *ctx.base;
    let left = &mut *ctx.left;
    let right = &mut *ctx.right;
    if !*left_consumed {
        while let Some(mut chunk) = right.advance()? {
            base.ensure_not_cancelled()?;
            let col_names = chunk.col_names();
            if right_col_names.is_empty() {
                *right_col_names = col_names.clone();
            }
            chunk.materialize_columns();
            let cols = chunk.columns.as_deref();
            for (row_idx, row) in chunk.rows.iter().enumerate() {
                memory_tracker.try_reserve_row(row)?;
                let hash_key = evaluate_join_key(
                    row,
                    &col_names,
                    hash_keys,
                    cols.map(|c| (c, row_idx)),
                )?;
                build_side_hash
                    .entry(hash_key)
                    .or_default()
                    .push(row.clone());
                all_right_rows.push(row.clone());
            }
        }
        *left_consumed = true;
    }

    while let Some(mut left_chunk) = left.advance()? {
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        let combined_layout = if join_condition.is_some() {
            let fallback_width = right_col_names.len();
            let names = build_combined_names(&left_col_names, right_col_names, fallback_width);
            Some(Arc::new(SlotLayout::from_names(&names)))
        } else {
            None
        };
        left_chunk.materialize_columns();
        let left_cols = left_chunk.columns.as_deref();
        for (row_idx, left_row) in left_chunk.rows.iter().enumerate() {
            let probe_key = evaluate_join_key(
                left_row,
                &left_col_names,
                probe_keys,
                left_cols.map(|c| (c, row_idx)),
            )?;
            let matching_right_rows = build_side_hash.get(&probe_key);

            if let Some(right_rows) = matching_right_rows {
                if let Some((condition, layout)) =
                    join_condition.as_ref().zip(combined_layout.as_ref())
                {
                    for right_row in right_rows {
                        let mut ctx = SplitRowContext::new(left_row, right_row, Arc::clone(layout));
                        if matches!(
                            ExpressionEvaluator::evaluate(condition, &mut ctx),
                            Ok(Value::Bool(b)) if b
                        ) {
                            let mut combined =
                                Vec::with_capacity(left_row.len() + right_row.len());
                            combined.extend_from_slice(left_row);
                            combined.extend_from_slice(right_row);
                            result_rows.push(combined);
                        }
                    }
                } else {
                    for right_row in right_rows {
                        let mut joined_row = left_row.clone();
                        joined_row.extend(right_row.clone());
                        result_rows.push(joined_row);
                    }
                }
            }
        }

        if !result_rows.is_empty() {
            return Ok(Some(DataChunk::new_with_layout(
                result_rows,
                Arc::clone(&base.output_layout),
            )));
        }
    }

    Ok(None)
}

pub(super) fn next_hash_left_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side_hash: &mut HashMap<JoinKeyValue, Vec<Vec<Value>>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    ctx: &mut JoinCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let memory_tracker = &mut *ctx.memory_tracker;
    let right_col_names = &mut *ctx.right_col_names;
    let base = &mut *ctx.base;
    let left = &mut *ctx.left;
    let right = &mut *ctx.right;
    if !*left_consumed {
        while let Some(mut chunk) = right.advance()? {
            base.ensure_not_cancelled()?;
            let col_names = chunk.col_names();
            if right_col_names.is_empty() {
                *right_col_names = col_names.clone();
            }
            chunk.materialize_columns();
            let cols = chunk.columns.as_deref();
            for (row_idx, row) in chunk.rows.iter().enumerate() {
                memory_tracker.try_reserve_row(row)?;
                let hash_key = evaluate_join_key(
                    row,
                    &col_names,
                    hash_keys,
                    cols.map(|c| (c, row_idx)),
                )?;
                build_side_hash
                    .entry(hash_key)
                    .or_default()
                    .push(row.clone());
                all_right_rows.push(row.clone());
            }
        }
        *left_consumed = true;
    }

    while let Some(mut left_chunk) = left.advance()? {
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        let combined_layout = if join_condition.is_some() {
            let fallback_width = right_col_names.len();
            let names = build_combined_names(&left_col_names, right_col_names, fallback_width);
            Some(Arc::new(SlotLayout::from_names(&names)))
        } else {
            None
        };
        left_chunk.materialize_columns();
        let left_cols = left_chunk.columns.as_deref();
        for (row_idx, left_row) in left_chunk.rows.iter().enumerate() {
            let probe_key = evaluate_join_key(
                left_row,
                &left_col_names,
                probe_keys,
                left_cols.map(|c| (c, row_idx)),
            )?;
            let matching_right_rows = build_side_hash.get(&probe_key);

            if let Some(right_rows) = matching_right_rows {
                if let Some((condition, layout)) =
                    join_condition.as_ref().zip(combined_layout.as_ref())
                {
                    for right_row in right_rows {
                        let mut ctx = SplitRowContext::new(left_row, right_row, Arc::clone(layout));
                        let satisfied = match ExpressionEvaluator::evaluate(condition, &mut ctx)
                        {
                            Ok(Value::Bool(b)) => b,
                            Ok(Value::Null(_)) => false,
                            Ok(_) => true,
                            Err(e) => {
                                return Err(QueryError::execution(format!(
                                    "HashLeftJoin condition evaluation failed: {}",
                                    e
                                )));
                            }
                        };
                        if satisfied {
                            let mut combined =
                                Vec::with_capacity(left_row.len() + right_row.len());
                            combined.extend_from_slice(left_row);
                            combined.extend_from_slice(right_row);
                            result_rows.push(combined);
                        }
                    }
                } else {
                    for right_row in right_rows {
                        let mut joined_row = left_row.clone();
                        joined_row.extend(right_row.clone());
                        result_rows.push(joined_row);
                    }
                }
            } else {
                let mut unmatched_row = left_row.clone();
                let right_width = base
                    .output_layout
                    .len()
                    .checked_sub(left_row.len())
                    .ok_or_else(|| {
                        QueryError::execution(
                            "HashLeftJoin planned output layout is narrower than its left input"
                                .to_string(),
                        )
                    })?;
                for _ in 0..right_width {
                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                }
                result_rows.push(unmatched_row);
            }
        }

        if !result_rows.is_empty() {
            return Ok(Some(DataChunk::new_with_layout(
                result_rows,
                Arc::clone(&base.output_layout),
            )));
        }
    }

    Ok(None)
}

pub(super) fn close(
    lifecycle: &mut OperatorLifecycle,
    memory_tracker: &mut MemoryTracker,
    build_side_hash: &mut HashMap<JoinKeyValue, Vec<Vec<Value>>>,
    all_right_rows: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    close_common(lifecycle, memory_tracker, || {
        build_side_hash.clear();
        all_right_rows.clear();
    })
}

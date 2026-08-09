use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::FullOuterJoinPhase;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operators::base::OperatorLifecycle;

use super::{build_combined_names, close_common, JoinCtx};

pub(super) fn next_inner_join(
    join_condition: &mut Option<Expression>,
    build_side_tuples: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    ctx: &mut JoinCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let memory_tracker = &mut *ctx.memory_tracker;
    let right_col_names = &mut *ctx.right_col_names;
    let base = &mut *ctx.base;
    let left = &mut *ctx.left;
    let right = &mut *ctx.right;
    if !*left_consumed {
        let mut captured_right_names = Vec::new();
        while let Some(mut chunk) = right.advance()? {
            chunk.materialize_selection_by("MergeJoin");
            base.ensure_not_cancelled()?;
            if captured_right_names.is_empty() {
                captured_right_names = chunk.col_names();
            }
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                build_side_tuples.push(row);
            }
        }
        *right_col_names = captured_right_names;
        *left_consumed = true;
    }

    while let Some(mut left_chunk) = left.advance()? {
        left_chunk.materialize_selection_by("MergeJoin");
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        for left_row in &left_chunk.rows {
            for right_row in build_side_tuples.iter() {
                let condition_satisfied = if let Some(condition) = join_condition {
                    let mut combined_row = left_row.clone();
                    combined_row.extend(right_row.clone());
                    let combined_names =
                        build_combined_names(&left_col_names, right_col_names, right_row.len());
                    let mut context = ValueRowContext::from_names(combined_row, combined_names);
                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                        Ok(Value::Bool(b)) => b,
                        _ => false,
                    }
                } else {
                    true
                };

                if condition_satisfied {
                    let mut joined_row = left_row.clone();
                    joined_row.extend(right_row.clone());
                    result_rows.push(joined_row);
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

pub(super) fn next_left_join(
    join_condition: &mut Option<Expression>,
    build_side_tuples: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    ctx: &mut JoinCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let memory_tracker = &mut *ctx.memory_tracker;
    let right_col_names = &mut *ctx.right_col_names;
    let base = &mut *ctx.base;
    let left = &mut *ctx.left;
    let right = &mut *ctx.right;
    if !*left_consumed {
        let mut captured_right_names = Vec::new();
        while let Some(mut chunk) = right.advance()? {
            chunk.materialize_selection_by("MergeJoin");
            base.ensure_not_cancelled()?;
            if captured_right_names.is_empty() {
                captured_right_names = chunk.col_names();
            }
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                build_side_tuples.push(row);
            }
        }
        *right_col_names = captured_right_names;
        *left_consumed = true;
    }

    while let Some(mut left_chunk) = left.advance()? {
        left_chunk.materialize_selection_by("MergeJoin");
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        for left_row in &left_chunk.rows {
            let mut matched = false;
            for right_row in build_side_tuples.iter() {
                let condition_satisfied = if let Some(condition) = join_condition {
                    let mut combined_row = left_row.clone();
                    combined_row.extend(right_row.clone());
                    let combined_names =
                        build_combined_names(&left_col_names, right_col_names, right_row.len());
                    let mut context = ValueRowContext::from_names(combined_row, combined_names);
                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                        Ok(Value::Bool(b)) => b,
                        _ => false,
                    }
                } else {
                    true
                };

                if condition_satisfied {
                    matched = true;
                    let mut joined_row = left_row.clone();
                    joined_row.extend(right_row.clone());
                    result_rows.push(joined_row);
                }
            }

            if !matched {
                let mut unmatched_row = left_row.clone();
                let right_width = base
                    .output_layout
                    .len()
                    .checked_sub(left_row.len())
                    .ok_or_else(|| {
                        QueryError::execution(
                            "LeftJoin planned output layout is narrower than its left input"
                                .to_string(),
                        )
                    })?;
                for _ in 0..right_width {
                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                }
                result_rows.push(unmatched_row);
            }
        }

        if result_rows.is_empty() {
            continue;
        }
        return Ok(Some(DataChunk::new_with_layout(
            result_rows,
            Arc::clone(&base.output_layout),
        )));
    }
    Ok(None)
}

pub(super) fn next_right_join(
    join_condition: &mut Option<Expression>,
    build_side_tuples: &mut Vec<Vec<Value>>,
    right_consumed: &mut bool,
    ctx: &mut JoinCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let memory_tracker = &mut *ctx.memory_tracker;
    let right_col_names = &mut *ctx.right_col_names;
    let base = &mut *ctx.base;
    let left = &mut *ctx.left;
    let right = &mut *ctx.right;
    if !*right_consumed {
        let mut captured_left_names = Vec::new();
        while let Some(mut chunk) = left.advance()? {
            chunk.materialize_selection_by("MergeJoin");
            base.ensure_not_cancelled()?;
            if captured_left_names.is_empty() {
                captured_left_names = chunk.col_names();
            }
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                build_side_tuples.push(row);
            }
        }
        *right_col_names = captured_left_names;
        *right_consumed = true;
    }

    while let Some(mut right_chunk) = right.advance()? {
        right_chunk.materialize_selection_by("MergeJoin");
        let right_cols = right_chunk.col_names();
        let mut result_rows = Vec::new();

        for right_row in &right_chunk.rows {
            let mut matched = false;
            for left_row in build_side_tuples.iter() {
                let condition_satisfied = if let Some(condition) = join_condition {
                    let mut combined_row = left_row.clone();
                    combined_row.extend(right_row.clone());
                    let combined_names =
                        build_combined_names(right_col_names, &right_cols, left_row.len());
                    let mut context = ValueRowContext::from_names(combined_row, combined_names);
                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                        Ok(Value::Bool(b)) => b,
                        _ => false,
                    }
                } else {
                    true
                };

                if condition_satisfied {
                    matched = true;
                    let mut joined_row = left_row.clone();
                    joined_row.extend(right_row.clone());
                    result_rows.push(joined_row);
                }
            }

            if !matched {
                let mut unmatched_row = Vec::new();
                let left_width = base
                    .output_layout
                    .len()
                    .checked_sub(right_row.len())
                    .ok_or_else(|| {
                        QueryError::execution(
                            "RightJoin planned output layout is narrower than its right input"
                                .to_string(),
                        )
                    })?;
                for _ in 0..left_width {
                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                }
                unmatched_row.extend(right_row.clone());
                result_rows.push(unmatched_row);
            }
        }

        if result_rows.is_empty() {
            continue;
        }
        return Ok(Some(DataChunk::new_with_layout(
            result_rows,
            Arc::clone(&base.output_layout),
        )));
    }
    Ok(None)
}

pub(super) fn next_full_outer_join(
    join_condition: &mut Option<Expression>,
    left_rows: &mut Vec<Vec<Value>>,
    right_rows: &mut Vec<Vec<Value>>,
    matched_right_indices: &mut HashSet<usize>,
    result_iter: &mut Option<std::vec::IntoIter<Vec<Value>>>,
    phase: &mut FullOuterJoinPhase,
    ctx: &mut JoinCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let memory_tracker = &mut *ctx.memory_tracker;
    let right_col_names = &mut *ctx.right_col_names;
    let base = &mut *ctx.base;
    let left = &mut *ctx.left;
    let right = &mut *ctx.right;
    loop {
        match phase {
            FullOuterJoinPhase::BuildingRight => {
                let mut captured_right_names = Vec::new();
                while let Some(mut chunk) = left.advance()? {
                    chunk.materialize_selection_by("MergeJoin");
                    base.ensure_not_cancelled()?;
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    left_rows.extend(chunk.rows);
                }
                while let Some(mut chunk) = right.advance()? {
                    chunk.materialize_selection_by("MergeJoin");
                    base.ensure_not_cancelled()?;
                    if captured_right_names.is_empty() {
                        captured_right_names = chunk.col_names();
                    }
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    right_rows.extend(chunk.rows);
                }
                *right_col_names = captured_right_names;
                *phase = FullOuterJoinPhase::ProbeLeft;
            }

            FullOuterJoinPhase::ProbeLeft => {
                let mut all_results = Vec::new();

                for left_row in left_rows.iter() {
                    base.ensure_not_cancelled()?;
                    let mut matched = false;
                    for (right_idx, right_row) in right_rows.iter().enumerate() {
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let left_col_names: Vec<String> =
                                (0..left_row.len()).map(|i| format!("col_{}", i)).collect();
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());
                            let combined_names = build_combined_names(
                                &left_col_names,
                                right_col_names,
                                right_row.len(),
                            );
                            let mut context =
                                ValueRowContext::from_names(combined_row, combined_names);
                            match ExpressionEvaluator::evaluate(condition, &mut context) {
                                Ok(Value::Bool(b)) => b,
                                _ => false,
                            }
                        } else {
                            true
                        };

                        if condition_satisfied {
                            matched = true;
                            matched_right_indices.insert(right_idx);
                            let mut joined_row = left_row.clone();
                            joined_row.extend(right_row.clone());
                            all_results.push(joined_row);
                        }
                    }

                    if !matched {
                        let mut unmatched_row = left_row.clone();
                        let right_width = base.output_layout.len().checked_sub(left_row.len()).ok_or_else(|| {
                            QueryError::execution(
                                "FullOuterJoin planned output layout is narrower than its left input"
                                    .to_string(),
                            )
                        })?;
                        for _ in 0..right_width {
                            unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                        }
                        all_results.push(unmatched_row);
                    }
                }

                *phase = FullOuterJoinPhase::EmitUnmatchedRight;
                if !all_results.is_empty() {
                    let rows: Vec<Vec<Value>> = all_results.into_iter().collect();
                    if !rows.is_empty() {
                        *result_iter = Some(rows.into_iter());
                        return Ok(Some(DataChunk::new_with_layout(
                            result_iter.as_mut().unwrap().collect::<Vec<_>>(),
                            Arc::clone(&base.output_layout),
                        )));
                    }
                }
            }

            FullOuterJoinPhase::EmitUnmatchedRight => {
                if let Some(iter) = result_iter {
                    let rows: Vec<Vec<Value>> = iter.collect();
                    if !rows.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            rows,
                            Arc::clone(&base.output_layout),
                        )));
                    }
                    *result_iter = None;
                }

                let mut unmatched = Vec::new();
                for (right_idx, right_row) in right_rows.iter().enumerate() {
                    if !matched_right_indices.contains(&right_idx) {
                        let mut row = Vec::new();
                        let left_width = base.output_layout.len().checked_sub(right_row.len()).ok_or_else(|| {
                            QueryError::execution(
                                "FullOuterJoin planned output layout is narrower than its right input"
                                    .to_string(),
                            )
                        })?;
                        for _ in 0..left_width {
                            row.push(Value::Null(crate::core::value::NullType::Null));
                        }
                        row.extend(right_row.clone());
                        unmatched.push(row);
                    }
                }

                if unmatched.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(DataChunk::new_with_layout(
                    unmatched,
                    Arc::clone(&base.output_layout),
                )));
            }
        }
    }
}

pub(super) fn close_full_outer(
    lifecycle: &mut OperatorLifecycle,
    memory_tracker: &mut MemoryTracker,
    left_rows: &mut Vec<Vec<Value>>,
    right_rows: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    close_common(lifecycle, memory_tracker, || {
        left_rows.clear();
        right_rows.clear();
    })
}

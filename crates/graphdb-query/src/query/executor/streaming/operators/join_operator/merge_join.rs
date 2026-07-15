use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::FullOuterJoinPhase;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::base::OperatorLifecycle;
use crate::query::executor::streaming::slot::{combine_layouts, SlotLayout};

use super::{build_combined_names, close_common};

pub(super) fn next_inner_join(
    join_condition: &mut Option<Expression>,
    build_side_tuples: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !*left_consumed {
        let mut captured_right_names = Vec::new();
        while let Some(chunk) = right.advance()? {
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

    if let Some(left_chunk) = left.advance()? {
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

        if result_rows.is_empty() {
            Ok(None)
        } else {
            let left_layout = left_chunk.get_layout();
            let right_layout = Arc::new(SlotLayout::from_names(&build_combined_names(
                &[],
                right_col_names,
                build_side_tuples.first().map(|r| r.len()).unwrap_or(0),
            )));
            let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
            Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn next_left_join(
    join_condition: &mut Option<Expression>,
    build_side_tuples: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !*left_consumed {
        let mut captured_right_names = Vec::new();
        while let Some(chunk) = right.advance()? {
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

    if let Some(left_chunk) = left.advance()? {
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
                for _ in 0..build_side_tuples.first().map(|r| r.len()).unwrap_or(0) {
                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                }
                result_rows.push(unmatched_row);
            }
        }

        if result_rows.is_empty() {
            Ok(None)
        } else {
            let left_layout = left_chunk.get_layout();
            let right_layout = Arc::new(SlotLayout::from_names(&build_combined_names(
                &[],
                right_col_names,
                build_side_tuples.first().map(|r| r.len()).unwrap_or(0),
            )));
            let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
            Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn next_right_join(
    join_condition: &mut Option<Expression>,
    build_side_tuples: &mut Vec<Vec<Value>>,
    right_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !*right_consumed {
        let mut captured_left_names = Vec::new();
        while let Some(chunk) = left.advance()? {
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

    if let Some(right_chunk) = right.advance()? {
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
                for _ in 0..build_side_tuples.first().map(|r| r.len()).unwrap_or(0) {
                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                }
                unmatched_row.extend(right_row.clone());
                result_rows.push(unmatched_row);
            }
        }

        if result_rows.is_empty() {
            Ok(None)
        } else {
            let left_layout = if let Some(first_left) = build_side_tuples.first() {
                Arc::new(SlotLayout::from_names(&build_combined_names(
                    right_col_names,
                    &[],
                    first_left.len(),
                )))
            } else {
                Arc::new(SlotLayout::from_names(&[]))
            };
            let right_layout = Arc::new(SlotLayout::from_names(&right_cols));
            let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
            Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn next_full_outer_join(
    join_condition: &mut Option<Expression>,
    left_rows: &mut Vec<Vec<Value>>,
    right_rows: &mut Vec<Vec<Value>>,
    matched_right_indices: &mut HashSet<usize>,
    result_iter: &mut Option<std::vec::IntoIter<Vec<Value>>>,
    phase: &mut FullOuterJoinPhase,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    loop {
        match phase {
            FullOuterJoinPhase::BuildingRight => {
                let mut captured_right_names = Vec::new();
                while let Some(chunk) = left.advance()? {
                    base.ensure_not_cancelled()?;
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    left_rows.extend(chunk.rows);
                }
                while let Some(chunk) = right.advance()? {
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
                let right_col_count = right_rows.first().map(|r| r.len()).unwrap_or(0);
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
                        for _ in 0..right_col_count {
                            unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                        }
                        all_results.push(unmatched_row);
                    }
                }

                *phase = FullOuterJoinPhase::EmitUnmatchedRight;
                if !all_results.is_empty() {
                    let left_layout = Arc::new(SlotLayout::from_names(
                        &(0..left_rows.first().map(|r| r.len()).unwrap_or(0))
                            .map(|i| format!("col_{}", i))
                            .collect::<Vec<_>>(),
                    ));
                    let right_layout = Arc::new(SlotLayout::from_names(&build_combined_names(
                        &[],
                        right_col_names,
                        right_col_count,
                    )));
                    let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                    let rows: Vec<Vec<Value>> = all_results.into_iter().collect();
                    if !rows.is_empty() {
                        *result_iter = Some(rows.into_iter());
                        return Ok(Some(DataChunk::new_with_layout(
                            result_iter.as_mut().unwrap().collect::<Vec<_>>(),
                            layout,
                        )));
                    }
                }
            }

            FullOuterJoinPhase::EmitUnmatchedRight => {
                if let Some(iter) = result_iter {
                    let rows: Vec<Vec<Value>> = iter.collect();
                    if !rows.is_empty() {
                        let left_layout = Arc::new(SlotLayout::from_names(
                            &(0..left_rows.first().map(|r| r.len()).unwrap_or(0))
                                .map(|i| format!("col_{}", i))
                                .collect::<Vec<_>>(),
                        ));
                        let right_layout = Arc::new(SlotLayout::from_names(&build_combined_names(
                            &[],
                            right_col_names,
                            right_rows.first().map(|r| r.len()).unwrap_or(0),
                        )));
                        let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                        return Ok(Some(DataChunk::new_with_layout(rows, layout)));
                    }
                    *result_iter = None;
                }

                let left_col_count = left_rows.first().map(|r| r.len()).unwrap_or(0);
                let mut unmatched = Vec::new();
                for (right_idx, right_row) in right_rows.iter().enumerate() {
                    if !matched_right_indices.contains(&right_idx) {
                        let mut row = Vec::new();
                        for _ in 0..left_col_count {
                            row.push(Value::Null(crate::core::value::NullType::Null));
                        }
                        row.extend(right_row.clone());
                        unmatched.push(row);
                    }
                }

                if unmatched.is_empty() {
                    return Ok(None);
                }
                let left_layout = Arc::new(SlotLayout::from_names(
                    &(0..left_col_count)
                        .map(|i| format!("col_{}", i))
                        .collect::<Vec<_>>(),
                ));
                let right_layout = Arc::new(SlotLayout::from_names(&build_combined_names(
                    &[],
                    right_col_names,
                    right_rows.first().map(|r| r.len()).unwrap_or(0),
                )));
                let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                return Ok(Some(DataChunk::new_with_layout(unmatched, layout)));
            }
        }
    }
}

pub(super) fn close_full_outer(
    lifecycle: &mut OperatorLifecycle,
    memory_tracker: &mut MemoryTracker,
    left_rows: &mut Vec<Vec<Value>>,
    right_rows: &mut Vec<Vec<Value>>,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<(), QueryError> {
    close_common(
        lifecycle,
        memory_tracker,
        || {
            left_rows.clear();
            right_rows.clear();
        },
        left,
        right,
    )
}

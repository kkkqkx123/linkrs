use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::base::OperatorLifecycle;

use super::close_common;

pub(super) fn next_cross_join(
    all_left_rows: &mut Vec<Vec<Value>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    right_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !*left_consumed {
        while let Some(chunk) = left.advance()? {
            base.ensure_not_cancelled()?;
            for row in &chunk.rows {
                memory_tracker.try_reserve_row(row)?;
            }
            all_left_rows.extend(chunk.rows);
        }
        *left_consumed = true;
    }

    if !*right_consumed {
        let mut captured_right_names = Vec::new();
        while let Some(chunk) = right.advance()? {
            base.ensure_not_cancelled()?;
            if captured_right_names.is_empty() {
                captured_right_names = chunk.col_names();
            }
            for row in &chunk.rows {
                memory_tracker.try_reserve_row(row)?;
            }
            all_right_rows.extend(chunk.rows);
        }
        *right_col_names = captured_right_names;
        *right_consumed = true;
    }

    if all_left_rows.is_empty() || all_right_rows.is_empty() {
        return Ok(None);
    }

    let mut result_rows = Vec::new();
    for left_row in all_left_rows.iter() {
        base.ensure_not_cancelled()?;
        for right_row in all_right_rows.iter() {
            let mut joined_row = left_row.clone();
            joined_row.extend(right_row.clone());
            result_rows.push(joined_row);
        }
    }

    if result_rows.is_empty() {
        Ok(None)
    } else {
        Ok(Some(DataChunk::new_with_layout(
            result_rows,
            Arc::clone(&base.output_layout),
        )))
    }
}

pub(super) fn next_semi_join(
    join_condition: &mut Option<Expression>,
    right_rows: &mut Vec<Vec<Value>>,
    right_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !*right_consumed {
        while let Some(chunk) = right.advance()? {
            base.ensure_not_cancelled()?;
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                right_rows.push(row);
            }
        }
        *right_consumed = true;
    }

    while let Some(left_chunk) = left.advance()? {
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        for left_row in &left_chunk.rows {
            for right_row in right_rows.iter() {
                let condition_satisfied = if let Some(condition) = join_condition {
                    let mut combined_row = left_row.clone();
                    combined_row.extend(right_row.clone());
                    let mut combined_col_names = left_col_names.clone();
                    for i in 0..right_row.len() {
                        combined_col_names.push(format!("right_{}", i));
                    }
                    let mut context = ValueRowContext::from_names(combined_row, combined_col_names);
                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                        Ok(Value::Bool(b)) => b,
                        _ => false,
                    }
                } else {
                    true
                };

                if condition_satisfied {
                    result_rows.push(left_row.clone());
                    break;
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

pub(super) fn close_cross(
    lifecycle: &mut OperatorLifecycle,
    memory_tracker: &mut MemoryTracker,
    all_left_rows: &mut Vec<Vec<Value>>,
    all_right_rows: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    close_common(
        lifecycle,
        memory_tracker,
        || {
            all_left_rows.clear();
            all_right_rows.clear();
        },
    )
}

pub(super) fn close_semi(
    lifecycle: &mut OperatorLifecycle,
    memory_tracker: &mut MemoryTracker,
    right_rows: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    close_common(
        lifecycle,
        memory_tracker,
        || {
            right_rows.clear();
        },
    )
}

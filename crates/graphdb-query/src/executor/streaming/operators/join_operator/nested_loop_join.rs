use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::executor::base::MemoryTracker;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::executor::ValueRowContext;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;

use super::build_combined_names;

#[allow(clippy::too_many_arguments)]
pub(super) fn next_nested_loop_join(
    join_condition: &mut Option<Expression>,
    build_side_tuples: &mut Vec<Vec<Value>>,
    build_done: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
) -> Result<Option<DataChunk>, QueryError> {
    if !*build_done {
        let mut captured_right_names = Vec::new();
        while let Some(mut chunk) = right.advance()? {
            chunk.materialize_selection_by("NestedLoopJoin");
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if captured_right_names.is_empty() {
                captured_right_names = chunk.col_names();
            }
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                build_side_tuples.push(row);
            }
        }
        *right_col_names = captured_right_names;
        *build_done = true;
    }

    while let Some(mut left_chunk) = left.advance()? {
        left_chunk.materialize_selection_by("NestedLoopJoin");
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
                        Ok(value) => match value {
                            Value::Bool(b) => b,
                            Value::Null(_) => false,
                            _ => true,
                        },
                        Err(e) => {
                            return Err(QueryError::execution(format!(
                                "NestedLoopJoin condition evaluation failed: {}",
                                e
                            )));
                        }
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
                Arc::clone(output_layout),
            )));
        }
    }

    Ok(None)
}

pub(super) fn close(
    memory_tracker: &mut MemoryTracker,
    build_side_tuples: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    memory_tracker.reset();
    build_side_tuples.clear();
    Ok(())
}

use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::executor::ValueRowContext;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

#[allow(clippy::too_many_arguments)]
pub(super) fn next_cross_join(
    all_left_rows: &mut Vec<Vec<Value>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    right_consumed: &mut bool,
    output_done: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
) -> Result<Option<DataChunk>, QueryError> {
    // The full cartesian product is emitted in a single chunk; subsequent
    // pulls must report exhaustion instead of re-emitting it forever.
    if *output_done {
        return Ok(None);
    }
    if !*left_consumed {
        while let Some(mut chunk) = left.advance()? {
            chunk.materialize_selection_by("CrossSemiJoin");
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            for row in &chunk.rows {
                memory_tracker.try_reserve_row(row)?;
            }
            all_left_rows.extend(chunk.rows);
        }
        *left_consumed = true;
    }

    if !*right_consumed {
        let mut captured_right_names = Vec::new();
        while let Some(mut chunk) = right.advance()? {
            chunk.materialize_selection_by("CrossSemiJoin");
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
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
        *output_done = true;
        return Ok(None);
    }

    let mut result_rows = Vec::new();
    for left_row in all_left_rows.iter() {
        if let Some(rt) = runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        for right_row in all_right_rows.iter() {
            let mut joined_row = left_row.clone();
            joined_row.extend(right_row.clone());
            result_rows.push(joined_row);
        }
    }

    *output_done = true;
    if result_rows.is_empty() {
        Ok(None)
    } else {
        Ok(Some(DataChunk::new_with_layout(
            result_rows,
            Arc::clone(output_layout),
        )))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn next_semi_join(
    join_condition: &mut Option<Expression>,
    anti: bool,
    right_rows: &mut Vec<Vec<Value>>,
    right_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
) -> Result<Option<DataChunk>, QueryError> {
    if !*right_consumed {
        while let Some(mut chunk) = right.advance()? {
            chunk.materialize_selection_by("CrossSemiJoin");
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if right_col_names.is_empty() {
                *right_col_names = chunk.col_names();
            }
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                right_rows.push(row);
            }
        }
        *right_consumed = true;
    }

    while let Some(mut left_chunk) = left.advance()? {
        left_chunk.materialize_selection_by("CrossSemiJoin");
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        for left_row in &left_chunk.rows {
            // Semi join: keep the left row when ANY right row satisfies the
            // condition. Anti join (NOT EXISTS): keep the left row when NO
            // right row satisfies it (empty right side always matches anti).
            let mut matched = false;
            for right_row in right_rows.iter() {
                let condition_satisfied = if let Some(condition) = join_condition {
                    let mut combined_row = left_row.clone();
                    combined_row.extend(right_row.clone());
                    let mut combined_col_names = left_col_names.clone();
                    // Use the real right column names so the Mark-Join
                    // residual condition can reference right-side variables
                    // (e.g. `t.age`) instead of synthetic slots.
                    combined_col_names.extend(right_col_names.clone());
                    let mut context = ValueRowContext::from_names(combined_row, combined_col_names);
                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                        Ok(Value::Bool(b)) => b,
                        _ => false,
                    }
                } else {
                    true
                };

                if condition_satisfied {
                    matched = true;
                    break;
                }
            }
            if matched != anti {
                result_rows.push(left_row.clone());
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

pub(super) fn close_cross(
    memory_tracker: &mut MemoryTracker,
    all_left_rows: &mut Vec<Vec<Value>>,
    all_right_rows: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    memory_tracker.reset();
    all_left_rows.clear();
    all_right_rows.clear();
    Ok(())
}

pub(super) fn close_semi(
    memory_tracker: &mut MemoryTracker,
    right_rows: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    memory_tracker.reset();
    right_rows.clear();
    Ok(())
}

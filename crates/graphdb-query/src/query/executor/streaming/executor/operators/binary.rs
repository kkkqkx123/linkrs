//! Binary operators: HashJoin, NestedLoopJoin

use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::{StreamingExecutor, ValueRowContext};
use crate::query::executor::streaming::slot::{combine_layouts, SlotLayout};

// ============ HashJoin ============

pub fn open_hashjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::HashJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_hashjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::HashJoin {
            left,
            right,
            join_condition,
            hash_keys,
            probe_keys,
            build_side_hash,
            all_right_rows,
            left_consumed,
            memory_tracker,
            right_col_names,
            ..
        } => {
            if !*left_consumed {
                let mut captured_right_names = Vec::new();
                while let Some(chunk) = right.advance()? {
                    if captured_right_names.is_empty() {
                        captured_right_names = chunk.col_names();
                    }
                    for row in chunk.rows {
                        memory_tracker.try_reserve_row(&row)?;
                        all_right_rows.push(row.clone());
                        if !hash_keys.is_empty() {
                            let names = if captured_right_names.is_empty() {
                                &(0..row.len()).map(|i| format!("right_{}", i)).collect()
                            } else {
                                &captured_right_names
                            };
                            let key = evaluate_join_key(&row, names, hash_keys)?;
                            build_side_hash.entry(key).or_default().push(row);
                        }
                    }
                }
                *right_col_names = captured_right_names;
                *left_consumed = true;
            }

            if all_right_rows.is_empty() {
                return Ok(None);
            }

            if let Some(left_chunk) = left.advance()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                if hash_keys.is_empty() && probe_keys.is_empty() {
                    for left_row in &left_chunk.rows {
                        for right_row in all_right_rows.iter() {
                            let mut joined_row = left_row.clone();
                            joined_row.extend(right_row.clone());
                            let combined_names =
                                build_combined_names(&left_col_names, right_col_names, right_row.len());
                            let mut ctx = ValueRowContext::new(joined_row.clone(), combined_names);
                            let satisfied = if let Some(condition) = join_condition {
                                match ExpressionEvaluator::evaluate(condition, &mut ctx) {
                                    Ok(Value::Bool(b)) => b,
                                    Ok(Value::Null(_)) => false,
                                    Ok(_) => true,
                                    Err(e) => {
                                        return Err(QueryError::execution(format!(
                                            "HashJoin condition evaluation failed: {}",
                                            e
                                        )));
                                    }
                                }
                            } else {
                                true
                            };
                            if satisfied {
                                result_rows.push(joined_row);
                            }
                        }
                    }
                } else {
                    for left_row in &left_chunk.rows {
                        let probe_key = evaluate_join_key(left_row, &left_col_names, probe_keys)?;
                        if let Some(matching_rows) = build_side_hash.get(&probe_key) {
                            for right_row in matching_rows {
                                let combined = left_row.clone();
                                let combined_names = build_combined_names(&left_col_names, right_col_names, right_row.len());
                                let mut ctx = ValueRowContext::new(combined, combined_names);

                                let satisfied = if let Some(condition) = join_condition {
                                    match ExpressionEvaluator::evaluate(condition, &mut ctx) {
                                        Ok(Value::Bool(b)) => b,
                                        Ok(Value::Null(_)) => false,
                                        Ok(_) => true,
                                        Err(e) => {
                                            return Err(QueryError::execution(format!(
                                                "HashJoin condition evaluation failed: {}",
                                                e
                                            )));
                                        }
                                    }
                                } else {
                                    true
                                };

                                if satisfied {
                                    let mut joined_row = left_row.clone();
                                    joined_row.extend(right_row.clone());
                                    result_rows.push(joined_row);
                                }
                            }
                        }
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    let left_layout = left_chunk.get_or_create_layout();
                    let right_layout = if right_col_names.is_empty() {
                        Arc::new(SlotLayout::from_names(
                            &all_right_rows.first().map(|r| {
                                (0..r.len()).map(|i| format!("right_{}", i)).collect::<Vec<_>>()
                            }).unwrap_or_default(),
                        ))
                    } else {
                        Arc::new(SlotLayout::from_names(right_col_names))
                    };
                    let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                    Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_hashjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::HashJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_hashjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::HashJoin {
            left,
            right,
            opened,
            build_side_hash,
            all_right_rows,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = MemoryBudget::estimate_rows_memory(all_right_rows);
                memory_tracker.release(mem);
                build_side_hash.clear();
                all_right_rows.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn reset_hashjoin(executor: &mut StreamingExecutor) {
    if let StreamingExecutor::HashJoin {
        build_side_hash,
        all_right_rows,
        left_consumed,
        right_col_names,
        ..
    } = executor
    {
        build_side_hash.clear();
        all_right_rows.clear();
        *left_consumed = false;
        right_col_names.clear();
    }
}

/// Build combined column names from left and right inputs.
/// Falls back to `right_N` when right column names are unavailable.
fn build_combined_names(
    left_col_names: &[String],
    right_col_names: &[String],
    fallback_right_width: usize,
) -> Vec<String> {
    let mut names = left_col_names.to_vec();
    if !right_col_names.is_empty() {
        names.extend_from_slice(right_col_names);
    } else {
        for i in 0..fallback_right_width {
            names.push(format!("right_{}", i));
        }
    }
    names
}

fn evaluate_join_key(
    row: &[Value],
    col_names: &[String],
    key_expressions: &[crate::core::types::expr::Expression],
) -> Result<Vec<Value>, QueryError> {
    if key_expressions.is_empty() {
        return Ok(Vec::new());
    }

    let mut key = Vec::with_capacity(key_expressions.len());
    for expr in key_expressions {
        let mut context = ValueRowContext::new(row.to_vec(), col_names.to_vec());
        let value = ExpressionEvaluator::evaluate(expr, &mut context)
            .map_err(|e| QueryError::execution(format!("HashJoin key evaluation failed: {}", e)))?;
        key.push(value);
    }
    Ok(key)
}

// ============ NestedLoopJoin ============

pub fn open_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_nestedloopjoin(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            memory_tracker,
            ..
        } => {
            if !*left_consumed {
                // Build right side - collect all rows
                while let Some(chunk) = right.advance()? {
                    for row in chunk.rows {
                        memory_tracker.try_reserve_row(&row)?;
                        build_side_tuples.push(row);
                    }
                }
                *left_consumed = true;
            }

            if let Some(left_chunk) = left.advance()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                for left_row in &left_chunk.rows {
                    for right_row in build_side_tuples.iter() {
                        // Always evaluate condition for nested loop join
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());

                            let mut combined_col_names = left_col_names.clone();
                            for i in 0..right_row.len() {
                                combined_col_names.push(format!("right_{}", i));
                            }

                            let mut context =
                                ValueRowContext::new(combined_row, combined_col_names);
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
                            // Cartesian product
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin {
            left,
            right,
            opened,
            build_side_tuples,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                memory_tracker.release(mem);
                build_side_tuples.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ InnerJoin (standard) ============

pub fn open_innerjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InnerJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_innerjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::InnerJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            memory_tracker,
            ..
        } => {
            if !*left_consumed {
                // Build right side
                while let Some(chunk) = right.advance()? {
                    for row in chunk.rows {
                        memory_tracker.try_reserve_row(&row)?;
                        build_side_tuples.push(row);
                    }
                }
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
                            let mut combined_col_names = left_col_names.clone();
                            for i in 0..right_row.len() {
                                combined_col_names.push(format!("right_{}", i));
                            }
                            let mut context =
                                ValueRowContext::new(combined_row, combined_col_names);
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_innerjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InnerJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_innerjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InnerJoin {
            left,
            right,
            opened,
            build_side_tuples,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                memory_tracker.release(mem);
                build_side_tuples.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ LeftJoin ============

pub fn open_leftjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LeftJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_leftjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::LeftJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            memory_tracker,
            ..
        } => {
            if !*left_consumed {
                while let Some(chunk) = right.advance()? {
                    for row in chunk.rows {
                        memory_tracker.try_reserve_row(&row)?;
                        build_side_tuples.push(row);
                    }
                }
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
                            let mut combined_col_names = left_col_names.clone();
                            for i in 0..right_row.len() {
                                combined_col_names.push(format!("right_{}", i));
                            }
                            let mut context =
                                ValueRowContext::new(combined_row, combined_col_names);
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

                    // If no match, emit left row with NULLs for right columns
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_leftjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LeftJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_leftjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LeftJoin {
            left,
            right,
            opened,
            build_side_tuples,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                memory_tracker.release(mem);
                build_side_tuples.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ RightJoin ============

pub fn open_rightjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RightJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_rightjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::RightJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            right_consumed,
            memory_tracker,
            ..
        } => {
            if !*right_consumed {
                while let Some(chunk) = left.advance()? {
                    for row in chunk.rows {
                        memory_tracker.try_reserve_row(&row)?;
                        build_side_tuples.push(row);
                    }
                }
                *right_consumed = true;
            }

            if let Some(right_chunk) = right.advance()? {
                let right_col_names = right_chunk.col_names();
                let mut result_rows = Vec::new();

                for right_row in &right_chunk.rows {
                    let mut matched = false;
                    for left_row in build_side_tuples.iter() {
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());
                            let mut combined_col_names = right_col_names.clone();
                            for i in 0..left_row.len() {
                                combined_col_names.push(format!("left_{}", i));
                            }
                            let mut context =
                                ValueRowContext::new(combined_row, combined_col_names);
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_rightjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RightJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_rightjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RightJoin {
            left,
            right,
            opened,
            build_side_tuples,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                memory_tracker.release(mem);
                build_side_tuples.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ FullOuterJoin ============

pub fn open_fullouterjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FullOuterJoin {
            left,
            right,
            opened,
            phase,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            *phase = crate::query::executor::streaming::executor::FullOuterJoinPhase::BuildingRight;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_fullouterjoin(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::FullOuterJoin {
            left,
            right,
            join_condition,
            left_rows,
            right_rows,
            matched_right_indices,
            result_iter,
            phase,
            memory_tracker,
            ..
        } => {
            loop {
                match phase {
                    crate::query::executor::streaming::executor::FullOuterJoinPhase::BuildingRight => {
                        // Collect all left and right rows
                        while let Some(chunk) = left.advance()? {
                            for row in &chunk.rows {
                                memory_tracker.try_reserve_row(row)?;
                            }
                            left_rows.extend(chunk.rows);
                        }
                        while let Some(chunk) = right.advance()? {
                            for row in &chunk.rows {
                                memory_tracker.try_reserve_row(row)?;
                            }
                            right_rows.extend(chunk.rows);
                        }
                        *phase = crate::query::executor::streaming::executor::FullOuterJoinPhase::ProbeLeft;
                    }

                    crate::query::executor::streaming::executor::FullOuterJoinPhase::ProbeLeft => {
                        let right_col_count = right_rows.first().map(|r| r.len()).unwrap_or(0);
                        let mut all_results = Vec::new();

                        for left_row in left_rows.iter() {
                            let mut matched = false;
                            for (right_idx, right_row) in right_rows.iter().enumerate() {
                                let condition_satisfied = if let Some(condition) = join_condition {
                                    let left_col_names: Vec<String> = (0..left_row.len()).map(|i| format!("col_{}", i)).collect();
                                    let mut combined_row = left_row.clone();
                                    combined_row.extend(right_row.clone());
                                    let mut combined_col_names = left_col_names.clone();
                                    for i in 0..right_row.len() {
                                        combined_col_names.push(format!("right_{}", i));
                                    }
                                    let mut context = ValueRowContext::new(combined_row, combined_col_names);
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

                        *phase = crate::query::executor::streaming::executor::FullOuterJoinPhase::EmitUnmatchedRight;
                        if !all_results.is_empty() {
                            *result_iter = Some(all_results.into_iter());
                        }
                        continue;
                    }

                    crate::query::executor::streaming::executor::FullOuterJoinPhase::EmitUnmatchedRight => {
                        // Drain buffered matched+unmatched-left results first
                        if let Some(iter) = result_iter {
                            let rows: Vec<Vec<Value>> = iter.collect();
                            if !rows.is_empty() {
                                return Ok(Some(DataChunk::from_rows(rows)));
                            }
                            *result_iter = None;
                        }

                        // Emit unmatched right rows
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
                        return Ok(Some(DataChunk::from_rows(unmatched)));
                    }
                }
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_fullouterjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FullOuterJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_fullouterjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FullOuterJoin {
            left,
            right,
            opened,
            left_rows,
            right_rows,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem_left = MemoryBudget::estimate_rows_memory(left_rows);
                let mem_right = MemoryBudget::estimate_rows_memory(right_rows);
                memory_tracker.release(mem_left + mem_right);
                left_rows.clear();
                right_rows.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ CrossJoin ============

pub fn open_crossjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::CrossJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_crossjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::CrossJoin {
            left,
            right,
            all_left_rows,
            all_right_rows,
            left_consumed,
            right_consumed,
            memory_tracker,
            ..
        } => {
            if !*left_consumed {
                while let Some(chunk) = left.advance()? {
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    all_left_rows.extend(chunk.rows);
                }
                *left_consumed = true;
            }

            if !*right_consumed {
                while let Some(chunk) = right.advance()? {
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    all_right_rows.extend(chunk.rows);
                }
                *right_consumed = true;
            }

            if all_left_rows.is_empty() || all_right_rows.is_empty() {
                return Ok(None);
            }

            let mut result_rows = Vec::new();
            for left_row in all_left_rows.iter() {
                for right_row in all_right_rows.iter() {
                    let mut joined_row = left_row.clone();
                    joined_row.extend(right_row.clone());
                    result_rows.push(joined_row);
                }
            }

            if result_rows.is_empty() {
                Ok(None)
            } else {
                Ok(Some(DataChunk::from_rows(result_rows)))
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_crossjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::CrossJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_crossjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::CrossJoin {
            left,
            right,
            opened,
            all_left_rows,
            all_right_rows,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem_left = MemoryBudget::estimate_rows_memory(all_left_rows);
                let mem_right = MemoryBudget::estimate_rows_memory(all_right_rows);
                memory_tracker.release(mem_left + mem_right);
                all_left_rows.clear();
                all_right_rows.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ SemiJoin ============

pub fn open_semijoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SemiJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_semijoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    executor.ensure_not_cancelled()?;
    match executor {
        StreamingExecutor::SemiJoin {
            left,
            right,
            join_condition,
            right_rows,
            right_consumed,
            memory_tracker,
            ..
        } => {
            if !*right_consumed {
                while let Some(chunk) = right.advance()? {
                    for row in chunk.rows {
                        memory_tracker.try_reserve_row(&row)?;
                        right_rows.push(row);
                    }
                }
                *right_consumed = true;
            }

            if let Some(left_chunk) = left.advance()? {
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
                            let mut context =
                                ValueRowContext::new(combined_row, combined_col_names);
                            match ExpressionEvaluator::evaluate(condition, &mut context) {
                                Ok(Value::Bool(b)) => b,
                                _ => false,
                            }
                        } else {
                            true
                        };

                        if condition_satisfied {
                            result_rows.push(left_row.clone());
                            break; // SemiJoin only returns one copy of left row
                        }
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_semijoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SemiJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_semijoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SemiJoin {
            left,
            right,
            opened,
            right_rows,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = MemoryBudget::estimate_rows_memory(right_rows);
                memory_tracker.release(mem);
                right_rows.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;
    use crate::core::value::NullType;
use crate::query::executor::base::MemoryBudget;
#[cfg(test)]
use crate::query::executor::base::MemoryTracker;

    fn create_left_buffer() -> Vec<Vec<Value>> {
        vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ]
    }

    fn create_right_buffer() -> Vec<Vec<Value>> {
        vec![
            vec![Value::Int(1), Value::String("x".to_string())],
            vec![Value::Int(2), Value::String("y".to_string())],
            vec![Value::Int(3), Value::String("z".to_string())],
        ]
    }

    #[test]
    fn test_hashjoin_basic() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: create_left_buffer(),
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: create_right_buffer(),
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            hash_keys: vec![],
            probe_keys: vec![],
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            right_col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        join.open().unwrap();
        let chunk = join.advance().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 left rows × 3 right rows = 6 result rows
        assert_eq!(chunk.unwrap().len(), 6);
        join.close().unwrap();
    }

    #[test]
    fn test_hashjoin_no_match() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(10), Value::String("a".to_string())]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(20), Value::String("b".to_string())]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let join_condition = Some(Expression::Literal(Value::Bool(false)));

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition,
            hash_keys: vec![],
            probe_keys: vec![],
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            right_col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        join.open().unwrap();
        let chunk = join.advance().unwrap();
        assert!(chunk.is_none());
        join.close().unwrap();
    }

    #[test]
    fn test_hashjoin_multi_match() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::String("a1".to_string())],
                vec![Value::Int(1), Value::String("a2".to_string())],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::String("b1".to_string())],
                vec![Value::Int(1), Value::String("b2".to_string())],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            hash_keys: vec![],
            probe_keys: vec![],
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            right_col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        join.open().unwrap();
        let chunk = join.advance().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 left rows × 2 right rows = 4 result rows
        assert_eq!(chunk.unwrap().len(), 4);
        join.close().unwrap();
    }

    #[test]
    fn test_nestedloop_cartesian() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(10)],
                vec![Value::Int(20)],
                vec![Value::Int(30)],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut join = StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition: None,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        join.open().unwrap();
        let chunk = join.advance().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 × 3 = 6 rows
        assert_eq!(chunk.unwrap().len(), 6);
        join.close().unwrap();
    }

    #[test]
    fn test_nestedloop_condition() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        // Condition: always true
        let join_condition = Some(Expression::Literal(Value::Bool(true)));

        let mut join = StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        join.open().unwrap();
        let chunk = join.advance().unwrap();
        assert!(chunk.is_some());
        // 2 × 2 = 4 rows
        assert_eq!(chunk.unwrap().len(), 4);
        join.close().unwrap();
    }

    #[test]
    fn test_join_null() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::Null(NullType::Null)],
                vec![Value::Int(2), Value::String("b".to_string())],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::String("x".to_string()), Value::Int(10)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            hash_keys: vec![],
            probe_keys: vec![],
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            right_col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        join.open().unwrap();
        let chunk = join.advance().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 left rows × 1 right row = 2 result rows
        assert_eq!(chunk.unwrap().len(), 2);
        join.close().unwrap();
    }

    #[test]
    fn test_join_column_naming() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1), Value::String("left".to_string())]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(2), Value::String("right".to_string())]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            hash_keys: vec![],
            probe_keys: vec![],
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            right_col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        join.open().unwrap();
        let chunk = join.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // Result row should have 4 columns (2 from left + 2 from right)
        assert_eq!(chunk.rows[0].len(), 4);
        join.close().unwrap();
    }
}

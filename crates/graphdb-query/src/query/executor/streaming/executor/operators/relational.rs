//! Simple relational operator implementations
//!
//! Includes: TopN, Dedup, Assign, Materialize, Remove, DataCollect, Unwind,
//! Apply, PatternApply, RollUpApply, Minus, Window

use super::super::helpers::comparison::compare_values;
use super::super::{SortDirection, StreamingExecutor, ValueRowContext};
use crate::core::error::QueryError;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::base::{MemoryBudget, MemoryTracker};
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;

// ============ TopN ============

pub fn open_topn(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TopN { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_topn".to_string(),
        )),
    }
}

pub fn next_topn(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TopN {
            input,
            n,
            sort_expressions,
            sort_directions,
            all_rows,
            result_iter,
            opened,
            memory_tracker,
        } => {
            if !*opened {
                return Err(QueryError::execution("TopN not opened".to_string()));
            }

            if result_iter.is_none() {
                let mut col_names: Vec<String> = vec![];
                while let Some(chunk) = input.next()? {
                    if col_names.is_empty() {
                        col_names = chunk.col_names();
                    }
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    all_rows.extend(chunk.rows);
                }

                all_rows.sort_by(|a, b| {
                    for (idx, expr) in sort_expressions.iter().enumerate() {
                        let direction = sort_directions
                            .get(idx)
                            .copied()
                            .unwrap_or(SortDirection::Ascending);

                        let mut ctx_a = ValueRowContext::new(a.clone(), col_names.clone());
                        let mut ctx_b = ValueRowContext::new(b.clone(), col_names.clone());

                        let val_a = ExpressionEvaluator::evaluate(expr, &mut ctx_a)
                            .unwrap_or(Value::Null(NullType::Null));
                        let val_b = ExpressionEvaluator::evaluate(expr, &mut ctx_b)
                            .unwrap_or(Value::Null(NullType::Null));

                        let cmp = compare_values(&val_a, &val_b);

                        let final_cmp = match direction {
                            SortDirection::Ascending => cmp,
                            SortDirection::Descending => cmp.reverse(),
                        };

                        if final_cmp != std::cmp::Ordering::Equal {
                            return final_cmp;
                        }
                    }
                    std::cmp::Ordering::Equal
                });

                all_rows.truncate(*n as usize);
                *result_iter = Some(all_rows.drain(..).collect::<Vec<_>>().into_iter());
            }

            if let Some(iter) = result_iter {
                if let Some(row) = iter.next() {
                    Ok(Some(DataChunk::from_rows(vec![row])))
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_topn".to_string(),
        )),
    }
}

pub fn stop_topn(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TopN { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_topn".to_string(),
        )),
    }
}

pub fn close_topn(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TopN {
            input,
            all_rows,
            result_iter,
            memory_tracker,
            ..
        } => {
            let mem = MemoryBudget::estimate_rows_memory(all_rows);
            memory_tracker.release(mem);
            all_rows.clear();
            *result_iter = None;
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_topn".to_string(),
        )),
    }
}

// ============ Dedup ============

pub fn open_dedup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Dedup { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_dedup".to_string(),
        )),
    }
}

pub fn next_dedup(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Dedup {
            input,
            seen_rows,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("Dedup not opened".to_string()));
            }

            while let Some(chunk) = input.next()? {
                let mut result_rows = vec![];
                for row in chunk.rows {
                    let row_str = format!("{:?}", row);
                    if !seen_rows.contains(&row_str) {
                        seen_rows.insert(row_str);
                        result_rows.push(row);
                    }
                }

                if !result_rows.is_empty() {
                    return Ok(Some(DataChunk::from_rows(result_rows)));
                }
            }
            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_dedup".to_string(),
        )),
    }
}

pub fn stop_dedup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Dedup { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_dedup".to_string(),
        )),
    }
}

pub fn close_dedup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Dedup {
            input, seen_rows, ..
        } => {
            input.close()?;
            seen_rows.clear();
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_dedup".to_string(),
        )),
    }
}

// ============ Assign ============

pub fn open_assign(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Assign { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_assign".to_string(),
        )),
    }
}

pub fn next_assign(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Assign {
            input,
            assignments,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("Assign not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let col_names = chunk.col_names();
                let mut result_rows = vec![];
                for row in chunk.rows {
                    let mut new_row = row.clone();
                    for (_col_name, expr) in assignments.iter() {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        // Evaluate expression and append to row
                        match ExpressionEvaluator::evaluate(expr, &mut context) {
                            Ok(val) => new_row.push(val),
                            Err(_) => new_row.push(Value::Null(crate::core::value::NullType::Null)),
                        }
                    }
                    result_rows.push(new_row);
                }

                return Ok(Some(DataChunk::from_rows(result_rows)));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_assign".to_string(),
        )),
    }
}

pub fn stop_assign(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Assign { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_assign".to_string(),
        )),
    }
}

pub fn close_assign(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Assign { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_assign".to_string(),
        )),
    }
}

// ============ Materialize ============

pub fn open_materialize(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Materialize { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_materialize".to_string(),
        )),
    }
}

pub fn next_materialize(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Materialize {
            input,
            materialized_rows,
            result_iter,
            materialized,
            opened,
            memory_tracker,
        } => {
            if !*opened {
                return Err(QueryError::execution("Materialize not opened".to_string()));
            }

            // Materialize all rows on first call
            if !*materialized {
                while let Some(chunk) = input.next()? {
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    materialized_rows.extend(chunk.rows);
                }
                *materialized = true;
                *result_iter = Some(materialized_rows.drain(..).collect::<Vec<_>>().into_iter());
            }

            // Return cached rows
            if let Some(iter) = result_iter {
                if let Some(row) = iter.next() {
                    Ok(Some(DataChunk::from_rows(vec![row])))
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_materialize".to_string(),
        )),
    }
}

pub fn stop_materialize(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Materialize { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_materialize".to_string(),
        )),
    }
}

pub fn close_materialize(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Materialize {
            input,
            materialized_rows,
            result_iter,
            memory_tracker,
            ..
        } => {
            let mem = MemoryBudget::estimate_rows_memory(materialized_rows);
            memory_tracker.release(mem);
            materialized_rows.clear();
            *result_iter = None;
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_materialize".to_string(),
        )),
    }
}

// ============ Remove ============

pub fn open_remove(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Remove { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_remove".to_string(),
        )),
    }
}

pub fn next_remove(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Remove {
            input,
            columns_to_remove,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("Remove not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let col_names = chunk.col_names();
                // Filter out columns to remove by name
                let mut new_col_names = vec![];
                let mut indices_to_keep = vec![];

                for (idx, col_name) in col_names.iter().enumerate() {
                    if !columns_to_remove.contains(col_name) {
                        new_col_names.push(col_name.clone());
                        indices_to_keep.push(idx);
                    }
                }

                let mut result_rows = vec![];
                for row in chunk.rows {
                    let mut new_row = vec![];
                    for idx in &indices_to_keep {
                        if *idx < row.len() {
                            new_row.push(row[*idx].clone());
                        }
                    }
                    result_rows.push(new_row);
                }

                return Ok(Some(DataChunk::from_rows(result_rows)));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_remove".to_string(),
        )),
    }
}

pub fn stop_remove(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Remove { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_remove".to_string(),
        )),
    }
}

pub fn close_remove(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Remove { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_remove".to_string(),
        )),
    }
}

// ============ DataCollect ============

pub fn open_datacollect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DataCollect { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_datacollect".to_string(),
        )),
    }
}

pub fn next_datacollect(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::DataCollect {
            input,
            all_rows,
            emitted,
            opened,
            memory_tracker,
        } => {
            if !*opened {
                return Err(QueryError::execution("DataCollect not opened".to_string()));
            }

            if *emitted {
                return Ok(None);
            }

            // Collect all rows and emit as single chunk
            while let Some(chunk) = input.next()? {
                for row in &chunk.rows {
                    memory_tracker.try_reserve_row(row)?;
                }
                all_rows.extend(chunk.rows);
            }

            if !all_rows.is_empty() {
                *emitted = true;
                let rows = all_rows.drain(..).collect::<Vec<_>>();
                return Ok(Some(DataChunk::from_rows(rows)));
            }

            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_datacollect".to_string(),
        )),
    }
}

pub fn stop_datacollect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DataCollect { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_datacollect".to_string(),
        )),
    }
}

pub fn close_datacollect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DataCollect {
            input,
            all_rows,
            memory_tracker,
            ..
        } => {
            let mem = MemoryBudget::estimate_rows_memory(all_rows);
            memory_tracker.release(mem);
            all_rows.clear();
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_datacollect".to_string(),
        )),
    }
}

// ============ Unwind ============

pub fn open_unwind(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Unwind {
            input,
            opened,
            col_index,
            ..
        } => {
            input.open()?;
            *opened = true;
            *col_index = None;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_unwind".to_string(),
        )),
    }
}

pub fn next_unwind(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Unwind {
            input,
            unwind_column,
            col_index,
            all_rows,
            current_row_index,
            current_unwind_index,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("Unwind not opened".to_string()));
            }

            // Collect all rows on first call
            if all_rows.is_empty() {
                while let Some(chunk) = input.next()? {
                    if col_index.is_none() && !chunk.rows.is_empty() {
                        // Compute column index from schema
                        let names = chunk.col_names();
                        *col_index = names.iter().position(|n| n == unwind_column.as_str());
                    }
                    all_rows.extend(chunk.rows);
                }
            }

            if all_rows.is_empty() {
                return Ok(None);
            }

            let col_idx = col_index.unwrap_or(0);
            const CHUNK_SIZE: usize = 1024;
            let mut result_rows = Vec::new();

            while result_rows.len() < CHUNK_SIZE && *current_row_index < all_rows.len() {
                let row = &all_rows[*current_row_index];
                if col_idx < row.len() {
                    let val = &row[col_idx];
                    match val {
                        Value::List(list) => {
                            if list.values.is_empty() {
                                *current_row_index += 1;
                                *current_unwind_index = 0;
                            } else {
                                let elements = &list.values;
                                while *current_unwind_index < elements.len()
                                    && result_rows.len() < CHUNK_SIZE
                                {
                                    let mut new_row = row.clone();
                                    new_row[col_idx] = elements[*current_unwind_index].clone();
                                    result_rows.push(new_row);
                                    *current_unwind_index += 1;
                                }
                                if *current_unwind_index >= elements.len() {
                                    *current_row_index += 1;
                                    *current_unwind_index = 0;
                                }
                            }
                        }
                        Value::Null(_) => {
                            *current_row_index += 1;
                            *current_unwind_index = 0;
                        }
                        _ => {
                            result_rows.push(row.clone());
                            *current_row_index += 1;
                            *current_unwind_index = 0;
                        }
                    }
                } else {
                    result_rows.push(row.clone());
                    *current_row_index += 1;
                    *current_unwind_index = 0;
                }
            }

            if result_rows.is_empty() {
                Ok(None)
            } else {
                Ok(Some(DataChunk::from_rows(result_rows)))
            }
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_unwind".to_string(),
        )),
    }
}

pub fn stop_unwind(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Unwind {
            input, col_index, ..
        } => {
            input.stop()?;
            *col_index = None;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_unwind".to_string(),
        )),
    }
}

pub fn close_unwind(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Unwind {
            input,
            all_rows,
            col_index,
            ..
        } => {
            input.close()?;
            all_rows.clear();
            *col_index = None;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_unwind".to_string(),
        )),
    }
}

// ============ Apply ============

pub fn open_apply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Apply { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_apply".to_string(),
        )),
    }
}

pub fn next_apply(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Apply {
            input,
            apply_expression,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("Apply not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let col_names = chunk.col_names();
                let mut result_rows = Vec::new();
                for row in chunk.rows {
                    let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                    match ExpressionEvaluator::evaluate(apply_expression, &mut context) {
                        Ok(val) => {
                            let mut new_row = row.clone();
                            new_row.push(val);
                            result_rows.push(new_row);
                        }
                        Err(_) => {}
                    }
                }

                if !result_rows.is_empty() {
                    return Ok(Some(DataChunk::from_rows(result_rows)));
                }
            }
            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_apply".to_string(),
        )),
    }
}

pub fn stop_apply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Apply { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_apply".to_string(),
        )),
    }
}

pub fn close_apply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Apply { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_apply".to_string(),
        )),
    }
}

// ============ PatternApply ============

pub fn open_patternapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PatternApply { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_patternapply".to_string(),
        )),
    }
}

pub fn next_patternapply(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::PatternApply {
            input,
            pattern,
            all_rows,
            result_iter,
            opened,
            memory_tracker,
        } => {
            if !*opened {
                return Err(QueryError::execution("PatternApply not opened".to_string()));
            }

            if result_iter.is_none() {
                while let Some(chunk) = input.next()? {
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    let col_names = chunk.col_names();
                    for row in chunk.rows {
                        let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
                        match ExpressionEvaluator::evaluate(pattern, &mut ctx) {
                            Ok(val) => {
                                let mut new_row = row.clone();
                                new_row.push(val);
                                all_rows.push(new_row);
                            }
                            Err(_) => {
                                all_rows.push(row);
                            }
                        }
                    }
                }
                *result_iter = Some(all_rows.drain(..).collect::<Vec<_>>().into_iter());
            }

            if let Some(iter) = result_iter {
                if let Some(row) = iter.next() {
                    return Ok(Some(DataChunk::from_rows(vec![row])));
                }
            }

            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_patternapply".to_string(),
        )),
    }
}

pub fn stop_patternapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PatternApply { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_patternapply".to_string(),
        )),
    }
}

pub fn close_patternapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PatternApply {
            input,
            all_rows,
            result_iter,
            ..
        } => {
            input.close()?;
            all_rows.clear();
            *result_iter = None;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_patternapply".to_string(),
        )),
    }
}

// ============ RollUpApply ============

pub fn open_rolluapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RollUpApply { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_rolluapply".to_string(),
        )),
    }
}

pub fn next_rolluapply(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::RollUpApply {
            input,
            rollup_expressions,
            all_rows,
            result_iter,
            opened,
            memory_tracker,
        } => {
            if !*opened {
                return Err(QueryError::execution("RollUpApply not opened".to_string()));
            }

            if result_iter.is_none() {
                let mut col_names: Vec<String> = Vec::new();
                while let Some(chunk) = input.next()? {
                    if col_names.is_empty() {
                        col_names = chunk.col_names();
                    }
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    for row in chunk.rows {
                        let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
                        let mut aggregated = row.clone();
                        for expr in rollup_expressions.iter() {
                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                Ok(val) => aggregated.push(val),
                                Err(_) => aggregated.push(Value::Null(crate::core::NullType::Null)),
                            }
                        }
                        all_rows.push(aggregated);
                    }
                }
                *result_iter = Some(all_rows.drain(..).collect::<Vec<_>>().into_iter());
            }

            if let Some(iter) = result_iter {
                if let Some(row) = iter.next() {
                    return Ok(Some(DataChunk::from_rows(vec![row])));
                }
            }

            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_rolluapply".to_string(),
        )),
    }
}

pub fn stop_rolluapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RollUpApply { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_rolluapply".to_string(),
        )),
    }
}

pub fn close_rolluapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RollUpApply {
            input,
            all_rows,
            result_iter,
            ..
        } => {
            input.close()?;
            all_rows.clear();
            *result_iter = None;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_rolluapply".to_string(),
        )),
    }
}

// ============ Minus ============

pub fn open_minus(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Minus { left, opened, .. } => {
            left.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_minus".to_string(),
        )),
    }
}

pub fn next_minus(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Minus {
            left,
            right,
            exclude_rows,
            right_buffered,
            opened,
            memory_tracker,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("Minus not opened".to_string()));
            }

            // Buffer right side on first call
            if !*right_buffered {
                right.open()?;
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        let row_str = format!("{:?}", row);
                        memory_tracker.try_reserve(row_str.len())?;
                        exclude_rows.insert(row_str);
                    }
                }
                right.close()?;
                *right_buffered = true;
            }

            // Return left rows not in right
            while let Some(chunk) = left.next()? {
                let mut result_rows = vec![];
                for row in chunk.rows {
                    let row_str = format!("{:?}", row);
                    if !exclude_rows.contains(&row_str) {
                        result_rows.push(row);
                    }
                }

                if !result_rows.is_empty() {
                    return Ok(Some(DataChunk::from_rows(result_rows)));
                }
            }

            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_minus".to_string(),
        )),
    }
}

pub fn stop_minus(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Minus { left, .. } => {
            left.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_minus".to_string(),
        )),
    }
}

pub fn close_minus(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Minus {
            left,
            exclude_rows,
            memory_tracker,
            ..
        } => {
            let mem = exclude_rows.len() * 256;
            memory_tracker.release(mem);
            exclude_rows.clear();
            left.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_minus".to_string(),
        )),
    }
}

// ============ Window ============

pub fn open_window(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Window { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in open_window".to_string(),
        )),
    }
}

pub fn next_window(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Window {
            input,
            window_exprs,
            partition_by_exprs,
            order_by_exprs,
            order_by_directions,
            all_rows,
            result_iter,
            opened,
            memory_tracker,
        } => {
            if !*opened {
                return Err(QueryError::execution("Window not opened".to_string()));
            }

            if result_iter.is_none() {
                let mut col_names: Vec<String> = Vec::new();
                while let Some(chunk) = input.next()? {
                    if col_names.is_empty() {
                        col_names = chunk.col_names();
                    }
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    all_rows.extend(chunk.rows);
                }

                if !all_rows.is_empty() && col_names.is_empty() {
                    col_names = (0..all_rows[0].len())
                        .map(|i| format!("col_{}", i))
                        .collect();
                }

                // Partition rows
                let mut partitions: Vec<Vec<Vec<Value>>> = Vec::new();
                if partition_by_exprs.is_empty() {
                    partitions.push(all_rows.drain(..).collect());
                } else {
                    let mut partition_map: std::collections::HashMap<String, Vec<Vec<Value>>> =
                        std::collections::HashMap::new();
                    for row in all_rows.drain(..) {
                        let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
                        let mut key_parts = Vec::new();
                        for expr in partition_by_exprs.iter() {
                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                Ok(val) => key_parts.push(format!("{:?}", val)),
                                Err(_) => key_parts.push("null".to_string()),
                            }
                        }
                        let key = key_parts.join("|");
                        partition_map.entry(key).or_default().push(row);
                    }
                    partitions = partition_map.into_values().collect();
                }

                // Sort each partition by order_by_exprs
                for partition in &mut partitions {
                    if !order_by_exprs.is_empty() {
                        partition.sort_by(|a, b| {
                            for (idx, expr) in order_by_exprs.iter().enumerate() {
                                let direction = order_by_directions
                                    .get(idx)
                                    .copied()
                                    .unwrap_or(super::super::SortDirection::Ascending);
                                let mut ctx_a = ValueRowContext::new(a.clone(), col_names.clone());
                                let mut ctx_b = ValueRowContext::new(b.clone(), col_names.clone());
                                let val_a = ExpressionEvaluator::evaluate(expr, &mut ctx_a)
                                    .unwrap_or(Value::Null(crate::core::NullType::Null));
                                let val_b = ExpressionEvaluator::evaluate(expr, &mut ctx_b)
                                    .unwrap_or(Value::Null(crate::core::NullType::Null));
                                let cmp = compare_values(&val_a, &val_b);
                                let final_cmp = match direction {
                                    super::super::SortDirection::Ascending => cmp,
                                    super::super::SortDirection::Descending => cmp.reverse(),
                                };
                                if final_cmp != std::cmp::Ordering::Equal {
                                    return final_cmp;
                                }
                            }
                            std::cmp::Ordering::Equal
                        });
                    }
                }

                // Evaluate window expressions for each row
                let mut out_rows = Vec::new();
                for partition in &partitions {
                    for row in partition {
                        let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
                        let mut new_row = row.clone();
                        for expr in window_exprs.iter() {
                            match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                Ok(val) => new_row.push(val),
                                Err(_) => new_row.push(Value::Null(crate::core::NullType::Null)),
                            }
                        }
                        out_rows.push(new_row);
                    }
                }

                *result_iter = Some(out_rows.into_iter());
            }

            if let Some(iter) = result_iter {
                if let Some(row) = iter.next() {
                    return Ok(Some(DataChunk::from_rows(vec![row])));
                }
            }

            Ok(None)
        }
        _ => Err(QueryError::execution(
            "Type mismatch in next_window".to_string(),
        )),
    }
}

pub fn stop_window(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Window { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in stop_window".to_string(),
        )),
    }
}

pub fn close_window(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Window {
            input,
            all_rows,
            result_iter,
            memory_tracker,
            ..
        } => {
            let mem = MemoryBudget::estimate_rows_memory(all_rows);
            memory_tracker.release(mem);
            all_rows.clear();
            *result_iter = None;
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution(
            "Type mismatch in close_window".to_string(),
        )),
    }
}

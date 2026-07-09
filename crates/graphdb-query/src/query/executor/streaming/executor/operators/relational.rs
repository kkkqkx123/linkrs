//! Simple relational operator implementations
//!
//! Includes: TopN, Dedup, Assign, Materialize, Remove, DataCollect, Unwind,
//! Apply, PatternApply, RollUpApply, Minus, Window

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use super::super::{StreamingExecutor, ValueRowContext};

// ============ TopN ============

pub fn open_topn(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TopN { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_topn".to_string())),
    }
}

pub fn next_topn(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TopN {
            input,
            n,
            sort_expressions: _sort_expressions,
            sort_directions: _sort_directions,
            all_rows,
            result_iter,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("TopN not opened".to_string()));
            }

            // Collect all rows on first call
            if result_iter.is_none() {
                while let Some(chunk) = input.next()? {
                    all_rows.extend(chunk.rows);
                }

                // Keep only top N (simplified: just take first N rows)
                all_rows.truncate(*n as usize);
                *result_iter = Some(all_rows.drain(..).collect::<Vec<_>>().into_iter());
            }

            // Return next chunk
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
        _ => Err(QueryError::execution("Type mismatch in next_topn".to_string())),
    }
}

pub fn stop_topn(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TopN { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_topn".to_string())),
    }
}

pub fn close_topn(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TopN {
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
        _ => Err(QueryError::execution("Type mismatch in close_topn".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_dedup".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in next_dedup".to_string())),
    }
}

pub fn stop_dedup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Dedup { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_dedup".to_string())),
    }
}

pub fn close_dedup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Dedup {
            input,
            seen_rows,
            ..
        } => {
            input.close()?;
            seen_rows.clear();
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_dedup".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_assign".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in next_assign".to_string())),
    }
}

pub fn stop_assign(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Assign { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_assign".to_string())),
    }
}

pub fn close_assign(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Assign { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_assign".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_materialize".to_string())),
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
        } => {
            if !*opened {
                return Err(QueryError::execution("Materialize not opened".to_string()));
            }

            // Materialize all rows on first call
            if !*materialized {
                while let Some(chunk) = input.next()? {
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
        _ => Err(QueryError::execution("Type mismatch in next_materialize".to_string())),
    }
}

pub fn stop_materialize(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Materialize { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_materialize".to_string())),
    }
}

pub fn close_materialize(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Materialize {
            input,
            materialized_rows,
            result_iter,
            ..
        } => {
            input.close()?;
            materialized_rows.clear();
            *result_iter = None;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_materialize".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_remove".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in next_remove".to_string())),
    }
}

pub fn stop_remove(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Remove { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_remove".to_string())),
    }
}

pub fn close_remove(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Remove { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_remove".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_datacollect".to_string())),
    }
}

pub fn next_datacollect(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::DataCollect {
            input,
            all_rows,
            emitted,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("DataCollect not opened".to_string()));
            }

            if *emitted {
                return Ok(None);
            }

            // Collect all rows and emit as single chunk
            while let Some(chunk) = input.next()? {
                all_rows.extend(chunk.rows);
            }

            if !all_rows.is_empty() {
                *emitted = true;
                let rows = all_rows.drain(..).collect::<Vec<_>>();
                return Ok(Some(DataChunk::from_rows(rows)));
            }

            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_datacollect".to_string())),
    }
}

pub fn stop_datacollect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DataCollect { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_datacollect".to_string())),
    }
}

pub fn close_datacollect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DataCollect {
            input,
            all_rows,
            ..
        } => {
            input.close()?;
            all_rows.clear();
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_datacollect".to_string())),
    }
}

// ============ Unwind ============

pub fn open_unwind(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Unwind { input, opened, col_index, .. } => {
            input.open()?;
            *opened = true;
            *col_index = None;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_unwind".to_string())),
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
                                while *current_unwind_index < elements.len() && result_rows.len() < CHUNK_SIZE {
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
        _ => Err(QueryError::execution("Type mismatch in next_unwind".to_string())),
    }
}

pub fn stop_unwind(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Unwind { input, col_index, .. } => {
            input.stop()?;
            *col_index = None;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_unwind".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_unwind".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_apply".to_string())),
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
                let mut result_rows = vec![];
                for row in chunk.rows {
                    let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                    // Evaluate expression for each row
                    match ExpressionEvaluator::evaluate(apply_expression, &mut context) {
                        Ok(_val) => result_rows.push(row),
                        Err(_) => {}, // Skip rows where expression fails
                    }
                }

                if !result_rows.is_empty() {
                    return Ok(Some(DataChunk::from_rows(result_rows)));
                }
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_apply".to_string())),
    }
}

pub fn stop_apply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Apply { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_apply".to_string())),
    }
}

pub fn close_apply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Apply { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_apply".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_patternapply".to_string())),
    }
}

pub fn next_patternapply(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::PatternApply {
            input,
            pattern: _pattern,
            all_rows,
            result_iter,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("PatternApply not opened".to_string()));
            }

            // Placeholder implementation
            if result_iter.is_none() {
                while let Some(chunk) = input.next()? {
                    all_rows.extend(chunk.rows);
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
        _ => Err(QueryError::execution("Type mismatch in next_patternapply".to_string())),
    }
}

pub fn stop_patternapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PatternApply { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_patternapply".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_patternapply".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_rolluapply".to_string())),
    }
}

pub fn next_rolluapply(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::RollUpApply {
            input,
            rollup_expressions: _rollup_expressions,
            all_rows,
            result_iter,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("RollUpApply not opened".to_string()));
            }

            // Placeholder implementation
            if result_iter.is_none() {
                while let Some(chunk) = input.next()? {
                    all_rows.extend(chunk.rows);
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
        _ => Err(QueryError::execution("Type mismatch in next_rolluapply".to_string())),
    }
}

pub fn stop_rolluapply(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RollUpApply { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_rolluapply".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in close_rolluapply".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_minus".to_string())),
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
        } => {
            if !*opened {
                return Err(QueryError::execution("Minus not opened".to_string()));
            }

            // Buffer right side on first call
            if !*right_buffered {
                right.open()?;
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        exclude_rows.insert(format!("{:?}", row));
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
        _ => Err(QueryError::execution("Type mismatch in next_minus".to_string())),
    }
}

pub fn stop_minus(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Minus { left, .. } => {
            left.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_minus".to_string())),
    }
}

pub fn close_minus(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Minus {
            left,
            exclude_rows,
            ..
        } => {
            left.close()?;
            exclude_rows.clear();
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_minus".to_string())),
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
        _ => Err(QueryError::execution("Type mismatch in open_window".to_string())),
    }
}

pub fn next_window(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Window {
            input,
            window_exprs: _window_exprs,
            partition_by_exprs: _partition_by_exprs,
            order_by_exprs: _order_by_exprs,
            order_by_directions: _order_by_directions,
            all_rows,
            result_iter,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("Window not opened".to_string()));
            }

            // Placeholder: collect all rows and emit in order
            if result_iter.is_none() {
                while let Some(chunk) = input.next()? {
                    all_rows.extend(chunk.rows);
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
        _ => Err(QueryError::execution("Type mismatch in next_window".to_string())),
    }
}

pub fn stop_window(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Window { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_window".to_string())),
    }
}

pub fn close_window(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Window {
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
        _ => Err(QueryError::execution("Type mismatch in close_window".to_string())),
    }
}

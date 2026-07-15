use std::collections::HashMap;
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
use crate::query::executor::streaming::slot::{combine_layouts, SlotLayout};

use super::{build_combined_names, close_common, evaluate_join_key};

pub(super) fn next_hash_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side_hash: &mut HashMap<Vec<Value>, Vec<Vec<Value>>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !*left_consumed {
        while let Some(chunk) = right.advance()? {
            base.ensure_not_cancelled()?;
            let col_names = chunk.col_names();
            if right_col_names.is_empty() {
                *right_col_names = col_names.clone();
            }
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                let hash_key = evaluate_join_key(&row, &col_names, hash_keys)?;
                build_side_hash
                    .entry(hash_key)
                    .or_default()
                    .push(row.clone());
                all_right_rows.push(row);
            }
        }
        *left_consumed = true;
    }

    if let Some(left_chunk) = left.advance()? {
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        for left_row in &left_chunk.rows {
            let probe_key = evaluate_join_key(left_row, &left_col_names, probe_keys)?;
            let matching_right_rows = build_side_hash.get(&probe_key);

            if let Some(right_rows) = matching_right_rows {
                for right_row in right_rows {
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
        }

        if result_rows.is_empty() {
            Ok(None)
        } else {
            let left_layout = left_chunk.get_layout();
            let right_layout = Arc::new(SlotLayout::from_names(&build_combined_names(
                &[],
                right_col_names,
                all_right_rows.first().map(|r| r.len()).unwrap_or(0),
            )));
            let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
            Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn next_hash_left_join(
    join_condition: &mut Option<Expression>,
    hash_keys: &mut [Expression],
    probe_keys: &mut [Expression],
    build_side_hash: &mut HashMap<Vec<Value>, Vec<Vec<Value>>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    base: &mut OperatorBase,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !*left_consumed {
        while let Some(chunk) = right.advance()? {
            base.ensure_not_cancelled()?;
            let col_names = chunk.col_names();
            if right_col_names.is_empty() {
                *right_col_names = col_names.clone();
            }
            for row in chunk.rows {
                memory_tracker.try_reserve_row(&row)?;
                let hash_key = evaluate_join_key(&row, &col_names, hash_keys)?;
                build_side_hash
                    .entry(hash_key)
                    .or_default()
                    .push(row.clone());
                all_right_rows.push(row);
            }
        }
        *left_consumed = true;
    }

    if let Some(left_chunk) = left.advance()? {
        let left_col_names = left_chunk.col_names();
        let mut result_rows = Vec::new();

        for left_row in &left_chunk.rows {
            let probe_key = evaluate_join_key(left_row, &left_col_names, probe_keys)?;
            let matching_right_rows = build_side_hash.get(&probe_key);

            if let Some(right_rows) = matching_right_rows {
                for right_row in right_rows {
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
                                    "HashLeftJoin condition evaluation failed: {}",
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
            } else {
                let mut unmatched_row = left_row.clone();
                for _ in 0..all_right_rows.first().map(|r| r.len()).unwrap_or(0) {
                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                }
                result_rows.push(unmatched_row);
            }
        }

        if result_rows.is_empty() {
            Ok(None)
        } else {
            let left_layout = left_chunk.get_layout();
            let right_layout = if right_col_names.is_empty() {
                Arc::new(SlotLayout::from_names(
                    &all_right_rows
                        .first()
                        .map(|r| {
                            (0..r.len())
                                .map(|i| format!("right_{}", i))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
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

pub(super) fn close(
    lifecycle: &mut OperatorLifecycle,
    memory_tracker: &mut MemoryTracker,
    build_side_hash: &mut HashMap<Vec<Value>, Vec<Vec<Value>>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
) -> Result<(), QueryError> {
    close_common(
        lifecycle,
        memory_tracker,
        || {
            build_side_hash.clear();
            all_right_rows.clear();
        },
        left,
        right,
    )
}

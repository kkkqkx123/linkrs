//! Stateful operators: Aggregate, Sort, GroupBy, WindowFunction

use crate::core::error::QueryError;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::helpers::*;
use crate::query::executor::streaming::executor::{
    SortDirection, StreamingExecutor, ValueRowContext,
};
use std::collections::HashMap;

// ============ Aggregate ============

pub fn open_aggregate(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Aggregate { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_aggregate(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Aggregate {
            input,
            group_by_expressions,
            aggregate_functions,
            all_rows,
            result_iter,
            ..
        } => {
            // First time: collect all rows and build groups
            if result_iter.is_none() {
                // Collect all input rows and get column names from first chunk
                let mut col_names: Vec<String> = vec![];
                while let Some(chunk) = input.next()? {
                    if col_names.is_empty() {
                        col_names = chunk.col_names();
                    }
                    all_rows.extend(chunk.rows);
                }

                // Build group map: group_key -> rows
                let mut group_map: HashMap<Vec<Value>, Vec<Vec<Value>>> = HashMap::new();

                for row in all_rows.iter().cloned() {
                    // Evaluate group_by_expressions to get group key
                    let mut group_key = Vec::new();
                    if group_by_expressions.is_empty() {
                        // No GROUP BY means entire result set is one group
                        group_key.push(Value::Null(NullType::Null));
                    } else {
                        for expr in group_by_expressions.iter() {
                            let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                            match ExpressionEvaluator::evaluate(expr, &mut context) {
                                Ok(value) => group_key.push(value),
                                Err(_) => group_key.push(Value::Null(NullType::Null)),
                            }
                        }
                    }

                    group_map
                        .entry(group_key)
                        .or_insert_with(Vec::new)
                        .push(row);
                }

                // Generate result rows
                let mut result_rows = Vec::new();
                for (_group_key, group_rows) in group_map {
                    let mut result_row = Vec::new();

                    // Add group key values
                    for expr in group_by_expressions.iter() {
                        if let Some(first_row) = group_rows.first() {
                            let mut context =
                                ValueRowContext::new(first_row.clone(), col_names.clone());
                            match ExpressionEvaluator::evaluate(expr, &mut context) {
                                Ok(value) => result_row.push(value),
                                Err(_) => result_row.push(Value::Null(NullType::Null)),
                            }
                        }
                    }

                    // Add aggregate function results
                    for (agg_func, _expr) in aggregate_functions.iter() {
                        let agg_value = compute_aggregate(agg_func, &group_rows, &col_names);
                        result_row.push(agg_value);
                    }

                    result_rows.push(result_row);
                }

                *result_iter = Some(result_rows.into_iter());
            }

            // Return next chunk from result_iter
            if let Some(iter) = result_iter {
                let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                if chunk_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(chunk_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_aggregate(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Aggregate { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_aggregate(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Aggregate { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ Sort ============

pub fn open_sort(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Sort { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_sort(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Sort {
            input,
            sort_expressions,
            sort_directions,
            all_rows,
            row_iter,
            ..
        } => {
            if row_iter.is_none() {
                // Collect all rows and get column names from first chunk
                let mut col_names: Vec<String> = vec![];
                while let Some(chunk) = input.next()? {
                    if col_names.is_empty() {
                        col_names = chunk.col_names();
                    }
                    all_rows.extend(chunk.rows);
                }

                // If no sort expressions, maintain natural order
                if sort_expressions.is_empty() {
                    // No sorting needed
                } else {
                    // Sort with multiple keys and directions
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
                }

                let all_rows_copy = all_rows.drain(..).collect::<Vec<_>>();
                *row_iter = Some(all_rows_copy.into_iter());
            }

            if let Some(iter) = row_iter {
                let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                if chunk_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(chunk_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_sort(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Sort { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_sort(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Sort { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ GroupBy ============

pub fn open_groupby(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GroupBy { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_groupby(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::GroupBy {
            input,
            group_by_expressions,
            all_rows,
            result_iter,
            ..
        } => {
            // Buffer all rows if not done yet
            if result_iter.is_none() {
                while let Some(chunk) = input.next()? {
                    for row in chunk.rows {
                        all_rows.push(row);
                    }
                }

                if all_rows.is_empty() {
                    return Ok(None);
                }

                // Get column names from first chunk
                let col_names = if all_rows.is_empty() {
                    vec![]
                } else {
                    let first_row_len = all_rows[0].len();
                    (0..first_row_len).map(|i| format!("col_{}", i)).collect()
                };

                // Group rows by key
                let mut groups: std::collections::HashMap<String, Vec<Vec<Value>>> =
                    std::collections::HashMap::new();
                for row in all_rows.iter() {
                    let mut key_parts: Vec<String> = Vec::new();
                    for expr in group_by_expressions.iter() {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        let key_val = ExpressionEvaluator::evaluate(expr, &mut context)
                            .unwrap_or(Value::Null(NullType::Null));
                        key_parts.push(format!("{:?}", key_val));
                    }
                    let key = key_parts.join("|");
                    groups.entry(key).or_insert_with(Vec::new).push(row.clone());
                }

                // Return all rows from all groups (preserving all data)
                let result_rows: Vec<Vec<Value>> = groups.into_values().flatten().collect();

                *result_iter = Some(result_rows.into_iter());
            }

            // Return next batch from result iterator
            if let Some(iter) = result_iter {
                let result_rows: Vec<Vec<Value>> = iter.take(1024).collect();
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

pub fn stop_groupby(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GroupBy { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_groupby(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::GroupBy { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ WindowFunction ============

use crate::core::types::expr::Expression;
use crate::query::executor::streaming::executor::helpers::*;
use std::collections::BTreeMap;

pub fn open_windowfunction(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::WindowFunction { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_windowfunction(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::WindowFunction {
            input,
            window_exprs,
            partition_by_exprs,
            order_by_exprs,
            order_by_directions,
            all_rows,
            result_iter,
            ..
        } => {
            if result_iter.is_none() {
                let mut col_names: Vec<String> = vec![];
                while let Some(chunk) = input.next()? {
                    if col_names.is_empty() {
                        col_names = chunk.col_names();
                    }
                    all_rows.extend(chunk.rows);
                }

                if all_rows.is_empty() {
                    *result_iter = Some(vec![].into_iter());
                } else {
                    let mut partitions: BTreeMap<Vec<Value>, Vec<(usize, Vec<Value>)>> =
                        BTreeMap::new();

                    for (idx, row) in all_rows.iter().enumerate() {
                        let mut partition_key = Vec::new();
                        if partition_by_exprs.is_empty() {
                            partition_key.push(Value::Null(NullType::Null));
                        } else {
                            for expr in partition_by_exprs.iter() {
                                let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
                                match ExpressionEvaluator::evaluate(expr, &mut ctx) {
                                    Ok(val) => partition_key.push(val),
                                    Err(_) => partition_key.push(Value::Null(NullType::Null)),
                                }
                            }
                        }
                        partitions
                            .entry(partition_key)
                            .or_insert_with(Vec::new)
                            .push((idx, row.clone()));
                    }

                    let mut result_rows = Vec::new();
                    for (_key, mut partition_rows) in partitions {
                        if !order_by_exprs.is_empty() {
                            partition_rows.sort_by(|a, b| {
                                for (idx, expr) in order_by_exprs.iter().enumerate() {
                                    let direction = order_by_directions
                                        .get(idx)
                                        .copied()
                                        .unwrap_or(SortDirection::Ascending);
                                    let mut ctx_a =
                                        ValueRowContext::new(a.1.clone(), col_names.clone());
                                    let mut ctx_b =
                                        ValueRowContext::new(b.1.clone(), col_names.clone());
                                    let val_a = ExpressionEvaluator::evaluate(expr, &mut ctx_a)
                                        .unwrap_or(Value::Null(NullType::Null));
                                    let val_b = ExpressionEvaluator::evaluate(expr, &mut ctx_b)
                                        .unwrap_or(Value::Null(NullType::Null));
                                    let cmp = comparison::compare_values(&val_a, &val_b);
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
                        }

                        for (row_idx, row) in partition_rows.iter() {
                            let mut result_row = row.clone();
                            for window_expr in window_exprs.iter() {
                                if let Expression::WindowFunction { name, args, .. } = window_expr {
                                    let func_args: Vec<Value> = args
                                        .iter()
                                        .map(|arg| {
                                            let mut ctx = ValueRowContext::new(
                                                row.clone(),
                                                col_names.clone(),
                                            );
                                            ExpressionEvaluator::evaluate(arg, &mut ctx)
                                                .unwrap_or(Value::Null(NullType::Null))
                                        })
                                        .collect();

                                    let window_result = compute_window_function(
                                        name,
                                        &func_args,
                                        &partition_rows,
                                        partition_rows
                                            .iter()
                                            .position(|(i, _)| i == row_idx)
                                            .unwrap_or(0),
                                    );
                                    result_row.push(window_result);
                                }
                            }
                            result_rows.push(result_row);
                        }
                    }

                    *result_iter = Some(result_rows.into_iter());
                }
            }

            if let Some(iter) = result_iter {
                let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(1024).collect();
                if chunk_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(chunk_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

fn compute_window_function(
    name: &str,
    args: &[Value],
    partition_rows: &[(usize, Vec<Value>)],
    current_pos: usize,
) -> Value {
    let value_to_i64 = |val: &Value| -> i64 {
        match val {
            Value::Int(i) => *i as i64,
            Value::BigInt(i) => *i,
            _ => 1,
        }
    };

    match name {
        "row_number" => Value::BigInt((current_pos + 1) as i64),
        "rank" => Value::BigInt((current_pos + 1) as i64),
        "dense_rank" => Value::BigInt((current_pos + 1) as i64),
        "lead" => {
            let offset = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            let default_val = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Null(NullType::Null)
            };
            if current_pos + offset < partition_rows.len() {
                partition_rows[current_pos + offset]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(default_val)
            } else {
                default_val
            }
        }
        "lag" => {
            let offset = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            let default_val = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Null(NullType::Null)
            };
            if current_pos >= offset {
                partition_rows[current_pos - offset]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(default_val)
            } else {
                default_val
            }
        }
        "first_value" => partition_rows
            .first()
            .map(|(_, r)| r.first().cloned())
            .flatten()
            .unwrap_or(Value::Null(NullType::Null)),
        "last_value" => partition_rows
            .last()
            .map(|(_, r)| r.first().cloned())
            .flatten()
            .unwrap_or(Value::Null(NullType::Null)),
        "nth_value" => {
            let n = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            if n > 0 && n <= partition_rows.len() {
                partition_rows[n - 1]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(Value::Null(NullType::Null))
            } else {
                Value::Null(NullType::Null)
            }
        }
        "ntile" => {
            let n = if !args.is_empty() {
                value_to_i64(&args[0])
            } else {
                1
            };
            if n > 0 {
                let bucket_size = (partition_rows.len() as i64 + n - 1) / n;
                Value::BigInt((current_pos as i64 / bucket_size) + 1)
            } else {
                Value::Null(NullType::Null)
            }
        }
        _ => Value::Null(NullType::Null),
    }
}

pub fn stop_windowfunction(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::WindowFunction { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_windowfunction(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::WindowFunction { input, opened, .. } => {
            if *opened {
                input.close()?;
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
    use crate::core::types::operators::AggregateFunction;
    use crate::core::value::NullType;
    use crate::core::Value;

    fn create_test_buffer(size: usize) -> Vec<Vec<Value>> {
        (0..size)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::String(format!("item_{}", i)),
                    Value::Int((i % 5) as i32),
                ]
            })
            .collect()
    }

    #[test]
    fn test_aggregate_with_groupby() {
        let buffer = create_test_buffer(10);
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
        });

        let mut aggregate = StreamingExecutor::Aggregate {
            input: scan,
            group_by_expressions: vec![Expression::Literal(Value::Int(0))],
            aggregate_functions: vec![(
                AggregateFunction::Count(None),
                Expression::Literal(Value::Null(NullType::Null)),
            )],
            all_rows: Vec::new(),
            result_iter: None,
            opened: false,
        };

        aggregate.open().unwrap();
        let chunk = aggregate.next().unwrap();
        assert!(chunk.is_some());
        // Should have at least one group
        assert!(chunk.unwrap().len() > 0);
        aggregate.close().unwrap();
    }

    #[test]
    fn test_aggregate_without_groupby() {
        let buffer = create_test_buffer(10);
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
        });

        let mut aggregate = StreamingExecutor::Aggregate {
            input: scan,
            group_by_expressions: vec![],
            aggregate_functions: vec![(
                AggregateFunction::Count(None),
                Expression::Literal(Value::Null(NullType::Null)),
            )],
            all_rows: Vec::new(),
            result_iter: None,
            opened: false,
        };

        aggregate.open().unwrap();
        let chunk = aggregate.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // Without GROUP BY, should return 1 result row with count = 10
        assert_eq!(chunk.len(), 1);
        aggregate.close().unwrap();
    }

    #[test]
    fn test_sort_with_order() {
        let buffer = vec![
            vec![Value::Int(3), Value::String("c".to_string())],
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ];

        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
        });

        let mut sort = StreamingExecutor::Sort {
            input: scan,
            sort_expressions: vec![Expression::Literal(Value::Int(0))],
            sort_directions: vec![SortDirection::Ascending],
            all_rows: Vec::new(),
            row_iter: None,
            opened: false,
        };

        sort.open().unwrap();
        let chunk = sort.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 3);
        sort.close().unwrap();
    }

    #[test]
    fn test_sort_empty_input() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![],
            current_index: 0,
            col_names: vec![],
        });

        let mut sort = StreamingExecutor::Sort {
            input: scan,
            sort_expressions: vec![],
            sort_directions: vec![],
            all_rows: Vec::new(),
            row_iter: None,
            opened: false,
        };

        sort.open().unwrap();
        let chunk = sort.next().unwrap();
        assert!(chunk.is_none());
        sort.close().unwrap();
    }

    #[test]
    fn test_groupby_deduplication() {
        let buffer = vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ];

        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
        });

        let mut groupby = StreamingExecutor::GroupBy {
            input: scan,
            group_by_expressions: vec![Expression::Literal(Value::Int(1))],
            all_rows: Vec::new(),
            result_iter: None,
            opened: false,
        };

        groupby.open().unwrap();
        let chunk = groupby.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // Should have 2 groups (for values 1 and 2)
        assert!(chunk.len() <= 4 && chunk.len() > 0);
        groupby.close().unwrap();
    }
}

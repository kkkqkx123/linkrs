use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::executor::{SortDirection, ValueRowContext};
use crate::executor::streaming::helpers::compare_values;
use crate::executor::streaming::spill::{HashPartitionSpiller, SpilledFile, SpilledRun};
use graphdb_core::types::expr::Expression;
use graphdb_core::value::NullType;
use graphdb_core::Value;

/// Sort the rows of a single window partition by the ORDER BY expressions.
///
/// Order keys are precomputed once per row (previously re-evaluated on
/// every comparison); the sort is stable so ties keep the input order.
pub(crate) fn sort_partition_rows(
    partition_rows: &mut [(usize, Vec<Value>)],
    col_names: &[String],
    order_by_exprs: &[Expression],
    order_by_directions: &[SortDirection],
) {
    if order_by_exprs.is_empty() {
        return;
    }
    let keys: Vec<Vec<Value>> = partition_rows
        .iter()
        .map(|(_, row)| {
            order_by_exprs
                .iter()
                .map(|expr| {
                    let mut ctx = ValueRowContext::from_names(row.clone(), col_names.to_vec());
                    ExpressionEvaluator::evaluate(expr, &mut ctx)
                        .unwrap_or(Value::Null(NullType::Null))
                })
                .collect()
        })
        .collect();
    let mut order: Vec<usize> = (0..partition_rows.len()).collect();
    order.sort_by(|&i, &j| {
        for (idx, _) in order_by_exprs.iter().enumerate() {
            let direction = order_by_directions
                .get(idx)
                .copied()
                .unwrap_or(SortDirection::Ascending);
            let cmp = compare_values(&keys[i][idx], &keys[j][idx]);
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
    let mut sorted = Vec::with_capacity(partition_rows.len());
    for &i in &order {
        sorted.push(std::mem::take(&mut partition_rows[i]));
    }
    partition_rows.clone_from_slice(&sorted);
}

#[derive(Debug)]
pub struct WindowFunctionState {
    pub all_rows: Vec<Vec<Value>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
    pub partition_spiller: Option<HashPartitionSpiller>,
    pub spilled_runs: Vec<Option<SpilledRun>>,
    pub current_partition: usize,
    pub has_spilled: bool,
    /// True once all partitions have been emitted.
    pub output_complete: bool,
}

#[derive(Debug)]
pub struct WindowState {
    pub all_rows: Vec<Vec<Value>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
    pub partition_spiller: Option<HashPartitionSpiller>,
    pub spilled_runs: Vec<Option<SpilledRun>>,
    pub current_partition: usize,
    pub has_spilled: bool,
    /// True once all partitions have been emitted.
    pub output_complete: bool,
}

/// Compute the result rows for one window partition.
///
/// The partition rows must already be ordered as desired (partition-order
/// sort happens at the caller). Positional window values are computed per
/// `window_exprs` and appended to each source row.
pub(crate) fn compute_window_partition_result(
    partition_rows: &[(usize, Vec<Value>)],
    col_names: &[String],
    window_exprs: &[Expression],
) -> Vec<Vec<Value>> {
    let mut result_rows = Vec::with_capacity(partition_rows.len());
    for (pos, (_, row)) in partition_rows.iter().enumerate() {
        let mut result_row = row.clone();
        for window_expr in window_exprs.iter() {
            if let Expression::WindowFunction { name, args, .. } = window_expr {
                let func_args: Vec<Value> = args
                    .iter()
                    .map(|arg| {
                        let mut ctx = ValueRowContext::from_names(row.clone(), col_names.to_vec());
                        ExpressionEvaluator::evaluate(arg, &mut ctx)
                            .unwrap_or(Value::Null(NullType::Null))
                    })
                    .collect();
                let window_result = compute_window_function(name, &func_args, partition_rows, pos);
                result_row.push(window_result);
            }
        }
        result_rows.push(result_row);
    }
    result_rows
}

pub(crate) fn compute_window_function(
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
            .and_then(|(_, r)| r.first().cloned())
            .unwrap_or(Value::Null(NullType::Null)),
        "last_value" => partition_rows
            .last()
            .and_then(|(_, r)| r.first().cloned())
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

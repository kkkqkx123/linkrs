//! Function call evaluation
//!
//! Provide the functionality to evaluate aggregate functions.

use crate::core::types::operators::AggregateFunction;
use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::{ExpressionError, ExpressionErrorType};

/// Function evaluator
pub struct FunctionEvaluator;

impl FunctionEvaluator {
    /// Evaluation aggregate functions
    pub fn eval_aggregate_function(
        func: &AggregateFunction,
        args: &[Value],
        distinct: bool,
    ) -> Result<Value, ExpressionError> {
        if args.is_empty() {
            return Err(ExpressionError::argument_count_error(1, 0));
        }

        match func {
            AggregateFunction::Count(_) => {
                if distinct {
                    let unique_values: std::collections::HashSet<_> = args.iter().collect();
                    Ok(Value::BigInt(unique_values.len() as i64))
                } else {
                    Ok(Value::BigInt(args.len() as i64))
                }
            }
            AggregateFunction::Sum(_) => {
                let mut sum = Value::Int(0);
                for arg in args {
                    sum = sum.add(arg).map_err(ExpressionError::runtime_error)?;
                }
                Ok(sum)
            }
            AggregateFunction::Avg(_) => {
                let sum = Self::eval_aggregate_function(
                    &AggregateFunction::Sum("".to_string()),
                    args,
                    distinct,
                )?;
                let count =
                    Self::eval_aggregate_function(&AggregateFunction::Count(None), args, distinct)?;
                sum.div(&count).map_err(ExpressionError::runtime_error)
            }
            AggregateFunction::Min(_) => {
                let mut min = args[0].clone();
                for arg in args.iter().skip(1) {
                    if arg < &min {
                        min = arg.clone();
                    }
                }
                Ok(min)
            }
            AggregateFunction::Max(_) => {
                let mut max = args[0].clone();
                for arg in args.iter().skip(1) {
                    if arg > &max {
                        max = arg.clone();
                    }
                }
                Ok(max)
            }
            AggregateFunction::Collect(_) => {
                if distinct {
                    let unique_values: std::collections::HashSet<_> =
                        args.iter().cloned().collect();
                    Ok(Value::list(List::from(
                        unique_values.into_iter().collect::<Vec<_>>(),
                    )))
                } else {
                    Ok(Value::list(List::from(args.to_vec())))
                }
            }
            AggregateFunction::CollectSet(_) => {
                let unique_values: std::collections::HashSet<_> = args.iter().cloned().collect();
                Ok(Value::set(unique_values))
            }
            AggregateFunction::Distinct(_) => {
                let unique_values: std::collections::HashSet<_> = args.iter().cloned().collect();
                Ok(Value::set(unique_values))
            }
            AggregateFunction::Percentile(_, _) => {
                if args.len() < 2 {
                    return Err(ExpressionError::argument_count_error(2, args.len()));
                }

                let percentile = match &args[1] {
                    Value::Int(v) => *v as f64,
                    Value::BigInt(v) => *v as f64,
                    Value::Float(v) => *v as f64,
                    Value::Double(v) => *v,
                    _ => return Err(ExpressionError::type_error("Percentile must be a number")),
                };

                if !(0.0..=100.0).contains(&percentile) {
                    return Err(ExpressionError::new(
                        ExpressionErrorType::InvalidOperation,
                        "Percentile must be between 0 and 100",
                    ));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let mut numeric_values = Vec::new();
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => numeric_values.push(*v as f64),
                        Value::Int(v) => numeric_values.push(*v as f64),
                        Value::BigInt(v) => numeric_values.push(*v as f64),
                        Value::Float(v) => numeric_values.push(*v as f64),
                        Value::Double(v) => numeric_values.push(*v),
                        _ => continue,
                    }
                }

                if numeric_values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                numeric_values
                    .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

                let index = (percentile / 100.0) * (numeric_values.len() - 1) as f64;
                let lower_index = index.floor() as usize;
                let upper_index = index.ceil() as usize;

                if lower_index == upper_index {
                    Ok(Value::Double(numeric_values[lower_index]))
                } else {
                    let lower_value = numeric_values[lower_index];
                    let upper_value = numeric_values[upper_index];
                    let weight = index - lower_index as f64;
                    let interpolated = lower_value + weight * (upper_value - lower_value);
                    Ok(Value::Double(interpolated))
                }
            }
            AggregateFunction::Std(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let mut numeric_values = Vec::new();
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => numeric_values.push(*v as f64),
                        Value::Int(v) => numeric_values.push(*v as f64),
                        Value::BigInt(v) => numeric_values.push(*v as f64),
                        Value::Float(v) => numeric_values.push(*v as f64),
                        Value::Double(v) => numeric_values.push(*v),
                        _ => continue,
                    }
                }

                if numeric_values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let n = numeric_values.len() as f64;
                let mean: f64 = numeric_values.iter().sum::<f64>() / n;
                let variance: f64 = numeric_values
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / n;
                let std_dev = variance.sqrt();

                Ok(Value::Double(std_dev))
            }
            AggregateFunction::BitAnd(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let mut result = i64::MAX;
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => result &= *v as i64,
                        Value::Int(v) => result &= *v as i64,
                        Value::BigInt(v) => result &= *v,
                        _ => {
                            return Err(ExpressionError::type_error(
                                "All values must be integers for BIT_AND",
                            ))
                        }
                    }
                }

                Ok(Value::BigInt(result))
            }
            AggregateFunction::BitOr(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let mut result = 0i64;
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => result |= *v as i64,
                        Value::Int(v) => result |= *v as i64,
                        Value::BigInt(v) => result |= *v,
                        _ => {
                            return Err(ExpressionError::type_error(
                                "All values must be integers for BIT_OR",
                            ))
                        }
                    }
                }

                Ok(Value::BigInt(result))
            }
            AggregateFunction::Variance(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let mut numeric_values = Vec::new();
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => numeric_values.push(*v as f64),
                        Value::Int(v) => numeric_values.push(*v as f64),
                        Value::BigInt(v) => numeric_values.push(*v as f64),
                        Value::Float(v) => numeric_values.push(*v as f64),
                        Value::Double(v) => numeric_values.push(*v),
                        _ => continue,
                    }
                }

                if numeric_values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let n = numeric_values.len() as f64;
                let mean: f64 = numeric_values.iter().sum::<f64>() / n;
                let variance: f64 = numeric_values
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / n;

                Ok(Value::Double(variance))
            }
            AggregateFunction::Median(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let mut numeric_values = Vec::new();
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => numeric_values.push(*v as f64),
                        Value::Int(v) => numeric_values.push(*v as f64),
                        Value::BigInt(v) => numeric_values.push(*v as f64),
                        Value::Float(v) => numeric_values.push(*v as f64),
                        Value::Double(v) => numeric_values.push(*v),
                        _ => continue,
                    }
                }

                if numeric_values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                numeric_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

                let len = numeric_values.len();
                let median = if len % 2 == 0 {
                    (numeric_values[len / 2 - 1] + numeric_values[len / 2]) / 2.0
                } else {
                    numeric_values[len / 2]
                };

                Ok(Value::Double(median))
            }
            AggregateFunction::Mode(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }

                let mut frequency_map = std::collections::HashMap::new();
                for value in values.iter() {
                    let key = format!("{}", value);
                    *frequency_map.entry(key).or_insert(0usize) += 1;
                }

                let mode = frequency_map
                    .into_iter()
                    .max_by_key(|(_, count)| *count)
                    .map(|(key, _)| key);

                match mode {
                    Some(mode_str) => {
                        if let Ok(int_val) = mode_str.parse::<i32>() {
                            Ok(Value::Int(int_val))
                        } else if let Ok(float_val) = mode_str.parse::<f64>() {
                            Ok(Value::Double(float_val))
                        } else if mode_str == "true" {
                            Ok(Value::Bool(true))
                        } else if mode_str == "false" {
                            Ok(Value::Bool(false))
                        } else {
                            Ok(Value::String(mode_str))
                        }
                    }
                    None => Ok(Value::Null(crate::core::NullType::NaN)),
                }
            }
            AggregateFunction::BoolAnd(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Bool(true));
                }

                let mut result = true;
                for value in values.iter() {
                    match value {
                        Value::Bool(b) => result &= *b,
                        Value::Null(_) => {}
                        _ => {
                            return Err(ExpressionError::type_error(
                                "All values must be boolean for BOOL_AND",
                            ))
                        }
                    }
                }

                Ok(Value::Bool(result))
            }
            AggregateFunction::BoolOr(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::Bool(false));
                }

                let mut result = false;
                for value in values.iter() {
                    match value {
                        Value::Bool(b) => result |= *b,
                        Value::Null(_) => {}
                        _ => {
                            return Err(ExpressionError::type_error(
                                "All values must be boolean for BOOL_OR",
                            ))
                        }
                    }
                }

                Ok(Value::Bool(result))
            }
            AggregateFunction::GroupConcat(_, separator) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::String(String::new()));
                }

                let result: Vec<String> = values.iter().map(|v| format!("{}", v)).collect();
                Ok(Value::String(result.join(separator)))
            }
            AggregateFunction::GroupConcatWithOrder(_, separator, order_by_fields) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };

                if values.is_empty() {
                    return Ok(Value::String(String::new()));
                }

                let mut indexed_values: Vec<(usize, &Value)> = values.iter().enumerate().collect();

                if !order_by_fields.is_empty() {
                    indexed_values.sort_by(|(_, val_a), (_, val_b)| {
                        let cmp_a = format!("{}", val_a);
                        let cmp_b = format!("{}", val_b);
                        cmp_a.cmp(&cmp_b)
                    });
                }

                let result: Vec<String> = indexed_values
                    .iter()
                    .map(|(_, v)| format!("{}", v))
                    .collect();
                Ok(Value::String(result.join(separator)))
            }
            AggregateFunction::VecSum(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                // Sum all vectors in the list
                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => {
                        return Err(ExpressionError::type_error(
                            "First argument must be a list of vectors",
                        ))
                    }
                };

                if values.is_empty() {
                    return Ok(Value::Null(NullType::NaN));
                }

                let mut sum_vec: Option<Vec<f32>> = None;
                for val in values.iter() {
                    if let Value::Vector(v) = val {
                        let data = v.to_dense();
                        match &mut sum_vec {
                            Some(sum) => {
                                if sum.len() != data.len() {
                                    return Err(ExpressionError::type_error(
                                        "Vector dimensions must match",
                                    ));
                                }
                                for (i, &val) in data.iter().enumerate() {
                                    sum[i] += val;
                                }
                            }
                            None => sum_vec = Some(data.clone()),
                        }
                    } else {
                        return Err(ExpressionError::type_error("All elements must be vectors"));
                    }
                }

                match sum_vec {
                    Some(data) => Ok(Value::vector(data)),
                    None => Ok(Value::Null(NullType::NaN)),
                }
            }
            AggregateFunction::PercentileCont(_, _) => {
                // Same logic as PERCENTILE
                if args.len() < 2 {
                    return Err(ExpressionError::argument_count_error(2, args.len()));
                }
                let percentile = match &args[1] {
                    Value::Int(v) => *v as f64,
                    Value::BigInt(v) => *v as f64,
                    Value::Float(v) => *v as f64,
                    Value::Double(v) => *v,
                    _ => return Err(ExpressionError::type_error("Percentile must be a number")),
                };
                if !(0.0..=100.0).contains(&percentile) {
                    return Err(ExpressionError::new(
                        ExpressionErrorType::InvalidOperation,
                        "Percentile must be between 0 and 100",
                    ));
                }
                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };
                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                let mut numeric_values = Vec::new();
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => numeric_values.push(*v as f64),
                        Value::Int(v) => numeric_values.push(*v as f64),
                        Value::BigInt(v) => numeric_values.push(*v as f64),
                        Value::Float(v) => numeric_values.push(*v as f64),
                        Value::Double(v) => numeric_values.push(*v),
                        _ => continue,
                    }
                }
                if numeric_values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                numeric_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let index = (percentile / 100.0) * (numeric_values.len() - 1) as f64;
                let lower_index = index.floor() as usize;
                let upper_index = index.ceil() as usize;
                if lower_index == upper_index {
                    Ok(Value::Double(numeric_values[lower_index]))
                } else {
                    let lower_value = numeric_values[lower_index];
                    let upper_value = numeric_values[upper_index];
                    let weight = index - lower_index as f64;
                    let interpolated = lower_value + weight * (upper_value - lower_value);
                    Ok(Value::Double(interpolated))
                }
            }
            AggregateFunction::StddevPop(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }
                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };
                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                let mut numeric_values = Vec::new();
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => numeric_values.push(*v as f64),
                        Value::Int(v) => numeric_values.push(*v as f64),
                        Value::BigInt(v) => numeric_values.push(*v as f64),
                        Value::Float(v) => numeric_values.push(*v as f64),
                        Value::Double(v) => numeric_values.push(*v),
                        _ => continue,
                    }
                }
                if numeric_values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                let n = numeric_values.len() as f64;
                let mean: f64 = numeric_values.iter().sum::<f64>() / n;
                let variance: f64 = numeric_values.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / n;
                Ok(Value::Double(variance.sqrt()))
            }
            AggregateFunction::StddevSamp(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }
                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };
                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                let mut numeric_values = Vec::new();
                for value in values.iter() {
                    match value {
                        Value::SmallInt(v) => numeric_values.push(*v as f64),
                        Value::Int(v) => numeric_values.push(*v as f64),
                        Value::BigInt(v) => numeric_values.push(*v as f64),
                        Value::Float(v) => numeric_values.push(*v as f64),
                        Value::Double(v) => numeric_values.push(*v),
                        _ => continue,
                    }
                }
                if numeric_values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                let n = numeric_values.len() as f64;
                if n < 2.0 {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                let mean: f64 = numeric_values.iter().sum::<f64>() / n;
                let variance: f64 = numeric_values.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / (n - 1.0);
                Ok(Value::Double(variance.sqrt()))
            }
            AggregateFunction::Product(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }
                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => return Err(ExpressionError::type_error("First argument must be a list")),
                };
                if values.is_empty() {
                    return Ok(Value::Null(crate::core::NullType::NaN));
                }
                let mut product = Value::Int(1);
                for value in values.iter() {
                    if value.is_null() || value.is_empty() {
                        continue;
                    }
                    product = product.mul(value).map_err(ExpressionError::runtime_error)?;
                }
                Ok(product)
            }
            AggregateFunction::VecAvg(_) => {
                if args.is_empty() {
                    return Err(ExpressionError::argument_count_error(1, args.len()));
                }

                // Average all vectors in the list
                let values = match &args[0] {
                    Value::List(list) => list,
                    _ => {
                        return Err(ExpressionError::type_error(
                            "First argument must be a list of vectors",
                        ))
                    }
                };

                if values.is_empty() {
                    return Ok(Value::Null(NullType::NaN));
                }

                let count = values.len() as f32;
                let mut sum_vec: Option<Vec<f32>> = None;
                for val in values.iter() {
                    if let Value::Vector(v) = val {
                        let data = v.to_dense();
                        match &mut sum_vec {
                            Some(sum) => {
                                if sum.len() != data.len() {
                                    return Err(ExpressionError::type_error(
                                        "Vector dimensions must match",
                                    ));
                                }
                                for (i, &val) in data.iter().enumerate() {
                                    sum[i] += val;
                                }
                            }
                            None => sum_vec = Some(data.clone()),
                        }
                    } else {
                        return Err(ExpressionError::type_error("All elements must be vectors"));
                    }
                }

                match sum_vec {
                    Some(mut data) => {
                        for val in data.iter_mut() {
                            *val /= count;
                        }
                        Ok(Value::vector(data))
                    }
                    None => Ok(Value::Null(NullType::NaN)),
                }
            }
        }
    }
}

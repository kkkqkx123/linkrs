//! Aggregate function computation
//!
//! Computes aggregate functions (COUNT, SUM, AVG, MIN, MAX, COLLECT) on row groups.

use crate::core::types::operators::AggregateFunction;
use crate::core::value::{List, NullType};
use crate::core::Value;

/// Compute aggregate function result for a group of rows
///
/// Handles COUNT, SUM, AVG, MIN, MAX, and COLLECT aggregate functions.
/// Used in Aggregate and GroupBy operators.
pub fn compute_aggregate(
    func: &AggregateFunction,
    rows: &[Vec<Value>],
    col_names: &[String],
) -> Value {
    match func {
        AggregateFunction::Count(None) => Value::BigInt(rows.len() as i64),
        AggregateFunction::Count(Some(field)) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                let count = rows
                    .iter()
                    .filter(|row| row.get(idx).is_some_and(|v| !matches!(v, Value::Null(_))))
                    .count();
                Value::BigInt(count as i64)
            } else {
                Value::BigInt(0)
            }
        }
        AggregateFunction::Sum(field) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                let sum = rows
                    .iter()
                    .filter_map(|row| row.get(idx))
                    .filter_map(|v| match v {
                        Value::BigInt(n) => Some(*n as f64),
                        Value::Int(n) => Some(*n as f64),
                        Value::Float(f) => Some(*f as f64),
                        Value::Double(f) => Some(*f),
                        _ => None,
                    })
                    .sum::<f64>();

                // Return as BigInt if no decimal places
                if sum.fract() == 0.0 {
                    Value::BigInt(sum as i64)
                } else {
                    Value::Double(sum)
                }
            } else {
                Value::Null(NullType::Null)
            }
        }
        AggregateFunction::Avg(field) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                let values: Vec<f64> = rows
                    .iter()
                    .filter_map(|row| row.get(idx))
                    .filter_map(|v| match v {
                        Value::BigInt(n) => Some(*n as f64),
                        Value::Int(n) => Some(*n as f64),
                        Value::Float(f) => Some(*f as f64),
                        Value::Double(f) => Some(*f),
                        _ => None,
                    })
                    .collect::<Vec<f64>>();

                if values.is_empty() {
                    Value::Null(NullType::Null)
                } else {
                    let avg = values.iter().sum::<f64>() / values.len() as f64;
                    Value::Double(avg)
                }
            } else {
                Value::Null(NullType::Null)
            }
        }
        AggregateFunction::Min(field) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                let min_val = rows
                    .iter()
                    .filter_map(|row| row.get(idx).cloned())
                    .min_by(|a, b| a.to_string().cmp(&b.to_string()));
                min_val.unwrap_or(Value::Null(NullType::Null))
            } else {
                Value::Null(NullType::Null)
            }
        }
        AggregateFunction::Max(field) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                let max_val = rows
                    .iter()
                    .filter_map(|row| row.get(idx).cloned())
                    .max_by(|a, b| a.to_string().cmp(&b.to_string()));
                max_val.unwrap_or(Value::Null(NullType::Null))
            } else {
                Value::Null(NullType::Null)
            }
        }
        AggregateFunction::Collect(field) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                let values: Vec<Value> = rows
                    .iter()
                    .filter_map(|row| row.get(idx).cloned())
                    .filter(|v| !matches!(v, Value::Null(_)))
                    .collect();

                if values.is_empty() {
                    Value::Null(NullType::Null)
                } else {
                    let mut list = List::new();
                    for value in values {
                        list.push(value);
                    }
                    Value::List(Box::new(list))
                }
            } else {
                Value::Null(NullType::Null)
            }
        }
        _ => Value::Null(NullType::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value::NullType;

    #[test]
    fn test_count_all() {
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(3)],
        ];
        let result = compute_aggregate(&AggregateFunction::Count(None), &rows, &["id".to_string()]);
        assert_eq!(result, Value::BigInt(3));
    }

    #[test]
    fn test_count_column_with_null() {
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Null(NullType::Null)],
            vec![Value::Int(3)],
        ];
        let result = compute_aggregate(
            &AggregateFunction::Count(Some("id".to_string())),
            &rows,
            &["id".to_string()],
        );
        // Should count only non-NULL values
        assert_eq!(result, Value::BigInt(2));
    }

    #[test]
    fn test_sum_numeric() {
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(3)],
        ];
        let result = compute_aggregate(
            &AggregateFunction::Sum("id".to_string()),
            &rows,
            &["id".to_string()],
        );
        assert_eq!(result, Value::BigInt(6));
    }

    #[test]
    fn test_avg_calculation() {
        let rows = vec![
            vec![Value::Int(2)],
            vec![Value::Int(4)],
            vec![Value::Int(6)],
        ];
        let result = compute_aggregate(
            &AggregateFunction::Avg("id".to_string()),
            &rows,
            &["id".to_string()],
        );
        assert_eq!(result, Value::Double(4.0));
    }

    #[test]
    fn test_minmax_values() {
        let rows = vec![
            vec![Value::Int(3)],
            vec![Value::Int(1)],
            vec![Value::Int(2)],
        ];

        let min = compute_aggregate(
            &AggregateFunction::Min("id".to_string()),
            &rows,
            &["id".to_string()],
        );
        let max = compute_aggregate(
            &AggregateFunction::Max("id".to_string()),
            &rows,
            &["id".to_string()],
        );

        // Verify min and max are not NULL
        assert!(!matches!(min, Value::Null(_)));
        assert!(!matches!(max, Value::Null(_)));

        // For Int values, min should be 1 and max should be 3
        if let Value::Int(min_val) = min {
            assert_eq!(min_val, 1);
        }
        if let Value::Int(max_val) = max {
            assert_eq!(max_val, 3);
        }
    }

    #[test]
    fn test_empty_rows_aggregation() {
        let rows: Vec<Vec<Value>> = vec![];

        let count = compute_aggregate(&AggregateFunction::Count(None), &rows, &[]);
        assert_eq!(count, Value::BigInt(0));

        // For empty rows, aggregation on non-existent column returns NULL
        let sum = compute_aggregate(
            &AggregateFunction::Sum("id".to_string()),
            &rows,
            &[], // empty col_names
        );
        // When column doesn't exist, returns NULL
        assert_eq!(sum, Value::Null(NullType::Null));
    }

    #[test]
    fn test_collect_aggregation() {
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(3)],
        ];
        let result = compute_aggregate(
            &AggregateFunction::Collect("id".to_string()),
            &rows,
            &["id".to_string()],
        );
        match result {
            Value::List(list_box) => {
                assert_eq!(list_box.len(), 3);
                let values = &list_box.values;
                assert_eq!(values[0], Value::Int(1));
                assert_eq!(values[1], Value::Int(2));
                assert_eq!(values[2], Value::Int(3));
            }
            _ => panic!("Expected Value::List, got {:?}", result),
        }
    }

    #[test]
    fn test_collect_with_null_values() {
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Null(NullType::Null)],
            vec![Value::Int(3)],
        ];
        let result = compute_aggregate(
            &AggregateFunction::Collect("id".to_string()),
            &rows,
            &["id".to_string()],
        );
        match result {
            Value::List(list_box) => {
                // Should skip NULL values
                assert_eq!(list_box.len(), 2);
                let values = &list_box.values;
                assert_eq!(values[0], Value::Int(1));
                assert_eq!(values[1], Value::Int(3));
            }
            _ => panic!("Expected Value::List, got {:?}", result),
        }
    }
}

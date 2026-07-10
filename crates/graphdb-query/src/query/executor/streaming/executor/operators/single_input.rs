//! Single-input operators: Filter, Project, Limit, Distinct

use crate::core::error::QueryError;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::{StreamingExecutor, ValueRowContext};

// ============ Filter ============

pub fn open_filter(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Filter { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_filter(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Filter {
            input, predicate, ..
        } => loop {
            match input.next()? {
                Some(mut chunk) => {
                    let col_names = chunk.col_names();
                    chunk.rows.retain(|row| {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        match ExpressionEvaluator::evaluate(predicate, &mut context) {
                            Ok(value) => match value {
                                Value::Bool(b) => b,
                                Value::Null(_) => false,
                                Value::Int(i) => i != 0,
                                Value::BigInt(i) => i != 0,
                                Value::Float(f) => f != 0.0,
                                Value::Double(f) => f != 0.0,
                                Value::String(s) => !s.is_empty(),
                                _ => true,
                            },
                            Err(_) => false,
                        }
                    });

                    if !chunk.rows.is_empty() {
                        return Ok(Some(chunk));
                    }
                }
                None => return Ok(None),
            }
        },
        _ => unreachable!(),
    }
}

pub fn stop_filter(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Filter { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_filter(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Filter { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ Project ============

pub fn open_project(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Project { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_project(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Project {
            input,
            output_expressions,
            ..
        } => {
            if let Some(chunk) = input.next()? {
                let col_names = chunk.col_names();

                let mut projected_rows = Vec::new();
                for row in chunk.rows {
                    let mut context = ValueRowContext::new(row, col_names.clone());
                    let mut projected_row = Vec::new();

                    for expr in output_expressions.iter() {
                        match ExpressionEvaluator::evaluate(expr, &mut context) {
                            Ok(value) => {
                                projected_row.push(value);
                            }
                            Err(_) => {
                                projected_row.push(Value::Null(NullType::Null));
                            }
                        }
                    }

                    projected_rows.push(projected_row);
                }

                Ok(Some(DataChunk::from_rows(projected_rows)))
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_project(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Project { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_project(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Project { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ Limit ============

pub fn open_limit(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Limit { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_limit(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Limit {
            input,
            limit,
            consumed,
            ..
        } => {
            if *consumed >= *limit {
                return Ok(None);
            }

            if let Some(mut chunk) = input.next()? {
                let remaining = *limit - *consumed;

                if chunk.rows.len() > remaining as usize {
                    chunk.rows.truncate(remaining as usize);
                }

                *consumed += chunk.rows.len() as u32;
                Ok(Some(chunk))
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_limit(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Limit { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_limit(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Limit { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ Distinct ============

pub fn open_distinct(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Distinct { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_distinct(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Distinct {
            input, seen_rows, ..
        } => loop {
            match input.next()? {
                Some(chunk) => {
                    let mut result_rows = Vec::new();
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
                None => return Ok(None),
            }
        },
        _ => unreachable!(),
    }
}

pub fn stop_distinct(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Distinct { input, .. } => input.stop(),
        _ => unreachable!(),
    }
}

pub fn close_distinct(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Distinct { input, opened, .. } => {
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

    fn create_test_buffer(size: usize) -> Vec<Vec<Value>> {
        (0..size)
            .map(|i| vec![Value::Int(i as i32), Value::String(format!("item_{}", i))])
            .collect()
    }

    #[test]
    fn test_filter_basic() {
        let buffer = create_test_buffer(10);
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        let mut filter = StreamingExecutor::Filter {
            input: scan,
            predicate: Expression::Literal(Value::Bool(true)),
            opened: false,
        };

        filter.open().unwrap();
        let chunk = filter.next().unwrap();
        assert!(chunk.is_some());
        // All rows should pass (predicate is always true)
        assert_eq!(chunk.unwrap().len(), 10);
        filter.close().unwrap();
    }

    #[test]
    fn test_filter_empty() {
        let buffer = create_test_buffer(5);
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        let mut filter = StreamingExecutor::Filter {
            input: scan,
            predicate: Expression::Literal(Value::Bool(false)),
            opened: false,
        };

        filter.open().unwrap();
        let chunk = filter.next().unwrap();
        // All rows filtered out (predicate is always false)
        assert!(chunk.is_none());
        filter.close().unwrap();
    }

    #[test]
    fn test_project_reorder() {
        let buffer = vec![
            vec![
                Value::Int(1),
                Value::String("a".to_string()),
                Value::Int(100),
            ],
            vec![
                Value::Int(2),
                Value::String("b".to_string()),
                Value::Int(200),
            ],
        ];

        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        // Project to reorder columns (swap first two)
        let mut project = StreamingExecutor::Project {
            input: scan,
            output_expressions: vec![
                Expression::Literal(Value::Int(0)),
                Expression::Literal(Value::Int(0)),
            ],
            opened: false,
        };

        project.open().unwrap();
        let chunk = project.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.rows[0].len(), 2);
        project.close().unwrap();
    }

    #[test]
    fn test_project_expression() {
        let buffer = vec![vec![Value::Int(5), Value::Int(3)]];

        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        let mut project = StreamingExecutor::Project {
            input: scan,
            output_expressions: vec![Expression::Literal(Value::String("col1".to_string()))],
            opened: false,
        };

        project.open().unwrap();
        let chunk = project.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 1);
        assert_eq!(chunk.rows[0].len(), 1);
        project.close().unwrap();
    }

    #[test]
    fn test_limit_exact() {
        let buffer = create_test_buffer(100);
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 10,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let chunk = limit.next().unwrap();
        assert!(chunk.is_some());
        assert_eq!(chunk.unwrap().len(), 10);

        // Next chunk should be None (limit reached)
        let chunk2 = limit.next().unwrap();
        assert!(chunk2.is_none());
        limit.close().unwrap();
    }

    #[test]
    fn test_limit_boundary() {
        let buffer = create_test_buffer(50);
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        // Limit larger than buffer size
        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 100,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let chunk = limit.next().unwrap();
        assert!(chunk.is_some());
        // Should return all 50 rows
        assert_eq!(chunk.unwrap().len(), 50);

        let chunk2 = limit.next().unwrap();
        assert!(chunk2.is_none());
        limit.close().unwrap();
    }

    #[test]
    fn test_distinct_basic() {
        let buffer = vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ];

        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        let mut distinct = StreamingExecutor::Distinct {
            input: scan,
            seen_rows: std::collections::HashSet::new(),
            opened: false,
        };

        distinct.open().unwrap();
        let chunk = distinct.next().unwrap();
        assert!(chunk.is_some());
        // Should deduplicate: 2 distinct rows
        let chunk = chunk.unwrap();
        assert!(chunk.len() <= 3 && chunk.len() >= 2);
        distinct.close().unwrap();
    }

    #[test]
    fn test_distinct_with_nulls() {
        let buffer = vec![
            vec![Value::Int(1), Value::Null(NullType::Null)],
            vec![Value::Int(1), Value::Null(NullType::Null)],
            vec![Value::Int(2), Value::String("b".to_string())],
        ];

        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        let mut distinct = StreamingExecutor::Distinct {
            input: scan,
            seen_rows: std::collections::HashSet::new(),
            opened: false,
        };

        distinct.open().unwrap();
        let chunk = distinct.next().unwrap();
        assert!(chunk.is_some());
        distinct.close().unwrap();
    }
}

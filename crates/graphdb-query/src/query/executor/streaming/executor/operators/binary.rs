//! Binary operators: HashJoin, NestedLoopJoin

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::{StreamingExecutor, ValueRowContext};

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
    match executor {
        StreamingExecutor::HashJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            ..
        } => {
            if !*left_consumed {
                // Build right side - collect all rows from right executor
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        build_side_tuples.push(row);
                    }
                }

                *left_consumed = true;
            }

            if let Some(left_chunk) = left.next()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                for left_row in &left_chunk.rows {
                    for right_row in build_side_tuples.iter() {
                        // Check join condition if provided
                        let condition_satisfied = if let Some(condition) = join_condition {
                            // Create combined row for context
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());

                            // Create schema for combined row with correct column names
                            let mut combined_col_names = left_col_names.clone();
                            // Add right columns with "right_" prefix to avoid conflicts
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
                                Err(_) => false,
                            }
                        } else {
                            // No condition means join all
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
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
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

pub fn next_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            ..
        } => {
            if !*left_consumed {
                // Build right side - collect all rows
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        build_side_tuples.push(row);
                    }
                }
                *left_consumed = true;
            }

            if let Some(left_chunk) = left.next()? {
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
                                Err(_) => false,
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
            ..
        } => {
            if *opened {
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
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: create_right_buffer(),
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
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
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(20), Value::String("b".to_string())]],
            current_index: 0,
        });

        // Condition that never matches
        let join_condition = Some(Expression::Literal(Value::Bool(false)));

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        // No rows match the condition
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
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::String("b1".to_string())],
                vec![Value::Int(1), Value::String("b2".to_string())],
            ],
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        // 2 left rows × 2 right rows = 4 result rows
        assert_eq!(chunk.unwrap().len(), 4);
        join.close().unwrap();
    }

    #[test]
    fn test_nestedloop_cartesian() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
            ],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(10)],
                vec![Value::Int(20)],
                vec![Value::Int(30)],
            ],
            current_index: 0,
        });

        let mut join = StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition: None,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
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
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            current_index: 0,
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
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
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
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::String("x".to_string()), Value::Int(10)]],
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        // Both left rows should match: 2 × 1 = 2
        assert_eq!(chunk.unwrap().len(), 2);
        join.close().unwrap();
    }

    #[test]
    fn test_join_column_naming() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1), Value::String("left".to_string())]],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(2), Value::String("right".to_string())]],
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // Result row should have 4 columns (2 from left + 2 from right)
        assert_eq!(chunk.rows[0].len(), 4);
        join.close().unwrap();
    }
}


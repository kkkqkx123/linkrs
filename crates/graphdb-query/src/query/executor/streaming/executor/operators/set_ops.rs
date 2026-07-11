//! Set operations: Union, UnionAll, Intersect, Except

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;

// ============ Union ============

pub fn open_union(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Union {
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

pub fn next_union(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Union {
            left,
            right,
            seen_rows,
            left_consumed,
            memory_tracker,
            ..
        } => {
            // Process left side first
            if !*left_consumed {
                if let Some(chunk) = left.advance()? {
                    let mut result_rows = Vec::new();
                    for row in chunk.rows {
                        let row_str = format!("{:?}", row);
                        if !seen_rows.contains(&row_str) {
                            memory_tracker.try_reserve(row_str.len())?;
                            seen_rows.insert(row_str);
                            result_rows.push(row);
                        }
                    }
                    return if result_rows.is_empty() {
                        executor.advance()
                    } else {
                        Ok(Some(DataChunk::from_rows(result_rows)))
                    };
                } else {
                    *left_consumed = true;
                }
            }

            // Process right side
            if let Some(chunk) = right.advance()? {
                let mut result_rows = Vec::new();
                for row in chunk.rows {
                    let row_str = format!("{:?}", row);
                    if !seen_rows.contains(&row_str) {
                        seen_rows.insert(row_str);
                        result_rows.push(row);
                    }
                }
                if result_rows.is_empty() {
                    executor.advance()
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

pub fn stop_union(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Union { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_union(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Union {
            left,
            right,
            opened,
            seen_rows,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = seen_rows.len() * 256;
                memory_tracker.release(mem);
                seen_rows.clear();
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ UnionAll ============

pub fn open_unionall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UnionAll {
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

pub fn next_unionall(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::UnionAll {
            left,
            right,
            left_consumed,
            ..
        } => {
            if !*left_consumed {
                if let Some(chunk) = left.advance()? {
                    return Ok(Some(chunk));
                } else {
                    *left_consumed = true;
                }
            }

            right.advance()
        }
        _ => unreachable!(),
    }
}

pub fn stop_unionall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UnionAll { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_unionall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UnionAll {
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

// ============ Intersect ============

pub fn open_intersect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Intersect {
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

pub fn next_intersect(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Intersect {
            left,
            right,
            left_rows,
            right_rows,
            left_buffered,
            right_buffered,
            memory_tracker,
            ..
        } => {
            if !*left_buffered {
                while let Some(chunk) = left.advance()? {
                    for row in &chunk.rows {
                        memory_tracker.try_reserve_row(row)?;
                    }
                    left_rows.extend(chunk.rows);
                }
                *left_buffered = true;
            }

            if !*right_buffered {
                while let Some(chunk) = right.advance()? {
                    for row in chunk.rows {
                        let row_str = format!("{:?}", row);
                        memory_tracker.try_reserve(row_str.len())?;
                        right_rows.insert(row_str);
                    }
                }
                *right_buffered = true;
            }

            let result_rows: Vec<Vec<Value>> = left_rows
                .iter()
                .filter(|row| right_rows.contains(&format!("{:?}", row)))
                .cloned()
                .collect();

            if result_rows.is_empty() {
                Ok(None)
            } else {
                Ok(Some(DataChunk::from_rows(result_rows)))
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_intersect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Intersect { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_intersect(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Intersect {
            left,
            right,
            opened,
            left_rows,
            right_rows,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem_rows = MemoryBudget::estimate_rows_memory(left_rows);
                let mem_set = right_rows.len() * 256;
                memory_tracker.release(mem_rows + mem_set);
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

// ============ Except ============

pub fn open_except(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Except {
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

pub fn next_except(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Except {
            left,
            right,
            exclude_rows,
            right_buffered,
            memory_tracker,
            ..
        } => {
            // Buffer right side for exclusion
            if !*right_buffered {
                while let Some(chunk) = right.advance()? {
                    for row in chunk.rows {
                        let row_str = format!("{:?}", row);
                        memory_tracker.try_reserve(row_str.len())?;
                        exclude_rows.insert(row_str);
                    }
                }
                *right_buffered = true;
            }

            // Process left side
            if let Some(chunk) = left.advance()? {
                let result_rows: Vec<Vec<Value>> = chunk
                    .rows
                    .into_iter()
                    .filter(|row| !exclude_rows.contains(&format!("{:?}", row)))
                    .collect();

                if result_rows.is_empty() {
                    executor.advance()
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

pub fn stop_except(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Except { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_except(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Except {
            left,
            right,
            opened,
            exclude_rows,
            memory_tracker,
            ..
        } => {
            if *opened {
                let mem = exclude_rows.len() * 256;
                memory_tracker.release(mem);
                exclude_rows.clear();
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
use crate::query::executor::base::MemoryBudget;
#[cfg(test)]
use crate::query::executor::base::MemoryTracker;

    // ====== Union Tests ======

    #[test]
    fn test_union_basic_dedup() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::String("a".to_string())],
                vec![Value::Int(2), Value::String("b".to_string())],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(2), Value::String("b".to_string())],
                vec![Value::Int(3), Value::String("c".to_string())],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut union = StreamingExecutor::Union {
            left,
            right,
            seen_rows: std::collections::HashSet::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        union.open().unwrap();
        let chunk = union.advance().unwrap();
        assert!(chunk.is_some());
        // Union deduplicates: should have at most 3 distinct rows
        let mut total_rows = 0;
        let mut current = chunk;
        while let Some(c) = current {
            total_rows += c.len();
            current = union.advance().unwrap();
        }
        assert_eq!(total_rows, 3);
        union.close().unwrap();
    }

    #[test]
    fn test_union_no_duplicates() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(2)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut union = StreamingExecutor::Union {
            left,
            right,
            seen_rows: std::collections::HashSet::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        union.open().unwrap();
        let chunk = union.advance().unwrap();
        assert!(chunk.is_some());
        let mut total = 0;
        let mut current = chunk;
        while let Some(c) = current {
            total += c.len();
            current = union.advance().unwrap();
        }
        assert_eq!(total, 2);
        union.close().unwrap();
    }

    #[test]
    fn test_union_empty_left() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![],
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

        let mut union = StreamingExecutor::Union {
            left,
            right,
            seen_rows: std::collections::HashSet::new(),
            left_consumed: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        union.open().unwrap();
        let chunk = union.advance().unwrap();
        assert!(chunk.is_some());
        union.close().unwrap();
    }

    // ====== UnionAll Tests ======

    #[test]
    fn test_unionall_basic() {
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
            buffer: vec![vec![Value::Int(2)], vec![Value::Int(3)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut unionall = StreamingExecutor::UnionAll {
            left,
            right,
            left_consumed: false,
            opened: false,
            plan_node_id: 0,
            runtime: None,
        };

        unionall.open().unwrap();
        let chunk = unionall.advance().unwrap();
        assert!(chunk.is_some());
        // UnionAll does NOT deduplicate: 2 from left + 2 from right = 4
        let chunk1 = chunk.unwrap();
        assert_eq!(chunk1.len(), 2); // First chunk from left
        let chunk2 = unionall.advance().unwrap();
        assert!(chunk2.is_some());
        assert_eq!(chunk2.unwrap().len(), 2); // Second chunk from right
        unionall.close().unwrap();
    }

    #[test]
    fn test_unionall_preserves_duplicates() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1), Value::String("a".to_string())]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1), Value::String("a".to_string())]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut unionall = StreamingExecutor::UnionAll {
            left,
            right,
            left_consumed: false,
            opened: false,
            plan_node_id: 0,
            runtime: None,
        };

        unionall.open().unwrap();
        let chunk1 = unionall.advance().unwrap();
        assert!(chunk1.is_some());
        let chunk2 = unionall.advance().unwrap();
        assert!(chunk2.is_some());
        // Both chunks exist, proving duplicates are preserved
        unionall.close().unwrap();
    }

    // ====== Intersect Tests ======

    #[test]
    fn test_intersect_common_elements() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(2)],
                vec![Value::Int(3)],
                vec![Value::Int(4)],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut intersect = StreamingExecutor::Intersect {
            left,
            right,
            left_rows: Vec::new(),
            right_rows: std::collections::HashSet::new(),
            left_buffered: false,
            right_buffered: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        intersect.open().unwrap();
        let chunk = intersect.advance().unwrap();
        assert!(chunk.is_some());
        // Intersection of {1,2,3} and {2,3,4} = {2,3}
        assert_eq!(chunk.unwrap().len(), 2);
        intersect.close().unwrap();
    }

    #[test]
    fn test_intersect_no_common_elements() {
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
            buffer: vec![vec![Value::Int(3)], vec![Value::Int(4)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut intersect = StreamingExecutor::Intersect {
            left,
            right,
            left_rows: Vec::new(),
            right_rows: std::collections::HashSet::new(),
            left_buffered: false,
            right_buffered: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        intersect.open().unwrap();
        let result = intersect.advance().unwrap();
        // With no common elements, should return None
        assert!(result.is_none());
        intersect.close().unwrap();
    }

    // ====== Except Tests ======

    #[test]
    fn test_except_basic_difference() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(2)]],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut except = StreamingExecutor::Except {
            left,
            right,
            exclude_rows: std::collections::HashSet::new(),
            right_buffered: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        except.open().unwrap();
        let chunk = except.advance().unwrap();
        assert!(chunk.is_some());
        // Should have 2 rows (1 and 3, excluding 2)
        let c = chunk.unwrap();
        assert_eq!(c.len(), 2);
        except.close().unwrap();
    }

    #[test]
    fn test_except_all_excluded() {
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

        let mut except = StreamingExecutor::Except {
            left,
            right,
            exclude_rows: std::collections::HashSet::new(),
            right_buffered: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        except.open().unwrap();
        let result = except.advance().unwrap();
        // All rows are excluded, should return None
        assert!(result.is_none());
        except.close().unwrap();
    }

    #[test]
    fn test_except_empty_right() {
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
            buffer: vec![],
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut except = StreamingExecutor::Except {
            left,
            right,
            exclude_rows: std::collections::HashSet::new(),
            right_buffered: false,
            opened: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            plan_node_id: 0,
            runtime: None,
        };

        except.open().unwrap();
        let chunk = except.advance().unwrap();
        assert!(chunk.is_some());
        // No exclusions, should return all left rows
        assert_eq!(chunk.unwrap().len(), 2);
        except.close().unwrap();
    }
}

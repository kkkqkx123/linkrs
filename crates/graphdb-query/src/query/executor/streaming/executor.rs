//! StreamingExecutor: Enum-based pull executor
//!
//! Uses Enum (not Trait) for type safety and compile-time completeness checking.
//! Each variant represents one executor type.

use super::chunk::DataChunk;
use crate::core::error::QueryError;
use std::ops::Range;

/// Pull-based streaming executor
///
/// Each variant handles different operation types:
/// - Data sources: ScanVertices, ScanEdges
/// - Single input: Filter, Project, Limit
/// - Stateful: Aggregate, Sort
/// - Binary input: HashJoin
pub enum StreamingExecutor {
    // ============ Data Sources ============

    /// Scan vertices from a partition
    ScanVertices {
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },

    /// Scan edges from a partition
    ScanEdges {
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },

    // ============ Single Input ============

    /// Filter executor
    Filter {
        input: Box<StreamingExecutor>,
        opened: bool,
    },

    /// Project executor
    Project {
        input: Box<StreamingExecutor>,
        opened: bool,
    },

    /// Limit executor
    Limit {
        input: Box<StreamingExecutor>,
        limit: u32,
        consumed: u32,
        opened: bool,
    },

    // ============ Stateful ============

    /// Aggregate executor
    Aggregate {
        input: Box<StreamingExecutor>,
        opened: bool,
    },

    /// Sort executor
    Sort {
        input: Box<StreamingExecutor>,
        all_rows: Vec<Vec<String>>,
        row_iter: Option<std::vec::IntoIter<Vec<String>>>,
        opened: bool,
    },

    // ============ Binary Input ============

    /// HashJoin executor
    HashJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        build_side_tuples: Vec<Vec<String>>,
        left_consumed: bool,
        opened: bool,
    },
}

impl StreamingExecutor {
    /// Initialize the executor
    pub fn open(&mut self) -> Result<(), QueryError> {
        match self {
            Self::ScanVertices { .. } => Ok(()),
            Self::ScanEdges { .. } => Ok(()),
            Self::Filter { input, opened } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Project { input, opened } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Limit { input, opened, .. } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Aggregate { input, opened } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Sort { input, opened, .. } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::HashJoin { left, right, opened, .. } => {
                left.open()?;
                right.open()?;
                *opened = true;
                Ok(())
            }
        }
    }

    /// Pull next chunk from the executor
    pub fn next(&mut self) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::ScanVertices {
                current_offset,
                partition_range,
                ..
            } => {
                const CHUNK_SIZE: u32 = 1024;

                if *current_offset >= partition_range.end {
                    return Ok(None);
                }

                let end = (*current_offset + CHUNK_SIZE).min(partition_range.end);
                let count = end - *current_offset;

                // Mock: generate dummy data
                let mut rows = Vec::new();
                for i in 0..count {
                    rows.push(vec![
                        (*current_offset + i).to_string(),
                        format!("vertex_{}", *current_offset + i),
                    ]);
                }

                *current_offset = end;
                Ok(Some(DataChunk::from_rows(rows)))
            }
            Self::Filter { input, .. } => {
                // Simple pass-through for now
                input.next()
            }
            Self::Project { input, .. } => {
                // Simple pass-through for now
                input.next()
            }
            Self::Limit {
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
            Self::Aggregate { .. } => {
                // TODO: Implement aggregation
                Ok(None)
            }
            Self::Sort { .. } => {
                // TODO: Implement sort
                Ok(None)
            }
            Self::HashJoin { .. } => {
                // TODO: Implement hash join
                Ok(None)
            }
            Self::ScanEdges {
                current_offset,
                partition_range,
                ..
            } => {
                const CHUNK_SIZE: u32 = 1024;

                if *current_offset >= partition_range.end {
                    return Ok(None);
                }

                let end = (*current_offset + CHUNK_SIZE).min(partition_range.end);
                let count = end - *current_offset;

                // Mock: generate dummy data
                let mut rows = Vec::new();
                for i in 0..count {
                    rows.push(vec![
                        (*current_offset + i).to_string(),
                        format!("src_{}", *current_offset + i),
                        format!("dst_{}", *current_offset + i),
                    ]);
                }

                *current_offset = end;
                Ok(Some(DataChunk::from_rows(rows)))
            }
        }
    }

    /// Stop execution (for LIMIT)
    pub fn stop(&mut self) -> Result<(), QueryError> {
        match self {
            Self::Filter { input, .. } => input.stop(),
            Self::Project { input, .. } => input.stop(),
            Self::Limit { input, .. } => input.stop(),
            Self::Aggregate { input, .. } => input.stop(),
            Self::Sort { input, .. } => input.stop(),
            Self::HashJoin { left, right, .. } => {
                left.stop()?;
                right.stop()
            }
            _ => Ok(()),
        }
    }

    /// Clean up resources
    pub fn close(&mut self) -> Result<(), QueryError> {
        match self {
            Self::Filter { input, opened } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Project { input, opened } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Limit { input, opened, .. } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Aggregate { input, opened } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Sort { input, opened, .. } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::HashJoin {
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
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_vertices() {
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);
        executor.close().unwrap();
    }

    #[test]
    fn test_limit() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..10000,
            current_offset: 0,
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 10,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = limit.next().unwrap() {
            total += chunk.len();
        }
        limit.close().unwrap();

        assert_eq!(total, 10);
    }
}

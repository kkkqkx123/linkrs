//! Data source operators: ScanVertices, ScanEdges

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;

const CHUNK_SIZE: usize = 1024;

/// Open ScanVertices operator
pub fn open_scanvertices(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Next chunk from ScanVertices
pub fn next_scanvertices(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ScanVertices {
            current_index,
            buffer,
            col_names,
            ..
        } => {
            if *current_index >= buffer.len() {
                return Ok(None);
            }

            let end = (*current_index + CHUNK_SIZE).min(buffer.len());
            let chunk_rows: Vec<Vec<Value>> = buffer[*current_index..end].to_vec();
            *current_index = end;

            if chunk_rows.is_empty() {
                Ok(None)
            } else {
                let col = if col_names.is_empty() {
                    None
                } else {
                    Some(col_names.clone())
                };
                Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
            }
        }
        _ => unreachable!(),
    }
}

/// Stop ScanVertices operator
pub fn stop_scanvertices(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Close ScanVertices operator
pub fn close_scanvertices(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Open ScanEdges operator
pub fn open_scanedges(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Next chunk from ScanEdges
pub fn next_scanedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ScanEdges {
            current_index,
            buffer,
            col_names,
            ..
        } => {
            if *current_index >= buffer.len() {
                return Ok(None);
            }

            let end = (*current_index + CHUNK_SIZE).min(buffer.len());
            let chunk_rows: Vec<Vec<Value>> = buffer[*current_index..end].to_vec();
            *current_index = end;

            if chunk_rows.is_empty() {
                Ok(None)
            } else {
                let col = if col_names.is_empty() {
                    None
                } else {
                    Some(col_names.clone())
                };
                Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
            }
        }
        _ => unreachable!(),
    }
}

/// Stop ScanEdges operator
pub fn stop_scanedges(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Close ScanEdges operator
pub fn close_scanedges(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;

    fn create_test_buffer(size: usize) -> Vec<Vec<Value>> {
        (0..size)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::String(format!("item_{}", i)),
                ]
            })
            .collect()
    }

    #[test]
    fn test_scan_vertices_chunking() {
        // Test that chunks are 1024 rows max
        let buffer = create_test_buffer(2100);
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
        };

        executor.open().unwrap();

        // First chunk should be 1024
        let chunk1 = executor.next().unwrap();
        assert!(chunk1.is_some());
        assert_eq!(chunk1.unwrap().len(), 1024);

        // Second chunk should be 1024
        let chunk2 = executor.next().unwrap();
        assert!(chunk2.is_some());
        assert_eq!(chunk2.unwrap().len(), 1024);

        // Third chunk should be 52 (2100 - 1024 - 1024)
        let chunk3 = executor.next().unwrap();
        assert!(chunk3.is_some());
        assert_eq!(chunk3.unwrap().len(), 52);

        // Fourth chunk should be None
        let chunk4 = executor.next().unwrap();
        assert!(chunk4.is_none());

        executor.close().unwrap();
    }

    #[test]
    fn test_scan_empty_buffer() {
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![],
            current_index: 0,
            col_names: vec![],
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_none());
        executor.close().unwrap();
    }

    #[test]
    fn test_scan_small_buffer() {
        // Test with buffer smaller than chunk size
        let buffer = create_test_buffer(500);
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        assert_eq!(chunk.unwrap().len(), 500);

        // Next should be None
        let chunk2 = executor.next().unwrap();
        assert!(chunk2.is_none());

        executor.close().unwrap();
    }
}

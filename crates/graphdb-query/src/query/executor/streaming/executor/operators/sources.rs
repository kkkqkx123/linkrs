//! Data source operators: ScanVertices, ScanEdges
//!
//! The `StorageScanVertices` / `StorageScanEdges` variants hold a live
//! cursor and pull batches on each `next()` call.  This enables true
//! streaming execution: downstream operators like `Limit` can terminate
//! early without forcing a full scan.

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::storage::cursor::{open_edge_scan, open_vertex_scan};

const CHUNK_SIZE: usize = 1024;

// ---------------------------------------------------------------------------
// ScanVertices
// ---------------------------------------------------------------------------

/// Open ScanVertices operator
pub fn open_scanvertices(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Next chunk from ScanVertices
pub fn next_scanvertices(
    executor: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        // Pre-buffered variant (used in tests / internal wrapping)
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
        // Lazy cursor variant – pulls one batch at a time from storage
        StreamingExecutor::StorageScanVertices {
            storage,
            space_name,
            limit,
            cursor,
            buffer: _,
            current_index: _,
            col_names,
            ..
        } => {
            // Initialize cursor on first pull
            if cursor.is_none() {
                *cursor = if let Some(storage) = storage.as_ref() {
                    Some(
                        open_vertex_scan(storage, space_name, *limit)
                            .map_err(|e| QueryError::execution(e.to_string()))?,
                    )
                } else {
                    return Ok(None);
                };
            }

            // Pull one batch from the live cursor
            let c = cursor.as_mut().unwrap();
            let batch = c
                .next_batch(CHUNK_SIZE)
                .map_err(|e| QueryError::execution(e.to_string()))?;

            if batch.is_empty() {
                *cursor = None;
                return Ok(None);
            }

            let chunk_rows: Vec<Vec<Value>> = batch
                .into_iter()
                .map(|vertex| vec![Value::Vertex(Box::new(vertex))])
                .collect();

            let col = if col_names.is_empty() {
                None
            } else {
                Some(col_names.clone())
            };
            Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
        }
        _ => unreachable!(),
    }
}

/// Stop ScanVertices operator
pub fn stop_scanvertices(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Close ScanVertices operator – drops the cursor to release resources.
pub fn close_scanvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    if let StreamingExecutor::StorageScanVertices {
        cursor, current_index, ..
    } = executor
    {
        *cursor = None;
        *current_index = 0;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ScanEdges
// ---------------------------------------------------------------------------

/// Open ScanEdges operator
pub fn open_scanedges(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Next chunk from ScanEdges
pub fn next_scanedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        // Pre-buffered variant (used in tests / internal wrapping)
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
        // Lazy cursor variant – pulls one batch at a time from storage
        StreamingExecutor::StorageScanEdges {
            storage,
            space_name,
            limit,
            cursor,
            buffer: _,
            current_index: _,
            col_names,
            ..
        } => {
            // Initialize cursor on first pull
            if cursor.is_none() {
                *cursor = if let Some(storage) = storage.as_ref() {
                    Some(
                        open_edge_scan(storage, space_name, None, *limit)
                            .map_err(|e| QueryError::execution(e.to_string()))?,
                    )
                } else {
                    return Ok(None);
                };
            }

            // Pull one batch from the live cursor
            let c = cursor.as_mut().unwrap();
            let batch = c
                .next_batch(CHUNK_SIZE)
                .map_err(|e| QueryError::execution(e.to_string()))?;

            if batch.is_empty() {
                *cursor = None;
                return Ok(None);
            }

            let chunk_rows: Vec<Vec<Value>> = batch
                .into_iter()
                .map(|edge| vec![Value::Edge(Box::new(edge))])
                .collect();

            let col = if col_names.is_empty() {
                None
            } else {
                Some(col_names.clone())
            };
            Ok(Some(DataChunk::from_rows_with_col_names(chunk_rows, col)))
        }
        _ => unreachable!(),
    }
}

/// Stop ScanEdges operator
pub fn stop_scanedges(_executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    Ok(())
}

/// Close ScanEdges operator – drops the cursor to release resources.
pub fn close_scanedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    if let StreamingExecutor::StorageScanEdges {
        cursor, current_index, ..
    } = executor
    {
        *cursor = None;
        *current_index = 0;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        let buffer = create_test_buffer(2100);
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
        };

        executor.open().unwrap();

        let chunk1 = executor.next().unwrap();
        assert!(chunk1.is_some());
        assert_eq!(chunk1.unwrap().len(), 1024);

        let chunk2 = executor.next().unwrap();
        assert!(chunk2.is_some());
        assert_eq!(chunk2.unwrap().len(), 1024);

        let chunk3 = executor.next().unwrap();
        assert!(chunk3.is_some());
        assert_eq!(chunk3.unwrap().len(), 52);

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
            plan_node_id: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_none());
        executor.close().unwrap();
    }

    #[test]
    fn test_scan_small_buffer() {
        let buffer = create_test_buffer(500);
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        assert_eq!(chunk.unwrap().len(), 500);

        let chunk2 = executor.next().unwrap();
        assert!(chunk2.is_none());

        executor.close().unwrap();
    }
}

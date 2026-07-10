//! Data source operators: ScanVertices, ScanEdges

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::storage::cursor::{open_edge_scan, open_vertex_scan, EdgeCursor, VertexCursor};

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
        StreamingExecutor::StorageScanVertices {
            storage,
            space_name,
            limit,
            buffer,
            current_index,
            col_names,
            ..
        } => {
            if buffer.is_none() {
                let mut rows = if let Some(storage) = storage {
                    let mut cursor = open_vertex_scan(storage, space_name)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    let mut all = Vec::new();
                    loop {
                        let batch = cursor
                            .next_batch(CHUNK_SIZE)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        if batch.is_empty() {
                            break;
                        }
                        all.extend(
                            batch
                                .into_iter()
                                .map(|vertex| vec![Value::Vertex(Box::new(vertex))]),
                        );
                    }
                    all
                } else {
                    Vec::new()
                };

                if let Some(limit) = *limit {
                    rows.truncate(limit);
                }

                *buffer = Some(rows);
            }

            let rows = buffer
                .as_ref()
                .ok_or_else(|| QueryError::execution("Storage scan buffer not initialized"))?;
            if *current_index >= rows.len() {
                return Ok(None);
            }

            let end = (*current_index + CHUNK_SIZE).min(rows.len());
            let chunk_rows: Vec<Vec<Value>> = rows[*current_index..end].to_vec();
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
pub fn close_scanvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    if let StreamingExecutor::StorageScanVertices {
        buffer,
        current_index,
        ..
    } = executor
    {
        *buffer = None;
        *current_index = 0;
    }
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
        StreamingExecutor::StorageScanEdges {
            storage,
            space_name,
            limit,
            buffer,
            current_index,
            col_names,
            ..
        } => {
            if buffer.is_none() {
                let mut rows = if let Some(storage) = storage {
                    let mut cursor = open_edge_scan(storage, space_name, None)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    let mut all = Vec::new();
                    loop {
                        let batch = cursor
                            .next_batch(CHUNK_SIZE)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        if batch.is_empty() {
                            break;
                        }
                        all.extend(
                            batch
                                .into_iter()
                                .map(|edge| vec![Value::Edge(Box::new(edge))]),
                        );
                    }
                    all
                } else {
                    Vec::new()
                };

                if let Some(limit) = *limit {
                    rows.truncate(limit);
                }

                *buffer = Some(rows);
            }

            let rows = buffer
                .as_ref()
                .ok_or_else(|| QueryError::execution("Storage scan buffer not initialized"))?;
            if *current_index >= rows.len() {
                return Ok(None);
            }

            let end = (*current_index + CHUNK_SIZE).min(rows.len());
            let chunk_rows: Vec<Vec<Value>> = rows[*current_index..end].to_vec();
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
pub fn close_scanedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    if let StreamingExecutor::StorageScanEdges {
        buffer,
        current_index,
        ..
    } = executor
    {
        *buffer = None;
        *current_index = 0;
    }
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
            plan_node_id: 0,
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
            plan_node_id: 0,
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
            plan_node_id: 0,
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

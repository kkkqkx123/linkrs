//! Utility functions for converting streaming results to standard formats.

use super::chunk::DataChunk;
use super::chunk::LocalChunkCollector;
use graphdb_core::DataSet;
use crate::executor::base::ExecutionResult;
use graphdb_core::error::QueryError;

/// Convert a Vec of DataChunks to a single DataSet
///
/// Merges all chunks into a unified result set with consistent column names
/// and aggregated rows.
///
/// # Arguments
/// * `chunks` - Vector of data chunks to merge
/// * `col_names` - Optional column names to use; if None, extracted from first chunk's schema
///
/// # Returns
/// * Result with merged DataSet or error if chunks are incompatible
pub fn convert_chunks_to_dataset(
    chunks: Vec<DataChunk>,
    col_names: Option<Vec<String>>,
) -> Result<DataSet, QueryError> {
    if chunks.is_empty() {
        let names = col_names.unwrap_or_default();
        return Ok(DataSet::with_columns(names));
    }

    let col_names = match col_names {
        Some(names) if !names.is_empty() => names,
        _ => chunks[0].col_names(),
    };

    let mut collector = LocalChunkCollector::new(col_names.clone());
    let expected_cols = col_names.len();
    for mut chunk in chunks {
        if chunk.num_columns() != expected_cols {
            return Err(QueryError::execution(format!(
                "Chunk has {} columns, expected {}",
                chunk.num_columns(),
                expected_cols
            )));
        }
        // Single terminal expansion point (selection + multiplicity aware).
        collector.push_chunk(&mut chunk);
    }

    let (all_rows, _) = collector.into_rows();
    Ok(DataSet::from_rows(all_rows, col_names))
}

/// Convert streaming execution result to ExecutionResult
///
/// # Arguments
/// * `chunks` - Result chunks from StreamingExecutionEngine
/// * `col_names` - Optional column names override
///
/// # Returns
/// * ExecutionResult with merged DataSet
pub fn chunks_to_execution_result(
    chunks: Vec<DataChunk>,
    col_names: Option<Vec<String>>,
) -> Result<ExecutionResult, QueryError> {
    let dataset = convert_chunks_to_dataset(chunks, col_names)?;
    Ok(ExecutionResult::DataSet { data: dataset })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::streaming::chunk::DataChunk;
    use graphdb_core::Value;

    fn create_test_chunk(rows: Vec<Vec<Value>>) -> DataChunk {
        DataChunk::from_rows(rows)
    }

    #[test]
    fn test_empty_chunks() {
        let result = convert_chunks_to_dataset(vec![], None);
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert!(ds.is_empty());
        assert_eq!(ds.col_count(), 0);
    }

    #[test]
    fn test_single_chunk() {
        let rows = vec![
            vec![Value::Int(1), Value::string("a")],
            vec![Value::Int(2), Value::string("b")],
        ];
        let chunk = create_test_chunk(rows);
        let result = convert_chunks_to_dataset(vec![chunk], None);
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert_eq!(ds.row_count(), 2);
        assert_eq!(ds.col_count(), 2);
    }

    #[test]
    fn test_multiple_chunks() {
        let chunk1 = create_test_chunk(vec![
            vec![Value::Int(1), Value::string("a")],
            vec![Value::Int(2), Value::string("b")],
        ]);
        let chunk2 = create_test_chunk(vec![
            vec![Value::Int(3), Value::string("c")],
            vec![Value::Int(4), Value::string("d")],
        ]);

        let result = convert_chunks_to_dataset(vec![chunk1, chunk2], None);
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert_eq!(ds.row_count(), 4);
        assert_eq!(ds.col_count(), 2);
    }

    #[test]
    fn test_execution_result_conversion() {
        let chunk = create_test_chunk(vec![vec![Value::Int(42)]]);
        let result = chunks_to_execution_result(vec![chunk], None);
        assert!(result.is_ok());
        if let ExecutionResult::DataSet { data: ds, .. } = result.unwrap() {
            assert_eq!(ds.row_count(), 1);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_custom_col_names() {
        let rows = vec![vec![Value::Int(1), Value::string("test")]];
        let chunk = create_test_chunk(rows);
        let col_names = vec!["id".to_string(), "name".to_string()];
        let result = convert_chunks_to_dataset(vec![chunk], Some(col_names.clone()));
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert_eq!(ds.col_names, col_names);
    }
}

//! Utility functions for converting streaming results to standard formats.

use std::sync::Arc;

use super::chunk::DataChunk;
use super::chunk::LocalChunkCollector;
use super::spill::SpillManager;
use crate::executor::base::ExecutionResult;
use graphdb_core::error::QueryError;
use graphdb_core::DataSet;

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
    convert_chunks_to_dataset_with_spill(chunks, col_names, None).map(|(dataset, _, _, _)| dataset)
}

/// Convert chunks to a `DataSet`, spilling through the manager when present.
///
/// Small results stay fully in memory; large results spill automatically once
/// the collector threshold is exceeded. Returns the dataset plus the spilled
/// row/byte/run counts so callers can record observability counters.
pub fn convert_chunks_to_dataset_with_spill(
    chunks: Vec<DataChunk>,
    col_names: Option<Vec<String>>,
    spill_manager: Option<Arc<SpillManager>>,
) -> Result<(DataSet, u64, u64, u64), QueryError> {
    if chunks.is_empty() {
        let names = col_names.unwrap_or_default();
        return Ok((DataSet::with_columns(names), 0, 0, 0));
    }

    let col_names = match col_names {
        Some(names) if !names.is_empty() => names,
        _ => chunks[0].col_names(),
    };

    let mut collector = LocalChunkCollector::new(col_names.clone());
    if let Some(manager) = spill_manager {
        collector.attach_spill_manager(manager);
    }
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
        collector.push_chunk(&mut chunk)?;
    }

    collector.finish_spill()?;
    let spilled_rows = collector.spilled_rows();
    let spilled_bytes = collector.spilled_bytes();
    let spilled_runs = collector.spilled_run_count() as u64;
    let (all_rows, _) = collector.into_rows()?;
    Ok((
        DataSet::from_rows(all_rows, col_names),
        spilled_rows,
        spilled_bytes,
        spilled_runs,
    ))
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
    chunks_to_execution_result_with_spill(chunks, col_names, None)
}

/// Convert streaming execution result to ExecutionResult, spilling through
/// the manager when present.
///
/// Large results spill automatically once the collector threshold is
/// exceeded; callers with a spill manager should prefer this over
/// [`chunks_to_execution_result`].
pub fn chunks_to_execution_result_with_spill(
    chunks: Vec<DataChunk>,
    col_names: Option<Vec<String>>,
    spill_manager: Option<Arc<SpillManager>>,
) -> Result<ExecutionResult, QueryError> {
    let (dataset, _, _, _) =
        convert_chunks_to_dataset_with_spill(chunks, col_names, spill_manager)?;
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

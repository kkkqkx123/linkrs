//! Streaming Executor Factory Integration
//!
//! Provides utilities to create and execute streaming queries
//! from execution plans without requiring StorageClient modifications.
//!
//! This module also handles conversion of streaming execution results
//! (Vec<DataChunk>) into the standard ExecutionResult format.

use super::builder::StreamingExecutorBuilder;
use super::chunk::DataChunk;
use super::engine::StreamingExecutionEngine;
use crate::core::error::QueryError;
use crate::query::data_set::DataSet;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::base::ExecutionResult;
use crate::query::planning::plan::PlanNodeEnum;

/// High-level streaming execution interface
///
/// Encapsulates the creation and execution of streaming queries
/// from a plan node without exposing executor complexity.
pub struct StreamingQueryExecutor {
    engine: Option<StreamingExecutionEngine>,
    col_names: Option<Vec<String>>,
}

impl StreamingQueryExecutor {
    /// Create a new streaming executor
    pub fn new() -> Self {
        Self {
            engine: None,
            col_names: None,
        }
    }

    /// Build executor from a plan node
    ///
    /// # Arguments
    /// * `plan_node` - Root node of the execution plan
    /// * `context` - Execution context (for expression evaluation)
    ///
    /// # Returns
    /// * QueryError if the plan cannot be converted to streaming execution
    pub fn from_plan_node(
        &mut self,
        plan_node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let executor = StreamingExecutorBuilder::from_plan_node(plan_node, context)?;

        let mut engine = StreamingExecutionEngine::new();
        engine.register_executor(0, executor);

        self.engine = Some(engine);
        Ok(())
    }

    /// Execute the streaming query
    ///
    /// # Returns
    /// * ExecutionResult with DataSet or error
    pub fn execute(&mut self) -> Result<ExecutionResult, QueryError> {
        let engine = self
            .engine
            .as_mut()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;

        let chunks = engine.execute()?;
        chunks_to_execution_result(chunks, self.col_names.clone())
    }

    /// Set optional column names for result formatting
    pub fn set_col_names(&mut self, col_names: Vec<String>) {
        self.col_names = Some(col_names);
    }
}

impl Default for StreamingQueryExecutor {
    fn default() -> Self {
        Self::new()
    }
}

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

    // Get column names from provided arg or first chunk's schema
    let col_names = if let Some(names) = col_names {
        names
    } else {
        chunks[0].col_names()
    };

    // Validate all chunks have same column count
    let expected_cols = col_names.len();
    for (i, chunk) in chunks.iter().enumerate() {
        if chunk.num_columns() != expected_cols {
            return Err(QueryError::execution(format!(
                "Chunk {} has {} columns, expected {}",
                i,
                chunk.num_columns(),
                expected_cols
            )));
        }
    }

    // Merge all rows from all chunks
    let mut all_rows = Vec::new();
    for chunk in chunks {
        for row in chunk.rows {
            all_rows.push(row);
        }
    }

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
    Ok(ExecutionResult::DataSet(dataset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_creation() {
        let executor = StreamingQueryExecutor::new();
        assert!(executor.engine.is_none());
    }

    #[test]
    fn test_col_names_setting() {
        let mut executor = StreamingQueryExecutor::new();
        let col_names = vec!["id".to_string(), "name".to_string()];
        executor.set_col_names(col_names.clone());
        assert_eq!(executor.col_names, Some(col_names));
    }

    // Tests for convert_chunks_to_dataset and chunks_to_execution_result
    use crate::core::Value;

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
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
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
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ]);
        let chunk2 = create_test_chunk(vec![
            vec![Value::Int(3), Value::String("c".to_string())],
            vec![Value::Int(4), Value::String("d".to_string())],
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
        if let ExecutionResult::DataSet(ds) = result.unwrap() {
            assert_eq!(ds.row_count(), 1);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_custom_col_names() {
        let rows = vec![vec![Value::Int(1), Value::String("test".to_string())]];
        let chunk = create_test_chunk(rows);
        let col_names = vec!["id".to_string(), "name".to_string()];
        let result = convert_chunks_to_dataset(vec![chunk], Some(col_names.clone()));
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert_eq!(ds.col_names, col_names);
    }
}

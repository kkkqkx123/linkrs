//! Implementation of the Union executor
//!
//! Implement the UNION operation to merge two datasets and remove duplicate rows.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::query::executor::base::{DBResult, ExecutionResult, Executor};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;

use super::base::SetExecutor;

/// Union Executor
///
/// Implement the UNION operation to merge two datasets.
/// When `distinct` is true, duplicate rows are removed (SQL UNION).
/// When `distinct` is false, all rows are kept (SQL UNION ALL).
#[derive(Debug)]
pub struct UnionExecutor<S: StorageClient> {
    pub set_executor: SetExecutor<S>,
    pub distinct: bool,
}

impl<S: StorageClient> UnionExecutor<S> {
    /// Create a new Union executor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        left_input_var: String,
        right_input_var: String,
        distinct: bool,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            set_executor: SetExecutor::new(
                id,
                "UnionExecutor".to_string(),
                storage,
                left_input_var,
                right_input_var,
                expr_context,
            ),
            distinct,
        }
    }

    /// Create a new Union executor with a shared execution context.
    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        left_input_var: String,
        right_input_var: String,
        distinct: bool,
        context: crate::query::executor::base::ExecutionContext,
    ) -> Self {
        Self {
            set_executor: SetExecutor::with_context(
                id,
                "UnionExecutor".to_string(),
                storage,
                left_input_var,
                right_input_var,
                context,
            ),
            distinct,
        }
    }

    /// Perform the UNION operation
    ///
    /// Algorithm steps:
    /// 1. Obtain the two input datasets on the left and right sides.
    /// 2. Verify whether the column names are consistent.
    /// 3. Merge all rows from the two datasets.
    /// 4. Remove duplicate rows if `distinct` is true (UNION).
    fn execute_union(&mut self) -> Result<DataSet, QueryError> {
        // Obtaining the left and right input datasets
        let left_dataset = self.set_executor.get_left_input_data()?;
        let right_dataset = self.set_executor.get_right_input_data()?;

        // Check the validity of the input dataset.
        self.set_executor
            .check_input_data_sets(&left_dataset, &right_dataset)?;

        // Merge two datasets
        let combined_dataset = SetExecutor::<S>::concat_datasets(left_dataset, right_dataset);

        // Remove duplicate rows only for UNION (not UNION ALL).
        let rows = if self.distinct {
            SetExecutor::<S>::dedup_rows(combined_dataset.rows)
        } else {
            combined_dataset.rows
        };

        // Constructing the resulting dataset
        let result_dataset = DataSet {
            col_names: self.set_executor.get_col_names().clone(),
            rows,
        };

        Ok(result_dataset)
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for UnionExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let dataset = self
            .execute_union()
            .map_err(|e| crate::core::error::DBError::query(e.to_string()))?;

        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> DBResult<()> {
        self.set_executor.open()
    }

    fn close(&mut self) -> DBResult<()> {
        self.set_executor.close()
    }

    fn is_open(&self) -> bool {
        self.set_executor.is_open()
    }

    fn id(&self) -> i64 {
        self.set_executor.id()
    }

    fn name(&self) -> &str {
        self.set_executor.name()
    }

    fn description(&self) -> &str {
        self.set_executor.description()
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.set_executor.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.set_executor.stats_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::query::DataSet;
    use ExpressionAnalysisContext;

    // Create a storage engine for testing purposes.
    fn create_test_storage() -> Arc<RwLock<crate::storage::MockStorage>> {
        let storage = crate::storage::MockStorage::new().expect("Failed to create test storage");
        Arc::new(RwLock::new(storage))
    }

    #[test]
    fn test_union_basic() {
        let storage = create_test_storage();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = UnionExecutor::new(
            1,
            storage,
            "left_input".to_string(),
            "right_input".to_string(),
            true,
            expr_context,
        );

        // Set up the test data
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(2), Value::String("Bob".to_string())], // Duplicate rows
                vec![Value::Int(3), Value::String("Charlie".to_string())],
            ],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "left_input".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "right_input".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // Perform the UNION operation
        let result = executor.execute();

        // Verification results
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // There should be 3 unique rows (after deduplication).
            assert_eq!(dataset.row_count(), 3);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_union_empty_datasets() {
        let storage = create_test_storage();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = UnionExecutor::new(
            2,
            storage,
            "empty_left".to_string(),
            "empty_right".to_string(),
            true,
            expr_context,
        );

        // Create two empty datasets.
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "empty_left".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "empty_right".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // Testing the UNION operation on an empty dataset
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            assert_eq!(dataset.row_count(), 0);
        }
    }

    #[test]
    fn test_union_mismatched_columns() {
        let storage = create_test_storage();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = UnionExecutor::new(
            3,
            storage,
            "left_mismatch".to_string(),
            "right_mismatch".to_string(),
            true,
            expr_context,
        );

        // A dataset with column names that do not match the specified values
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec![Value::Int(1), Value::String("Alice".to_string())]],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "title".to_string()], // Different column names
            rows: vec![vec![Value::Int(2), Value::String("Mr".to_string())]],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "left_mismatch".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "right_mismatch".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // The execution should fail.
        let result = executor.execute();
        assert!(result.is_err());

        if let Err(err) = result {
            assert_eq!(err.kind(), crate::core::error::ErrorKind::Query);
            assert!(err.message().contains("column name mismatch"));
        } else {
            panic!("Error: The expected column names do not match.");
        }
    }
}

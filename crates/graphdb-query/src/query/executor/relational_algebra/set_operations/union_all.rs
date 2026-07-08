//! Implementation of the UnionAll executor
//!
//! Implement the UNION ALL operation to merge two datasets while retaining duplicate rows.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::query::executor::base::{DBResult, ExecutionResult, Executor};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;

use super::base::SetExecutor;

/// The UnionAll executor
///
/// Implement the UNION ALL operation to merge two datasets while retaining duplicate rows.
/// Something similar to SQL’s UNION ALL
#[derive(Debug)]
pub struct UnionAllExecutor<S: StorageClient> {
    pub set_executor: SetExecutor<S>,
}

impl<S: StorageClient> UnionAllExecutor<S> {
    /// Create a new UnionAll executor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        left_input_var: String,
        right_input_var: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            set_executor: SetExecutor::new(
                id,
                "UnionAllExecutor".to_string(),
                storage,
                left_input_var,
                right_input_var,
                expr_context,
            ),
        }
    }

    /// Perform the UNION ALL operation
    ///
    /// Algorithm steps:
    /// 1. Obtain the two input datasets on the left and right.
    /// 2. Verify whether the column names are consistent.
    fn execute_union_all(&mut self) -> Result<DataSet, QueryError> {
        // Obtain the left and right input datasets
        let left_dataset = self.set_executor.get_left_input_data()?;
        let right_dataset = self.set_executor.get_right_input_data()?;

        // Check the validity of the input dataset.
        self.set_executor
            .check_input_data_sets(&left_dataset, &right_dataset)?;

        // Merge two datasets (without removing duplicates)
        let result_dataset = SetExecutor::<S>::concat_datasets(left_dataset, right_dataset);

        Ok(result_dataset)
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for UnionAllExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let dataset = self
            .execute_union_all()
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

    // Create a storage engine for testing purposes.
    fn create_test_storage() -> Arc<RwLock<crate::storage::MockStorage>> {
        let storage = crate::storage::MockStorage::new().expect("Failed to create test storage");
        Arc::new(RwLock::new(storage))
    }

    fn create_test_context() -> Arc<ExpressionAnalysisContext> {
        Arc::new(ExpressionAnalysisContext::new())
    }

    #[test]
    fn test_union_all_basic() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = UnionAllExecutor::new(
            1,
            storage,
            "left_input".to_string(),
            "right_input".to_string(),
            context,
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

        // Perform the UNION ALL operation
        let result = executor.execute();

        // Verification results
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // All rows should be included: 2 rows from left + 2 rows from right = 4 rows
            assert_eq!(dataset.rows.len(), 4);
        } else {
            panic!("Expected DataSet results");
        }
    }

    #[test]
    fn test_union_all_empty_left() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = UnionAllExecutor::new(
            2,
            storage,
            "empty_left".to_string(),
            "right_input".to_string(),
            context,
        );

        // Set an empty left dataset and a non-empty right dataset.
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
            ],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "empty_left".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "right_input".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // The UNION ALL operation is being tested on a left dataset that is empty.
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // Only the right dataset rows should be included (2 rows)
            assert_eq!(dataset.rows.len(), 2);
        }
    }

    #[test]
    fn test_union_all_empty_right() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = UnionAllExecutor::new(
            3,
            storage,
            "left_input".to_string(),
            "empty_right".to_string(),
            context,
        );

        // Set a non-empty left dataset and an empty right dataset.
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "left_input".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "empty_right".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // The UNION ALL operation on the right dataset, which is empty, results in no data being returned.
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // Only the content from the left dataset should be included.
            assert_eq!(dataset.rows.len(), 2);
        }
    }

    #[test]
    fn test_union_all_both_empty() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = UnionAllExecutor::new(
            4,
            storage,
            "empty_left".to_string(),
            "empty_right".to_string(),
            context,
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

        // Test the UNION ALL operation when both datasets are empty.
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_union_all_mismatched_columns() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = UnionAllExecutor::new(
            5,
            storage,
            "left_mismatch".to_string(),
            "right_mismatch".to_string(),
            context,
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

    #[test]
    fn test_union_all_preserve_duplicates() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = UnionAllExecutor::new(
            6,
            storage,
            "left_dup".to_string(),
            "right_dup".to_string(),
            context,
        );

        // Setting up a dataset that contains duplicate rows
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "value".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("same".to_string())],
                vec![Value::Int(1), Value::String("same".to_string())], // Duplicate rows in the left dataset
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "value".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("same".to_string())], // Rows that are repeated in the left dataset
                vec![Value::Int(2), Value::String("different".to_string())],
            ],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "left_dup".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "right_dup".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // Perform the UNION ALL operation
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // All duplicate rows should be retained.
            // The left dataset contains 2 rows (one of which is repeated), and the right dataset also contains 2 rows. In total, there are 4 rows.
            assert_eq!(dataset.rows.len(), 4);
        } else {
            panic!("Expected DataSet results");
        }
    }
}

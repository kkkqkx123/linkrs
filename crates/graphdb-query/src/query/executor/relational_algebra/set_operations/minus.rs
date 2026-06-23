//! Implementation of the Minus actuator
//!
//! Implement the MINUS operation to return the rows that exist in the left dataset but not in the right dataset.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::query::QueryError;
use crate::query::executor::base::{DBResult, ExecutionResult, Executor};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

use super::base::SetExecutor;

/// Minus Actuator
///
/// Implement the MINUS operation to return the rows that exist in the left dataset but not in the right dataset.
/// Something similar to the SQL operators EXCEPT or MINUS
#[derive(Debug)]
pub struct MinusExecutor<S: StorageClient> {
    pub set_executor: SetExecutor<S>,
}

impl<S: StorageClient> MinusExecutor<S> {
    /// Create a newMinus executor.
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
                "MinusExecutor".to_string(),
                storage,
                left_input_var,
                right_input_var,
                expr_context,
            ),
        }
    }

    /// Create a new Minus executor with a shared execution context.
    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        left_input_var: String,
        right_input_var: String,
        context: crate::query::executor::base::ExecutionContext,
    ) -> Self {
        Self {
            set_executor: SetExecutor::with_context(
                id,
                "MinusExecutor".to_string(),
                storage,
                left_input_var,
                right_input_var,
                context,
            ),
        }
    }

    /// Perform the MINUS operation
    ///
    /// Algorithm steps:
    /// 1. Obtain the two input datasets on the left and right.
    /// 2. Verify whether the column names are consistent.
    /// 3. Create a set of row hashes for the right dataset.
    fn execute_minus(&mut self) -> Result<DataSet, QueryError> {
        // Obtain the left and right input datasets
        let left_dataset = self.set_executor.get_left_input_data()?;
        let right_dataset = self.set_executor.get_right_input_data()?;

        // Check the validity of the input dataset.
        self.set_executor
            .check_input_data_sets(&left_dataset, &right_dataset)?;

        // If the right dataset is empty, return the left dataset directly.
        if right_dataset.rows.is_empty() {
            return Ok(DataSet {
                col_names: self.set_executor.get_col_names().clone(),
                rows: left_dataset.rows,
            });
        }

        // If the left dataset is empty, return an empty result directly.
        if left_dataset.rows.is_empty() {
            return Ok(DataSet {
                col_names: self.set_executor.get_col_names().clone(),
                rows: Vec::new(),
            });
        }

        // Creating a set of row hashes for the right dataset is used for quick searches.
        let right_row_set = SetExecutor::<S>::create_row_set(&right_dataset.rows);

        // Identify the rows that exist in the left dataset but not in the right dataset.
        let mut minus_rows = Vec::new();

        for left_row in &left_dataset.rows {
            if !SetExecutor::<S>::row_in_set(left_row, &right_row_set) {
                minus_rows.push(left_row.to_vec());
            }
        }

        // Constructing the resulting dataset
        let result_dataset = DataSet {
            col_names: self.set_executor.get_col_names().clone(),
            rows: minus_rows,
        };

        Ok(result_dataset)
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for MinusExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let dataset = self
            .execute_minus()
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
    fn test_minus_basic() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
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
                vec![Value::Int(3), Value::String("Charlie".to_string())],
                vec![Value::Int(4), Value::String("David".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(2), Value::String("Bob".to_string())], // Rows to be excluded
                vec![Value::Int(4), Value::String("David".to_string())], // Rows to be excluded
                vec![Value::Int(5), Value::String("Eve".to_string())], // Rows that do not exist in the left dataset
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

        // Perform the MINUS operation
        let result = executor.execute();

        // Verification results
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // Only Alice and Charlie should be included (Bob and David are excluded).
            assert_eq!(dataset.rows.len(), 2);
        } else {
            panic!("Expected DataSet results");
        }
    }

    #[test]
    fn test_minus_no_overlap() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
            2,
            storage,
            "left_no_overlap".to_string(),
            "right_no_overlap".to_string(),
            context,
        );

        // Set up datasets that do not overlap.
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
                vec![Value::Int(3), Value::String("Charlie".to_string())],
                vec![Value::Int(4), Value::String("David".to_string())],
            ],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "left_no_overlap".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "right_no_overlap".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // Perform the MINUS operation
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // There is no overlap; therefore, the entire left dataset should be returned.
            assert_eq!(dataset.rows.len(), 2);
        }
    }

    #[test]
    fn test_minus_all_overlap() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
            3,
            storage,
            "left_all_overlap".to_string(),
            "right_all_overlap".to_string(),
            context,
        );

        // Setting up a dataset with complete overlap
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
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
            ],
        };

        // Set the dataset in the executor context.
        executor.set_executor.base_mut().context.set_result(
            "left_all_overlap".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "right_all_overlap".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // Perform the MINUS operation
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // The overlap is complete; therefore, the result should be empty.
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_minus_empty_left() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
            4,
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

        // The test for the MINUS case where the left dataset is empty is completed.
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // The left dataset is empty; therefore, the result should also be empty.
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_minus_empty_right() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
            5,
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

        // The test for the MINUS case where the right dataset is empty is completed.
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // The right dataset is empty; therefore, the entire left dataset should be returned.
            assert_eq!(dataset.rows.len(), 2);
        }
    }

    #[test]
    fn test_minus_both_empty() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
            6,
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

        // Testing the MINUS case where both datasets are empty.
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_minus_with_duplicates() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
            7,
            storage,
            "left_dup".to_string(),
            "right_dup".to_string(),
            context,
        );

        // Setting up a dataset that contains duplicate rows
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "value".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("common".to_string())],
                vec![Value::Int(1), Value::String("common".to_string())], // Duplicate rows in the left dataset
                vec![Value::Int(2), Value::String("unique".to_string())],
                vec![Value::Int(3), Value::String("another".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "value".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("common".to_string())],
                vec![Value::Int(3), Value::String("another".to_string())],
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

        // Perform the MINUS operation
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // Only unique rows should be included; the terms "common" and "another" should be excluded.
            // Left has [2, "unique"] and [3, "another"], right has [1, "common"] and [3, "another"]
            // Result should be [2, "unique"] (1 row)
            assert_eq!(dataset.rows.len(), 1);
        } else {
            panic!("Expected DataSet results");
        }
    }

    #[test]
    fn test_minus_mismatched_columns() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = MinusExecutor::new(
            8,
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
            rows: vec![vec![Value::Int(1), Value::String("Ms".to_string())]],
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

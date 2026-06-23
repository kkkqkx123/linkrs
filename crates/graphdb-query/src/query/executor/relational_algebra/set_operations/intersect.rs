//! Implementation of the Intersect executor
//!
//! Implement the INTERSECT operation to return the intersection of the two datasets (rows that exist only in both datasets).

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::query::QueryError;
use crate::query::executor::base::{DBResult, ExecutionResult, Executor};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

use super::base::SetExecutor;

/// Intersect executor
///
/// Implement the INTERSECT operation to return the intersection of the two datasets.
/// Return only the rows that exist in both the left and right datasets.
#[derive(Debug)]
pub struct IntersectExecutor<S: StorageClient> {
    pub set_executor: SetExecutor<S>,
}

impl<S: StorageClient> IntersectExecutor<S> {
    /// Create a new Intersect executor.
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
                "IntersectExecutor".to_string(),
                storage,
                left_input_var,
                right_input_var,
                expr_context,
            ),
        }
    }

    /// Create a new Intersect executor with a shared execution context.
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
                "IntersectExecutor".to_string(),
                storage,
                left_input_var,
                right_input_var,
                context,
            ),
        }
    }

    /// Perform the INTERSECT operation
    ///
    /// Algorithm steps:
    /// 1. Obtain the two input datasets on the left and right.
    /// 2. Verify whether the column names are consistent.
    /// 3. Create a set of row hashes for the right dataset.
    fn execute_intersect(&mut self) -> Result<DataSet, QueryError> {
        // Obtain the left and right input datasets
        let left_dataset = self.set_executor.get_left_input_data()?;
        let right_dataset = self.set_executor.get_right_input_data()?;

        // Check the validity of the input dataset.
        self.set_executor
            .check_input_data_sets(&left_dataset, &right_dataset)?;

        // If either dataset is empty, return an empty result directly.
        if left_dataset.rows.is_empty() || right_dataset.rows.is_empty() {
            return Ok(DataSet {
                col_names: self.set_executor.get_col_names().clone(),
                rows: Vec::new(),
            });
        }

        // Creating a set of row hashes for the right dataset is used for quick searches.
        let right_row_set = SetExecutor::<S>::create_row_set(&right_dataset.rows);

        // Identify the rows that exist in both the left and the right datasets.
        let mut intersect_rows = Vec::new();

        for left_row in &left_dataset.rows {
            if SetExecutor::<S>::row_in_set(left_row, &right_row_set) {
                intersect_rows.push(left_row.to_vec());
            }
        }

        // Constructing the resulting dataset
        let result_dataset = DataSet {
            col_names: self.set_executor.get_col_names().clone(),
            rows: intersect_rows,
        };

        Ok(result_dataset)
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for IntersectExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let dataset = self
            .execute_intersect()
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
    fn test_intersect_basic() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = IntersectExecutor::new(
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
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(2), Value::String("Bob".to_string())], // Act together
                vec![Value::Int(3), Value::String("Charlie".to_string())], // Act together
                vec![Value::Int(4), Value::String("David".to_string())],
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

        // Perform the INTERSECT operation
        let result = executor.execute();

        // Verification results
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // There should be 2 common lines: Bob and Charlie.
            assert_eq!(dataset.rows.len(), 2);
        } else {
            panic!("Expected DataSet results");
        }
    }

    #[test]
    fn test_intersect_no_common_rows() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = IntersectExecutor::new(
            2,
            storage,
            "left_no_common".to_string(),
            "right_no_common".to_string(),
            context,
        );

        // Setting a dataset for which there are no common rows
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
            "left_no_common".to_string(),
            ExecutionResult::DataSet(left_dataset),
        );
        executor.set_executor.base_mut().context.set_result(
            "right_no_common".to_string(),
            ExecutionResult::DataSet(right_dataset),
        );

        // Perform the INTERSECT operation
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // There should not be any common lines (or elements) between them.
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_intersect_empty_left() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = IntersectExecutor::new(
            3,
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

        // The INTERSECT operation with the left dataset being empty results in no output (i.e., no results are returned).
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // The left dataset is empty; therefore, the intersection should also be empty.
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_intersect_empty_right() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = IntersectExecutor::new(
            4,
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

        // The INTERSECT operation with the right dataset being empty results in no output (i.e., no results are returned).
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // The right dataset is empty; therefore, the intersection should also be empty.
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_intersect_both_empty() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = IntersectExecutor::new(
            5,
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

        // Test the INTERSECT operation for two datasets that are both empty.
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            assert_eq!(dataset.rows.len(), 0);
        }
    }

    #[test]
    fn test_intersect_with_duplicates() {
        let storage = create_test_storage();
        let context = create_test_context();
        let mut executor = IntersectExecutor::new(
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
                vec![Value::Int(1), Value::String("common".to_string())],
                vec![Value::Int(1), Value::String("common".to_string())], // Duplicate rows in the left dataset
                vec![Value::Int(2), Value::String("unique".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "value".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("common".to_string())],
                vec![Value::Int(3), Value::String("different".to_string())],
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

        // Perform the INTERSECT operation
        let result = executor.execute();
        assert!(result.is_ok());

        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            // The common rows should be included, and the duplicate rows from the left dataset should be retained.
            // Left has 2 copies of [1, "common"], right has 1 copy
            // Result should have 2 rows (the duplicates from left)
            assert_eq!(dataset.rows.len(), 2);
        } else {
            panic!("Expected DataSet results");
        }
    }

    #[test]
    fn test_intersect_mismatched_columns() {
        let storage = create_test_storage();
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = IntersectExecutor::new(
            7,
            storage,
            "left_mismatch".to_string(),
            "right_mismatch".to_string(),
            expr_context,
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

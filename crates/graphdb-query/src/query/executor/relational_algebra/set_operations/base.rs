//! Collection Operation Executor Base Class
//!
//! Provide the general functions and interfaces of all set operation executors.

use parking_lot::RwLock;
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::Arc;

use crate::core::Value;
use crate::query::executor::{BaseExecutor, ExecutionResult};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;

/// Collection Operation Executor Base Class
///
/// Provide general functionality for all set operations (Union, Intersect, Subtract, etc.)
#[derive(Debug)]
pub struct SetExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    left_input_var: String,
    right_input_var: String,
    col_names: Vec<String>,
}

impl<S: StorageClient> SetExecutor<S> {
    /// Create a new collection operation executor.
    pub fn new(
        id: i64,
        name: String,
        storage: Arc<RwLock<S>>,
        left_input_var: String,
        right_input_var: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, name, storage, expr_context),
            left_input_var,
            right_input_var,
            col_names: Vec::new(),
        }
    }

    /// Create a new set operation executor with a shared execution context.
    pub fn with_context(
        id: i64,
        name: String,
        storage: Arc<RwLock<S>>,
        left_input_var: String,
        right_input_var: String,
        context: crate::query::executor::base::ExecutionContext,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context(id, name, storage, context),
            left_input_var,
            right_input_var,
            col_names: Vec::new(),
        }
    }

    /// Obtain a variable reference to the internal base executor.
    pub fn base_mut(&mut self) -> &mut BaseExecutor<S> {
        &mut self.base
    }

    /// Obtain an immutable reference to the internal base executor.
    pub fn base(&self) -> &BaseExecutor<S> {
        &self.base
    }

    /// Obtain the left input dataset
    pub fn get_left_input_data(&self) -> Result<DataSet, QueryError> {
        match self.base.context.get_result(&self.left_input_var) {
            Some(ExecutionResult::DataSet(dataset)) => Ok(dataset.clone()),
            Some(_result) => {
                // Results of other types need to be converted into a DataSet.
                Err(QueryError::execution(format!(
                    "Left input variable {} is not a valid data set",
                    self.left_input_var
                )))
            }
            None => Err(QueryError::execution(format!(
                "Left input variable {} does not exist",
                self.left_input_var
            ))),
        }
    }

    /// Obtain the right input dataset
    pub fn get_right_input_data(&self) -> Result<DataSet, QueryError> {
        match self.base.context.get_result(&self.right_input_var) {
            Some(ExecutionResult::DataSet(dataset)) => Ok(dataset.clone()),
            Some(_result) => {
                // Results of other types need to be converted into a DataSet.
                Err(QueryError::execution(format!(
                    "Right input variable {} is not a valid data set",
                    self.right_input_var
                )))
            }
            None => Err(QueryError::execution(format!(
                "Right input variable {} does not exist",
                self.right_input_var
            ))),
        }
    }

    /// Check the validity of the input dataset.
    ///
    /// Verify whether the column names in the two input datasets are consistent.
    pub fn check_input_data_sets(
        &mut self,
        left: &DataSet,
        right: &DataSet,
    ) -> Result<(), QueryError> {
        if left.col_names != right.col_names {
            let left_cols = left.col_names.join(",");
            let right_cols = right.col_names.join(",");
            return Err(QueryError::execution(format!(
                "Dataset column name mismatch: <{}> vs <{}>",
                left_cols, right_cols
            )));
        }

        // Save the column names for later use.
        self.col_names = left.col_names.clone();
        Ok(())
    }

    /// Obtain the column names
    pub fn get_col_names(&self) -> &Vec<String> {
        &self.col_names
    }

    /// Set column names
    pub fn set_col_names(&mut self, col_names: Vec<String>) {
        self.col_names = col_names;
    }

    /// The hash values of the created rows are used for deduplication and comparison.
    pub fn hash_row(row: &[Value]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;

        let mut hasher = DefaultHasher::new();
        for value in row {
            value.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Creating a set of rows facilitates quick searches.
    pub fn create_row_set(rows: &[Vec<Value>]) -> HashSet<u64> {
        let mut row_set = HashSet::new();
        for row in rows {
            row_set.insert(Self::hash_row(row));
        }
        row_set
    }

    /// Check whether the row is in the set.
    pub fn row_in_set(row: &[Value], row_set: &HashSet<u64>) -> bool {
        let hash = Self::hash_row(row);
        row_set.contains(&hash)
    }

    /// Rows from the deduplicated dataset
    pub fn dedup_rows(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        for row in rows {
            let hash = Self::hash_row(&row);
            if seen.insert(hash) {
                result.push(row);
            }
        }

        result
    }

    /// Merge the rows of two datasets (without duplicates)
    pub fn concat_datasets(left: DataSet, right: DataSet) -> DataSet {
        let mut rows = left.rows;
        rows.extend(right.rows);

        DataSet {
            col_names: left.col_names,
            rows,
        }
    }
}

impl<S: StorageClient + Send + 'static> crate::query::executor::base::Executor<S>
    for SetExecutor<S>
{
    fn execute(
        &mut self,
    ) -> crate::query::executor::base::DBResult<crate::query::executor::base::ExecutionResult> {
        Err(crate::core::error::DBError::query(
            "SetExecutor is an abstract base class and cannot be executed directly".to_string(),
        ))
    }

    fn open(&mut self) -> crate::query::executor::base::DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> crate::query::executor::base::DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        "Set executor base class"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;

    #[test]
    fn test_hash_row() {
        let row1 = vec![Value::Int(1), Value::String("test".to_string())];
        let row2 = vec![Value::Int(1), Value::String("test".to_string())];
        let row3 = vec![Value::Int(2), Value::String("test".to_string())];

        assert_eq!(
            SetExecutor::<crate::storage::GraphStorage>::hash_row(&row1),
            SetExecutor::<crate::storage::GraphStorage>::hash_row(&row2)
        );
        assert_ne!(
            SetExecutor::<crate::storage::GraphStorage>::hash_row(&row1),
            SetExecutor::<crate::storage::GraphStorage>::hash_row(&row3)
        );
    }

    #[test]
    fn test_create_row_set() {
        let rows = vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
            vec![Value::Int(1), Value::String("a".to_string())], // Duplicate rows
        ];

        let row_set = SetExecutor::<crate::storage::GraphStorage>::create_row_set(&rows);
        assert_eq!(row_set.len(), 2); // There should only be 2 unique hash values.
    }

    #[test]
    fn test_dedup_rows() {
        let rows = vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
            vec![Value::Int(1), Value::String("a".to_string())], // Duplicate rows
            vec![Value::Int(3), Value::String("c".to_string())],
        ];

        let deduped = SetExecutor::<crate::storage::GraphStorage>::dedup_rows(rows);
        assert_eq!(deduped.len(), 3); // The text should be deduplicated to three lines.
    }

    #[test]
    fn test_concat_datasets() {
        let left = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec![Value::Int(1), Value::String("Alice".to_string())]],
        };

        let right = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec![Value::Int(2), Value::String("Bob".to_string())]],
        };

        let result = SetExecutor::<crate::storage::GraphStorage>::concat_datasets(left, right);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.col_names.len(), 2);
    }
}

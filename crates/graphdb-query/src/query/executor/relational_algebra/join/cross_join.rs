//! Implementation of a Cartesian product actuator
//!
//! Implement the Cartesian product (cross join) algorithm, supporting the joining of multiple tables.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::Value;
use crate::query::executor::base::JoinConfig;
use crate::query::executor::base::{ExecutionResult, Executor};
use crate::query::executor::relational_algebra::join::base_join::BaseJoinExecutor;
use crate::query::executor::relational_algebra::join::ExpressionContextStruct;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;

/// Cartesian product actuator
pub struct CrossJoinExecutor<S: StorageClient> {
    base_executor: BaseJoinExecutor<S>,
    /// List of input variables (multiple tables are supported)
    input_vars: Vec<String>,
}

// Manual Debug implementation for CrossJoinExecutor to avoid requiring Debug trait for BaseJoinExecutor
impl<S: StorageClient> std::fmt::Debug for CrossJoinExecutor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrossJoinExecutor")
            .field("base_executor", &"BaseJoinExecutor<S>")
            .field("input_vars", &self.input_vars)
            .finish()
    }
}

impl<S: StorageClient> CrossJoinExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        input_vars: Vec<String>,
        col_names: Vec<String>,
        expr_context: Arc<ExpressionContextStruct>,
    ) -> Self {
        Self {
            base_executor: BaseJoinExecutor::new(
                id,
                storage,
                expr_context,
                JoinConfig {
                    left_var: String::new(),  // Left variable (not used)
                    right_var: String::new(), // Right variable (not used)
                    hash_keys: Vec::new(),    // Hash key (not used)
                    probe_keys: Vec::new(),   // Detection key (not used)
                    col_names,
                },
            ),
            input_vars,
        }
    }

    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        input_vars: Vec<String>,
        col_names: Vec<String>,
        context: crate::query::executor::base::ExecutionContext,
    ) -> Self {
        Self {
            base_executor: BaseJoinExecutor::with_context(
                id,
                storage,
                context,
                crate::query::executor::base::JoinConfigWithDesc {
                    left_var: String::new(),  // Left variable (not used)
                    right_var: String::new(), // Right variable (not used)
                    hash_keys: Vec::new(),    // Hash key (not used)
                    probe_keys: Vec::new(),   // Detection key (not used)
                    col_names,
                    description: String::new(),
                },
            ),
            input_vars,
        }
    }

    /// Perform the Cartesian product of two tables.
    fn execute_two_way_cartesian_product(
        &self,
        left_dataset: &DataSet,
        right_dataset: &DataSet,
    ) -> Result<DataSet, QueryError> {
        let mut result = DataSet::new();
        // Merge column names from both inputs, avoiding duplicates
        let mut col_names = left_dataset.col_names.clone();
        for col in &right_dataset.col_names {
            if !col_names.contains(col) {
                col_names.push(col.clone());
            } else {
                // If duplicate, add a suffix to make it unique
                let mut idx = 1;
                let mut new_col = format!("{}_{}", col, idx);
                while col_names.contains(&new_col) {
                    idx += 1;
                    new_col = format!("{}_{}", col, idx);
                }
                col_names.push(new_col);
            }
        }
        result.col_names = col_names;

        // Calculate the size of the result set and pre-allocate memory accordingly.
        let estimated_size = left_dataset.rows.len() * right_dataset.rows.len();
        if estimated_size > 0 {
            result.rows.reserve(estimated_size);
        }

        // Calculate the Cartesian product.
        for left_row in &left_dataset.rows {
            for right_row in &right_dataset.rows {
                let mut new_row = left_row.clone();
                new_row.extend(right_row.clone());
                result.rows.push(new_row);
            }
        }

        Ok(result)
    }

    /// An optimized implementation of the Cartesian product (using iterators to avoid intermediate result sets)
    fn execute_optimized_cartesian_product(&self) -> Result<DataSet, QueryError> {
        if self.input_vars.len() < 2 {
            return Err(QueryError::execution(
                "The Cartesian product requires at least two inputs".to_string(),
            ));
        }

        // Obtain all the input datasets.
        let mut datasets = Vec::new();
        for var in &self.input_vars {
            let result = self
                .base_executor
                .get_base()
                .context
                .get_result(var)
                .ok_or_else(|| {
                    QueryError::execution(format!("Input variable not found: {}", var))
                })?;

            let dataset = match result {
                ExecutionResult::DataSet(dataset) => dataset.clone(),
                ExecutionResult::Empty
                | ExecutionResult::Success
                | ExecutionResult::SpaceSwitched(_) => DataSet::new(),
                ExecutionResult::Error(msg) => {
                    return Err(QueryError::execution(msg));
                }
            };

            datasets.push(dataset);
        }

        // Check whether there is an empty set.
        for dataset in &datasets {
            if dataset.rows.is_empty() {
                return Ok(DataSet {
                    col_names: self.base_executor.get_col_names().clone(),
                    rows: Vec::new(),
                });
            }
        }

        // Calculate the size of the result set
        let total_size: usize = datasets.iter().map(|ds| ds.rows.len()).product();

        let mut result = DataSet::new();
        result.col_names = self.base_executor.get_col_names().clone();

        if total_size > 0 {
            result.rows.reserve(total_size);
        }

        // Generate the Cartesian product using either a recursive or an iterative approach.
        self.generate_cartesian_product_recursive(&datasets, 0, Vec::new(), &mut result);

        Ok(result)
    }

    /// Recursively generate the Cartesian product
    fn generate_cartesian_product_recursive(
        &self,
        datasets: &[DataSet],
        current_index: usize,
        current_row: Vec<Value>,
        result: &mut DataSet,
    ) {
        if current_index >= datasets.len() {
            // Upon reaching the last dataset, add the complete row to the results.
            result.rows.push(current_row);
            return;
        }

        // Traverse each row of the current dataset.
        for row in &datasets[current_index].rows {
            let mut new_row = current_row.clone();
            new_row.extend(row.clone());
            self.generate_cartesian_product_recursive(datasets, current_index + 1, new_row, result);
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for CrossJoinExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // Select the implementation method based on the number of inputs.
        let result = if self.input_vars.len() == 2 {
            // Cartesian product of two sets
            let left_var = &self.input_vars[0];
            let right_var = &self.input_vars[1];

            let left_result = self
                .base_executor
                .get_base()
                .context
                .get_result(left_var)
                .ok_or_else(|| {
                    DBError::query(format!("Left input variable not found: {}", left_var))
                })?;

            let right_result = self
                .base_executor
                .get_base()
                .context
                .get_result(right_var)
                .ok_or_else(|| {
                    DBError::query(format!("Right input variable not found: {}", right_var))
                })?;

            let left_dataset = match left_result {
                ExecutionResult::DataSet(dataset) => dataset.clone(),
                ExecutionResult::Empty
                | ExecutionResult::Success
                | ExecutionResult::SpaceSwitched(_) => DataSet {
                    col_names: self.base_executor.get_col_names().clone(),
                    rows: Vec::new(),
                },
                ExecutionResult::Error(msg) => {
                    return Err(DBError::query(msg));
                }
            };

            let right_dataset = match right_result {
                ExecutionResult::DataSet(dataset) => dataset.clone(),
                ExecutionResult::Empty
                | ExecutionResult::Success
                | ExecutionResult::SpaceSwitched(_) => DataSet {
                    col_names: vec!["_empty".to_string()],
                    rows: Vec::new(),
                },
                ExecutionResult::Error(msg) => {
                    return Err(DBError::query(msg));
                }
            };

            self.execute_two_way_cartesian_product(&left_dataset, &right_dataset)
                .map_err(DBError::from)?
        } else {
            // Cartesian product of multiple tables
            self.execute_optimized_cartesian_product()
                .map_err(DBError::from)?
        };

        Ok(ExecutionResult::DataSet(result))
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base_executor.get_base().is_open()
    }

    fn id(&self) -> i64 {
        self.base_executor.get_base().id
    }

    fn name(&self) -> &str {
        "CrossJoinExecutor"
    }

    fn description(&self) -> &str {
        &self.base_executor.get_base().description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base_executor.get_base().get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base_executor.get_base_mut().get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> crate::query::executor::base::HasStorage<S>
    for CrossJoinExecutor<S>
{
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base_executor
            .get_base()
            .storage
            .as_ref()
            .expect("CrossJoinExecutor storage should be set")
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::core::Value;
    use crate::query::executor::relational_algebra::join::ExpressionContextStruct;
    use crate::query::DataSet;
    use crate::storage::MockStorage;

    #[test]
    fn test_cross_join_two_tables() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        let expr_context = Arc::new(ExpressionContextStruct::new());

        // Create an executor.
        let mut executor = CrossJoinExecutor::new(
            1,
            storage,
            vec!["left".to_string(), "right".to_string()],
            vec![
                "id".to_string(),
                "name".to_string(),
                "age".to_string(),
                "city".to_string(),
            ],
            expr_context,
        );

        // Setting the execution context
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["age".to_string(), "city".to_string()],
            rows: vec![
                vec![Value::Int(25), Value::String("New York".to_string())],
                vec![Value::Int(30), Value::String("London".to_string())],
            ],
        };

        executor
            .base_executor
            .get_base_mut()
            .context
            .set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));

        executor
            .base_executor
            .get_base_mut()
            .context
            .set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        // Establish the connection.
        let result = executor.execute().expect("Failed to execute");

        // Verification results
        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 4); // 2 * 2 = 4

                // Verify the first line.
                assert_eq!(
                    dataset.rows[0],
                    vec![
                        Value::Int(1),
                        Value::String("Alice".to_string()),
                        Value::Int(25),
                        Value::String("New York".to_string()),
                    ]
                );

                // Verify the last line.
                assert_eq!(
                    dataset.rows[3],
                    vec![
                        Value::Int(2),
                        Value::String("Bob".to_string()),
                        Value::Int(30),
                        Value::String("London".to_string()),
                    ]
                );
            }
            _ => panic!("Expected DataSet results"),
        }
    }

    #[test]
    fn test_cross_join_empty_table() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let expr_context = Arc::new(ExpressionContextStruct::new());

        // Create an executor.
        let mut executor = CrossJoinExecutor::new(
            1,
            storage,
            vec!["left".to_string(), "right".to_string()],
            vec!["id".to_string(), "name".to_string(), "age".to_string()],
            expr_context,
        );

        // Setting the execution context
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec![Value::Int(1), Value::String("Alice".to_string())]],
        };

        let right_dataset = DataSet {
            col_names: vec!["age".to_string()],
            rows: Vec::new(), // Empty right-hand table
        };

        executor
            .base_executor
            .get_base_mut()
            .context
            .set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));

        executor
            .base_executor
            .get_base_mut()
            .context
            .set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        // Establish the connection.
        let result = executor.execute().expect("Failed to execute");

        // Validation results
        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 0); // Empty result
            }
            _ => panic!("Expected DataSet results"),
        }
    }

    #[test]
    fn test_cross_join_three_tables() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let expr_context = Arc::new(ExpressionContextStruct::new());

        // Create an executor.
        let mut executor = CrossJoinExecutor::new(
            1,
            storage,
            vec![
                "table1".to_string(),
                "table2".to_string(),
                "table3".to_string(),
            ],
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
                "f".to_string(),
            ],
            expr_context,
        );

        // Setting the execution context
        let table1 = DataSet {
            col_names: vec!["a".to_string()],
            rows: vec![vec![Value::Int(1)]],
        };

        let table2 = DataSet {
            col_names: vec!["b".to_string(), "c".to_string()],
            rows: vec![vec![Value::Int(2), Value::Int(3)]],
        };

        let table3 = DataSet {
            col_names: vec!["d".to_string(), "e".to_string(), "f".to_string()],
            rows: vec![vec![Value::Int(4), Value::Int(5), Value::Int(6)]],
        };

        executor
            .base_executor
            .get_base_mut()
            .context
            .set_result("table1".to_string(), ExecutionResult::DataSet(table1));

        executor
            .base_executor
            .get_base_mut()
            .context
            .set_result("table2".to_string(), ExecutionResult::DataSet(table2));

        executor
            .base_executor
            .get_base_mut()
            .context
            .set_result("table3".to_string(), ExecutionResult::DataSet(table3));

        // Establish the connection.
        let result = executor.execute().expect("Failed to execute");

        // Verification results
        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 1); // 1 * 1 * 1 = 1
                assert_eq!(
                    dataset.rows[0],
                    vec![
                        Value::Int(1),
                        Value::Int(2),
                        Value::Int(3),
                        Value::Int(4),
                        Value::Int(5),
                        Value::Int(6)
                    ]
                );
            }
            _ => panic!("Expected DataSet results"),
        }
    }
}

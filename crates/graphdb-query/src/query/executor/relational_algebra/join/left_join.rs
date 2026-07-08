//! Implementation of the Left Outer Join Executor
//!
//! Implement a hash-based left outer join algorithm that supports both single-key and multi-key joins.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::ContextualExpression;
use crate::core::{Expression, NullType, Value};
use crate::query::executor::base::{ExecutionResult, Executor, HasStorage, JoinConfig};
use crate::query::executor::relational_algebra::join::{
    base_join::BaseJoinExecutor,
    hash_table::{build_hash_table, extract_key_values, JoinKey},
    ExpressionContextStruct,
};
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Left Outer Join Executor
pub struct LeftJoinExecutor<S: StorageClient> {
    base_executor: BaseJoinExecutor<S>,
    /// The number of columns in the right dataset (used to fill in NULL values)
    right_col_size: usize,
    /// Should a multi-key join be used?
    use_multi_key: bool,
}

/// Left Outer Join Executor Configuration
#[derive(Debug, Clone)]
pub struct LeftJoinConfig {
    pub id: i64,
    pub hash_keys: Vec<ContextualExpression>,
    pub probe_keys: Vec<ContextualExpression>,
    pub left_var: String,
    pub right_var: String,
    pub col_names: Vec<String>,
}

impl<S: StorageClient> LeftJoinExecutor<S> {
    pub fn new(
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionContextStruct>,
        config: LeftJoinConfig,
    ) -> Self {
        let use_multi_key = config.hash_keys.len() > 1;

        // Extract the list of Expressions from the list of ContextualExpressions.
        let hash_exprs = Self::extract_expressions(&config.hash_keys);
        let probe_exprs = Self::extract_expressions(&config.probe_keys);

        let join_config = JoinConfig {
            left_var: config.left_var,
            right_var: config.right_var,
            hash_keys: hash_exprs,
            probe_keys: probe_exprs,
            col_names: config.col_names,
        };

        Self {
            base_executor: BaseJoinExecutor::new(config.id, storage, expr_context, join_config),
            right_col_size: 0,
            use_multi_key,
        }
    }

    /// An auxiliary method for extracting the Expression list from the ContextualExpression list
    fn extract_expressions(ctx_exprs: &[ContextualExpression]) -> Vec<Expression> {
        ctx_exprs
            .iter()
            .filter_map(|ctx_expr| ctx_expr.expression().map(|meta| meta.inner().clone()))
            .collect()
    }

    /// Perform a single-key left outer join.
    fn execute_single_key_join(
        &mut self,
        left_dataset: &DataSet,
        right_dataset: &DataSet,
    ) -> DBResult<DataSet> {
        // Record the number of columns in the dataset on the right side.
        self.right_col_size = right_dataset.col_names.len();

        // A left outer join always uses the left table as the driving table, and the right table is used to build a hash table.
        let build_dataset = right_dataset;

        // Constructing a hash table
        let hash_table = build_hash_table(build_dataset, self.base_executor.get_probe_keys())
            .map_err(|e| DBError::query(format!("Failed to build hash table: {}", e)))?;

        // Construct a mapping from column names to indices.
        let left_col_map: std::collections::HashMap<&str, usize> = left_dataset
            .col_names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect();

        // Constructing the result set
        let mut result = DataSet::new();
        result.col_names = self.base_executor.get_col_names().clone();

        // The index of the matched row from the left table has been recorded.
        let mut matched_rows = std::collections::HashSet::new();

        // Process each row of the left table.
        for left_row in &left_dataset.rows {
            let left_key_parts = extract_key_values(
                left_row,
                &left_dataset.col_names,
                self.base_executor.get_hash_keys(),
                &left_col_map,
            );

            let left_key = JoinKey::new(left_key_parts);

            // Find the matching row in the right table.
            if let Some(right_indices) = hash_table.get(&left_key) {
                matched_rows.insert(left_row.to_vec()); // Marked as matched

                for &right_idx in right_indices {
                    if right_idx < build_dataset.rows.len() {
                        let right_row = &build_dataset.rows[right_idx];
                        let new_row = self.base_executor.new_row(
                            left_row.to_vec(),
                            right_row.to_vec(),
                            &left_dataset.col_names,
                            &right_dataset.col_names,
                        );
                        result.rows.push(new_row);
                    }
                }
            }
        }

        // Handle unmatched rows from the left table (fill with NULL).
        for left_row in &left_dataset.rows {
            if !matched_rows.contains(left_row) {
                let mut new_row = left_row.to_vec();
                // Fill the right column with NULL values.
                for _ in 0..self.right_col_size {
                    new_row.push(Value::Null(NullType::Null));
                }
                result.rows.push(new_row);
            }
        }

        Ok(result)
    }

    /// Perform a multi-key left outer join
    fn execute_multi_key_join(
        &mut self,
        left_dataset: &DataSet,
        right_dataset: &DataSet,
    ) -> DBResult<DataSet> {
        // Record the number of columns in the dataset on the right side.
        self.right_col_size = right_dataset.col_names.len();

        // A left outer join always uses the left table as the driving table, and the right table is used to build a hash table.
        let build_dataset = right_dataset;

        // Constructing a hash table
        let hash_table = build_hash_table(build_dataset, self.base_executor.get_probe_keys())
            .map_err(|e| DBError::query(format!("Failed to build multi-key hash table: {}", e)))?;

        // Construct a mapping from column names to indexes.
        let left_col_map: std::collections::HashMap<&str, usize> = left_dataset
            .col_names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect();

        // Constructing the result set
        let mut result = DataSet::new();
        result.col_names = self.base_executor.get_col_names().clone();

        // The index of the matched row from the left table has been recorded.
        let mut matched_rows = std::collections::HashSet::new();

        // Process each row of the left table.
        for left_row in &left_dataset.rows {
            let left_key_parts = extract_key_values(
                left_row,
                &left_dataset.col_names,
                self.base_executor.get_hash_keys(),
                &left_col_map,
            );

            let left_key = JoinKey::new(left_key_parts);

            // Find the matching row in the right table.
            if let Some(right_indices) = hash_table.get(&left_key) {
                matched_rows.insert(left_row.clone()); // Marked as matched

                for &right_idx in right_indices {
                    if right_idx < build_dataset.rows.len() {
                        let right_row = &build_dataset.rows[right_idx];
                        let new_row = self.base_executor.new_row(
                            left_row.clone(),
                            right_row.clone(),
                            &left_dataset.col_names,
                            &right_dataset.col_names,
                        );
                        result.rows.push(new_row);
                    }
                }
            }
        }

        // Handle unmatched rows from the left table (fill with NULL).
        for left_row in &left_dataset.rows {
            if !matched_rows.contains(left_row) {
                let mut new_row = left_row.clone();
                // Fill the right column with NULL values.
                for _ in 0..self.right_col_size {
                    new_row.push(Value::Null(NullType::Null));
                }
                result.rows.push(new_row);
            }
        }

        Ok(result)
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for LeftJoinExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let (left_dataset, right_dataset) = self.base_executor.check_input_datasets()?;

        if left_dataset.rows.is_empty() {
            let empty_result = DataSet {
                col_names: self.base_executor.get_col_names().clone(),
                rows: Vec::new(),
            };
            return Ok(ExecutionResult::DataSet(empty_result));
        }

        if right_dataset.rows.is_empty() {
            let mut result = DataSet::new();
            result.col_names = self.base_executor.get_col_names().clone();
            self.right_col_size = right_dataset.col_names.len();

            for left_row in &left_dataset.rows {
                let mut new_row = left_row.clone();
                for _ in 0..self.right_col_size {
                    new_row.push(Value::Null(NullType::Null));
                }
                result.rows.push(new_row);
            }

            return Ok(ExecutionResult::DataSet(result));
        }

        let result = if self.use_multi_key {
            self.execute_multi_key_join(&left_dataset, &right_dataset)?
        } else {
            self.execute_single_key_join(&left_dataset, &right_dataset)?
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
        &self.base_executor.get_base().name
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

impl<S: StorageClient + Send + 'static> HasStorage<S> for LeftJoinExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base_executor
            .get_base()
            .storage
            .as_ref()
            .expect("LeftJoinExecutor storage should be set")
    }
}

/// Hash Left Outer Join Executor (Parallel Version)
pub struct HashLeftJoinExecutor<S: StorageClient> {
    inner: LeftJoinExecutor<S>,
}

impl<S: StorageClient> HashLeftJoinExecutor<S> {
    pub fn new(
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionContextStruct>,
        config: LeftJoinConfig,
    ) -> Self {
        Self {
            inner: LeftJoinExecutor::new(storage, expr_context, config),
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for HashLeftJoinExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self.inner.execute()
    }

    fn open(&mut self) -> DBResult<()> {
        self.inner.open()
    }

    fn close(&mut self) -> DBResult<()> {
        self.inner.close()
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    fn id(&self) -> i64 {
        self.inner.id()
    }

    fn name(&self) -> &str {
        "HashLeftJoinExecutor"
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.inner.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.inner.stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> HasStorage<S> for HashLeftJoinExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.inner.get_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::storage::MockStorage;

    #[test]
    fn test_left_join_single_key() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionContextStruct::new());

        let expr1 = crate::core::Expression::Variable("id".to_string());
        let expr_meta1 = crate::core::types::expr::ExpressionMeta::new(expr1);
        let expr_id1 = expr_context.register_expression(expr_meta1);
        let ctx_expr1 =
            crate::core::types::ContextualExpression::new(expr_id1, expr_context.clone());

        // Create an executor.
        let config = LeftJoinConfig {
            id: 1,
            hash_keys: vec![ctx_expr1.clone()], // The id column in the left table serves as the key.
            probe_keys: vec![ctx_expr1], // The id column in the right table serves as the key.
            left_var: "left".to_string(),
            right_var: "right".to_string(),
            col_names: vec!["id".to_string(), "name".to_string(), "age".to_string()],
        };

        let mut executor = LeftJoinExecutor::new(storage, expr_context.clone(), config);

        // Setting the execution context
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
                vec![Value::Int(3), Value::String("Charlie".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "age".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::Int(25)],
                vec![Value::Int(2), Value::Int(30)],
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
                assert_eq!(dataset.rows.len(), 3); // Three lines of results (including those that did not match)

                // First line: Alice matches
                assert_eq!(
                    dataset.rows[0],
                    vec![
                        Value::Int(1),
                        Value::String("Alice".to_string()),
                        Value::Int(25),
                    ]
                );

                // Second line: Bob matches.
                assert_eq!(
                    dataset.rows[1],
                    vec![
                        Value::Int(2),
                        Value::String("Bob".to_string()),
                        Value::Int(30),
                    ]
                );

                // Third line: Charlie was not matched; the value for “age” is NULL.
                assert_eq!(dataset.rows[2][0], Value::Int(3));
                assert_eq!(dataset.rows[2][1], Value::String("Charlie".to_string()));
                assert_eq!(dataset.rows[2][2], Value::Null(NullType::Null));
            }
            _ => panic!("Expected DataSet results"),
        }
    }

    #[test]
    fn test_left_join_empty_right() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionContextStruct::new());

        let expr1 = Expression::Variable("0".to_string());
        let expr_meta1 = crate::core::types::expr::ExpressionMeta::new(expr1);
        let expr_id1 = expr_context.register_expression(expr_meta1);
        let ctx_expr1 =
            crate::core::types::ContextualExpression::new(expr_id1, expr_context.clone());

        // Create an executor.
        let config = LeftJoinConfig {
            id: 1,
            hash_keys: vec![ctx_expr1.clone()],
            probe_keys: vec![ctx_expr1],
            left_var: "left".to_string(),
            right_var: "right".to_string(),
            col_names: vec!["id".to_string(), "name".to_string(), "age".to_string()],
        };

        let mut executor = LeftJoinExecutor::new(storage, expr_context.clone(), config);

        // Setting the execution context
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "age".to_string()],
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

        // Verification results
        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 2); // The results for both lines should be filled with NULL.

                // The value of "age" in all rows should be NULL.
                for row in &dataset.rows {
                    assert_eq!(row[2], Value::Null(NullType::Null));
                }
            }
            _ => panic!("Expected DataSet results"),
        }
    }
}

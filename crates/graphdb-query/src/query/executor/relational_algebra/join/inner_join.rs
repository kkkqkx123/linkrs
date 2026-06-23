//! Implementation of the internal connection executor
//!
//! Implement an inner join algorithm based on hashing, which supports both single-key and multi-key joins.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::ContextualExpression;
use crate::core::{Expression, Value};
use crate::query::executor::base::{ExecutionResult, Executor, HasStorage, JoinConfig};
use crate::query::executor::expression::evaluation_context::row_context::RowExpressionContext;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::relational_algebra::join::base_join::BaseJoinExecutor;
use crate::query::executor::relational_algebra::join::ExpressionContextStruct;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;

/// Internal connection executor
pub struct InnerJoinExecutor<S: StorageClient> {
    base_executor: BaseJoinExecutor<S>,
    single_key_hash_table: Option<HashMap<Value, Vec<Vec<Value>>>>,
    multi_key_hash_table: Option<HashMap<Vec<Value>, Vec<Vec<Value>>>>,
    use_multi_key: bool,
}

/// Internal Connector Executor Configuration
#[derive(Debug, Clone)]
pub struct InnerJoinConfig {
    pub id: i64,
    pub hash_keys: Vec<ContextualExpression>,
    pub probe_keys: Vec<ContextualExpression>,
    pub left_var: String,
    pub right_var: String,
    pub col_names: Vec<String>,
}

impl<S: StorageClient> std::fmt::Debug for InnerJoinExecutor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InnerJoinExecutor")
            .field("base_executor", &"BaseJoinExecutor<S>")
            .field(
                "single_key_hash_table",
                &self.single_key_hash_table.is_some(),
            )
            .field("multi_key_hash_table", &self.multi_key_hash_table.is_some())
            .field("use_multi_key", &self.use_multi_key)
            .finish()
    }
}

impl<S: StorageClient> InnerJoinExecutor<S> {
    pub fn new(
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionContextStruct>,
        config: InnerJoinConfig,
    ) -> Self {
        let use_multi_key = config.hash_keys.len() > 1;

        // Extract the Expression list from the ContextualExpression list.
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
            single_key_hash_table: None,
            multi_key_hash_table: None,
            use_multi_key,
        }
    }

    pub fn with_context(
        storage: Arc<RwLock<S>>,
        context: crate::query::executor::base::ExecutionContext,
        config: InnerJoinConfig,
    ) -> Self {
        let use_multi_key = config.hash_keys.len() > 1;

        // Extract the Expression list from the ContextualExpression list.
        let hash_exprs = Self::extract_expressions(&config.hash_keys);
        let probe_exprs = Self::extract_expressions(&config.probe_keys);

        let join_config = JoinConfig {
            left_var: config.left_var,
            right_var: config.right_var,
            hash_keys: hash_exprs,
            probe_keys: probe_exprs,
            col_names: config.col_names,
        };

        let join_config_with_desc = crate::query::executor::base::JoinConfigWithDesc {
            left_var: join_config.left_var,
            right_var: join_config.right_var,
            hash_keys: join_config.hash_keys,
            probe_keys: join_config.probe_keys,
            col_names: join_config.col_names,
            description: String::new(),
        };

        Self {
            base_executor: BaseJoinExecutor::with_context(
                config.id,
                storage,
                context,
                join_config_with_desc,
            ),
            single_key_hash_table: None,
            multi_key_hash_table: None,
            use_multi_key,
        }
    }

    /// Auxiliary method for extracting the Expression list from the ContextualExpression list
    fn extract_expressions(ctx_exprs: &[ContextualExpression]) -> Vec<Expression> {
        ctx_exprs
            .iter()
            .filter_map(|ctx_expr| ctx_expr.expression().map(|meta| meta.inner().clone()))
            .collect()
    }

    /// Perform a single-key inner join (using expression evaluation).
    fn execute_single_key_join(
        &mut self,
        left_dataset: &DataSet,
        right_dataset: &DataSet,
    ) -> Result<DataSet, QueryError> {
        self.base_executor
            .optimize_join_order(left_dataset, right_dataset);
        let exchange = self.base_executor.is_exchanged();

        let hash_keys = self.base_executor.get_hash_keys().clone();
        let probe_keys = self.base_executor.get_probe_keys().clone();

        if hash_keys.is_empty() || probe_keys.is_empty() {
            return Err(QueryError::execution(
                "Hash key or probe key is empty".to_string(),
            ));
        }

        // Helper function to get column names from dataset or extract from key expression
        let get_col_names = |dataset: &DataSet, key_expr: &Expression| -> Vec<String> {
            if dataset.col_names.len() == 1 && dataset.col_names[0] == "_vertex" {
                // If the dataset has only one column named "_vertex", try to extract the variable name from the key expression
                if let Expression::Variable(var_name) = key_expr {
                    vec![var_name.clone()]
                } else {
                    dataset.col_names.clone()
                }
            } else {
                dataset.col_names.clone()
            }
        };

        let (hash_key, probe_key, build_dataset, probe_dataset, build_col_names, probe_col_names) =
            if exchange {
                let build_col_names = get_col_names(right_dataset, &probe_keys[0]);
                let probe_col_names = get_col_names(left_dataset, &hash_keys[0]);
                (
                    probe_keys[0].clone(),
                    hash_keys[0].clone(),
                    right_dataset,
                    left_dataset,
                    build_col_names,
                    probe_col_names,
                )
            } else {
                let build_col_names = get_col_names(left_dataset, &hash_keys[0]);
                let probe_col_names = get_col_names(right_dataset, &probe_keys[0]);
                (
                    hash_keys[0].clone(),
                    probe_keys[0].clone(),
                    left_dataset,
                    right_dataset,
                    build_col_names,
                    probe_col_names,
                )
            };

        let mut hash_table: HashMap<Value, Vec<Vec<Value>>> = HashMap::new();

        for row in &build_dataset.rows {
            let mut context = RowExpressionContext::from_dataset(row, &build_col_names);
            let key = ExpressionEvaluator::evaluate(&hash_key, &mut context)
                .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;

            hash_table.entry(key).or_default().push(row.to_vec());
        }

        let mut result = DataSet::new();
        result.col_names = self.base_executor.get_col_names().clone();
        let output_col_names = result.col_names.clone();

        for probe_row in &probe_dataset.rows {
            let mut probe_context = RowExpressionContext::from_dataset(probe_row, &probe_col_names);
            let probe_key_val = ExpressionEvaluator::evaluate(&probe_key, &mut probe_context)
                .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;

            if let Some(matching_rows) = hash_table.get(&probe_key_val) {
                for build_row in matching_rows {
                    // When exchange is true, build_row comes from right_dataset and probe_row comes from left_dataset
                    // But output_col_names is in left-then-right order, so we need to swap the arguments
                    let new_row = if exchange {
                        Self::build_join_result_row(
                            probe_row,
                            build_row,
                            &probe_col_names,
                            &build_col_names,
                            &output_col_names,
                        )
                    } else {
                        Self::build_join_result_row(
                            build_row,
                            probe_row,
                            &build_col_names,
                            &probe_col_names,
                            &output_col_names,
                        )
                    };
                    result.rows.push(new_row);
                }
            }
        }

        Ok(result)
    }

    /// Construct the rows of the connection result based on the column names in the output column.
    fn build_join_result_row(
        left_row: &[Value],
        right_row: &[Value],
        left_col_names: &[String],
        right_col_names: &[String],
        output_col_names: &[String],
    ) -> Vec<Value> {
        let mut result = Vec::with_capacity(output_col_names.len());

        for col_name in output_col_names {
            // First try exact match
            if let Some(idx) = left_col_names.iter().position(|c| c == col_name) {
                if let Some(val) = left_row.get(idx) {
                    result.push(val.clone());
                    continue;
                }
            } else if let Some(idx) = right_col_names.iter().position(|c| c == col_name) {
                if let Some(val) = right_row.get(idx) {
                    result.push(val.clone());
                    continue;
                }
            }

            // If not found, try to strip suffix (e.g., "src_1" -> "src")
            // This handles the case where HashInnerJoinNode adds suffixes to duplicate column names
            let base_name = if let Some(underscore_pos) = col_name.rfind('_') {
                if underscore_pos > 0 {
                    let suffix = &col_name[underscore_pos + 1..];
                    if suffix.parse::<usize>().is_ok() {
                        // This is a suffixed column name like "src_1"
                        &col_name[..underscore_pos]
                    } else {
                        col_name
                    }
                } else {
                    col_name
                }
            } else {
                col_name
            };

            if let Some(idx) = left_col_names.iter().position(|c| c == base_name) {
                if let Some(val) = left_row.get(idx) {
                    result.push(val.clone());
                    continue;
                }
            } else if let Some(idx) = right_col_names.iter().position(|c| c == base_name) {
                if let Some(val) = right_row.get(idx) {
                    result.push(val.clone());
                    continue;
                }
            }
        }

        result
    }

    /// Perform multiple key inner joins (using expression evaluation)
    fn execute_multi_key_join(
        &mut self,
        left_dataset: &DataSet,
        right_dataset: &DataSet,
    ) -> Result<DataSet, QueryError> {
        self.base_executor
            .optimize_join_order(left_dataset, right_dataset);
        let exchange = self.base_executor.is_exchanged();

        let hash_keys = self.base_executor.get_hash_keys().clone();
        let probe_keys = self.base_executor.get_probe_keys().clone();

        if hash_keys.is_empty() || probe_keys.is_empty() {
            return Err(QueryError::execution(
                "Hash or probe key is empty".to_string(),
            ));
        }

        let (hash_keys, probe_keys, build_dataset, probe_dataset, build_col_names, probe_col_names) =
            if exchange {
                // When exchanging, swap the hash and probe keys as well
                (
                    probe_keys,
                    hash_keys,
                    right_dataset,
                    left_dataset,
                    &right_dataset.col_names,
                    &left_dataset.col_names,
                )
            } else {
                (
                    hash_keys,
                    probe_keys,
                    left_dataset,
                    right_dataset,
                    &left_dataset.col_names,
                    &right_dataset.col_names,
                )
            };

        let mut hash_table: HashMap<Vec<Value>, Vec<Vec<Value>>> = HashMap::new();

        for row in &build_dataset.rows {
            let mut context = RowExpressionContext::from_dataset(row, build_col_names);
            let mut key_values = Vec::with_capacity(hash_keys.len());

            for hash_key in &hash_keys {
                let key = ExpressionEvaluator::evaluate(hash_key, &mut context)
                    .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;
                key_values.push(key);
            }

            hash_table.entry(key_values).or_default().push(row.to_vec());
        }

        let mut result = DataSet::new();
        result.col_names = self.base_executor.get_col_names().clone();
        let output_col_names = result.col_names.clone();

        for probe_row in &probe_dataset.rows {
            let mut context = RowExpressionContext::from_dataset(probe_row, probe_col_names);
            let mut key_values = Vec::with_capacity(probe_keys.len());

            for probe_key in &probe_keys {
                let key = ExpressionEvaluator::evaluate(probe_key, &mut context)
                    .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;
                key_values.push(key);
            }

            if let Some(matching_rows) = hash_table.get(&key_values) {
                for build_row in matching_rows {
                    // When exchange is true, build_row comes from right_dataset and probe_row comes from left_dataset
                    // But output_col_names is in left-then-right order, so we need to swap the arguments
                    let new_row = if exchange {
                        Self::build_join_result_row(
                            probe_row,
                            build_row,
                            probe_col_names,
                            build_col_names,
                            &output_col_names,
                        )
                    } else {
                        Self::build_join_result_row(
                            build_row,
                            probe_row,
                            build_col_names,
                            probe_col_names,
                            &output_col_names,
                        )
                    };
                    result.rows.push(new_row);
                }
            }
        }

        Ok(result)
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for InnerJoinExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let (left_dataset, right_dataset) = self
            .base_executor
            .check_input_datasets()
            .map_err(DBError::from)?;

        if left_dataset.rows.is_empty() || right_dataset.rows.is_empty() {
            let empty_result = DataSet {
                col_names: self.base_executor.get_col_names().clone(),
                rows: Vec::new(),
            };
            return Ok(ExecutionResult::DataSet(empty_result));
        }

        let result = if self.use_multi_key {
            self.execute_multi_key_join(&left_dataset, &right_dataset)
                .map_err(DBError::from)?
        } else {
            self.execute_single_key_join(&left_dataset, &right_dataset)
                .map_err(DBError::from)?
        };
        self.base_executor
            .get_base_mut()
            .get_stats_mut()
            .add_row(result.rows.len());

        Ok(ExecutionResult::DataSet(result))
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        self.single_key_hash_table = None;
        self.multi_key_hash_table = None;
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

impl<S: StorageClient + Send + 'static> HasStorage<S> for InnerJoinExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base_executor
            .get_base()
            .storage
            .as_ref()
            .expect("InnerJoinExecutor storage should be set")
    }
}

#[derive(Debug)]
pub struct HashInnerJoinExecutor<S: StorageClient> {
    inner: InnerJoinExecutor<S>,
}

impl<S: StorageClient> HashInnerJoinExecutor<S> {
    pub fn new(
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionContextStruct>,
        config: InnerJoinConfig,
    ) -> Self {
        Self {
            inner: InnerJoinExecutor::new(storage, expr_context, config),
        }
    }

    pub fn with_context(
        storage: Arc<RwLock<S>>,
        context: crate::query::executor::base::ExecutionContext,
        config: InnerJoinConfig,
    ) -> Self {
        Self {
            inner: InnerJoinExecutor::with_context(storage, context, config),
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for HashInnerJoinExecutor<S> {
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
        "HashInnerJoinExecutor"
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

impl<S: StorageClient + Send + 'static> HasStorage<S> for HashInnerJoinExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.inner.get_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::storage::MockStorage;

    fn create_test_datasets() -> (DataSet, DataSet) {
        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::String("Alice".to_string())],
                vec![Value::Int(2), Value::String("Bob".to_string())],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "age".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::Int(25)],
                vec![Value::Int(2), Value::Int(30)],
                vec![Value::Int(3), Value::Int(35)],
            ],
        };

        (left_dataset, right_dataset)
    }

    #[test]
    fn test_inner_join_single_key_with_expression() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionContextStruct::new());

        let expr1 = Expression::variable("id");
        let expr_meta1 = crate::core::types::expr::ExpressionMeta::new(expr1);
        let expr_id1 = expr_context.register_expression(expr_meta1);
        let ctx_expr1 =
            crate::core::types::ContextualExpression::new(expr_id1, expr_context.clone());

        let config = InnerJoinConfig {
            id: 1,
            hash_keys: vec![ctx_expr1.clone()],
            probe_keys: vec![ctx_expr1],
            left_var: "left".to_string(),
            right_var: "right".to_string(),
            col_names: vec!["id".to_string(), "name".to_string(), "age".to_string()],
        };

        let mut executor = InnerJoinExecutor::new(storage, expr_context.clone(), config);

        let (left_dataset, right_dataset) = create_test_datasets();

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

        let result = executor.execute().expect("failure of execution");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 2);
                assert_eq!(dataset.rows[0][0], Value::Int(1));
                assert_eq!(dataset.rows[0][1], Value::String("Alice".to_string()));
                assert_eq!(dataset.rows[0][2], Value::Int(25));
                assert_eq!(dataset.rows[1][0], Value::Int(2));
                assert_eq!(dataset.rows[1][1], Value::String("Bob".to_string()));
                assert_eq!(dataset.rows[1][2], Value::Int(30));
            }
            _ => panic!("Expected DataSet results"),
        }
    }

    #[test]
    fn test_inner_join_multi_key() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionContextStruct::new());

        let expr1 = Expression::variable("a");
        let expr_meta1 = crate::core::types::expr::ExpressionMeta::new(expr1);
        let expr_id1 = expr_context.register_expression(expr_meta1);
        let ctx_expr1 =
            crate::core::types::ContextualExpression::new(expr_id1, expr_context.clone());

        let expr2 = Expression::variable("b");
        let expr_meta2 = crate::core::types::expr::ExpressionMeta::new(expr2);
        let expr_id2 = expr_context.register_expression(expr_meta2);
        let ctx_expr2 =
            crate::core::types::ContextualExpression::new(expr_id2, expr_context.clone());

        let left_dataset = DataSet {
            col_names: vec!["a".to_string(), "b".to_string(), "name".to_string()],
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Int(10),
                    Value::String("Alice".to_string()),
                ],
                vec![
                    Value::Int(2),
                    Value::Int(20),
                    Value::String("Bob".to_string()),
                ],
            ],
        };

        let right_dataset = DataSet {
            col_names: vec!["a".to_string(), "b".to_string(), "age".to_string()],
            rows: vec![
                vec![Value::Int(1), Value::Int(10), Value::Int(25)],
                vec![Value::Int(1), Value::Int(11), Value::Int(26)],
                vec![Value::Int(2), Value::Int(20), Value::Int(30)],
            ],
        };

        let config = InnerJoinConfig {
            id: 2,
            hash_keys: vec![ctx_expr1.clone(), ctx_expr2.clone()],
            probe_keys: vec![ctx_expr1, ctx_expr2],
            left_var: "left".to_string(),
            right_var: "right".to_string(),
            col_names: vec![
                "a".to_string(),
                "b".to_string(),
                "name".to_string(),
                "age".to_string(),
            ],
        };

        let mut executor = InnerJoinExecutor::new(storage, expr_context.clone(), config);

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

        let result = executor.execute().expect("failure of execution");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 2);
                assert_eq!(dataset.rows[0][2], Value::String("Alice".to_string()));
                assert_eq!(dataset.rows[0][3], Value::Int(25));
                assert_eq!(dataset.rows[1][2], Value::String("Bob".to_string()));
                assert_eq!(dataset.rows[1][3], Value::Int(30));
            }
            _ => panic!("Expected DataSet results"),
        }
    }

    #[test]
    fn test_inner_join_empty_dataset() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionContextStruct::new());

        let expr1 = Expression::variable("id");
        let expr_meta1 = crate::core::types::expr::ExpressionMeta::new(expr1);
        let expr_id1 = expr_context.register_expression(expr_meta1);
        let ctx_expr1 =
            crate::core::types::ContextualExpression::new(expr_id1, expr_context.clone());

        let left_dataset = DataSet {
            col_names: vec!["id".to_string(), "name".to_string()],
            rows: vec![],
        };

        let right_dataset = DataSet {
            col_names: vec!["id".to_string(), "age".to_string()],
            rows: vec![vec![Value::Int(1), Value::Int(25)]],
        };

        let config = InnerJoinConfig {
            id: 3,
            hash_keys: vec![ctx_expr1.clone()],
            probe_keys: vec![ctx_expr1],
            left_var: "left".to_string(),
            right_var: "right".to_string(),
            col_names: vec!["id".to_string(), "name".to_string(), "age".to_string()],
        };

        let mut executor = InnerJoinExecutor::new(storage, expr_context.clone(), config);

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

        let result = executor.execute().expect("failure of execution");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 0);
            }
            _ => panic!("Expected DataSet results"),
        }
    }

    #[test]
    fn test_inner_join_with_variable_expression() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionContextStruct::new());

        let expr1 = Expression::Variable("id".to_string());
        let expr_meta1 = crate::core::types::expr::ExpressionMeta::new(expr1);
        let expr_id1 = expr_context.register_expression(expr_meta1);
        let ctx_expr1 =
            crate::core::types::ContextualExpression::new(expr_id1, expr_context.clone());

        let config = InnerJoinConfig {
            id: 4,
            hash_keys: vec![ctx_expr1.clone()],
            probe_keys: vec![ctx_expr1],
            left_var: "left".to_string(),
            right_var: "right".to_string(),
            col_names: vec!["id".to_string(), "name".to_string(), "age".to_string()],
        };

        let mut executor = InnerJoinExecutor::new(storage, expr_context.clone(), config);

        let (left_dataset, right_dataset) = create_test_datasets();

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

        let result = executor.execute().expect("failure of execution");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 2);
            }
            _ => panic!("Expected DataSet results"),
        }
    }
}

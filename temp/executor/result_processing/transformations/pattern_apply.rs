//! Implementation of PatternApplyExecutor
//!
//! Responsible for handling pattern matching operations, supporting the semantics of EXISTS and NOT EXISTS.
//! Perform a key matching between the left input data and the right input data.

use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::Expression;
use crate::core::{List, Value};
#[cfg(test)]
use crate::query::executor::base::Executor;
use crate::query::executor::base::{
    BaseExecutor, ExecutionResult, ExecutorConfig, PatternApplyConfig,
};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::DefaultExpressionContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

fn execution_result_to_values(result: &ExecutionResult) -> Result<Vec<Value>, DBError> {
    match result {
        ExecutionResult::DataSet(dataset) => {
            let values: Vec<Value> = dataset
                .rows
                .iter()
                .flat_map(|row| row.iter().cloned())
                .collect();
            Ok(values)
        }
        ExecutionResult::Empty | ExecutionResult::Success | ExecutionResult::SpaceSwitched(_) => {
            Ok(Vec::new())
        }
        ExecutionResult::Error(msg) => Err(DBError::query(msg.clone())),
    }
}

pub struct PatternApplyExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    left_input_var: String,
    right_input_var: String,
    key_cols: Vec<Expression>,
    col_names: Vec<String>,
    is_anti_predicate: bool,
}

impl<S: StorageClient + Send + 'static> PatternApplyExecutor<S> {
    pub fn new(base_config: ExecutorConfig<S>, config: PatternApplyConfig) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "PatternApplyExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            left_input_var: config.left_input_var,
            right_input_var: config.right_input_var,
            key_cols: config.key_cols,
            col_names: config.col_names,
            is_anti_predicate: config.is_anti_predicate,
        }
    }

    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        context: crate::query::executor::base::ExecutionContext,
        config: PatternApplyConfig,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context(
                id,
                "PatternApplyExecutor".to_string(),
                storage,
                context,
            ),
            left_input_var: config.left_input_var,
            right_input_var: config.right_input_var,
            key_cols: config.key_cols,
            col_names: config.col_names,
            is_anti_predicate: config.is_anti_predicate,
        }
    }

    fn check_bi_input_data_sets(&self) -> DBResult<()> {
        let _left_result = self
            .base
            .context
            .get_result(&self.left_input_var)
            .ok_or_else(|| {
                DBError::query(format!(
                    "Left input variable '{}' not found",
                    self.left_input_var
                ))
            })?;

        let _right_result = self
            .base
            .context
            .get_result(&self.right_input_var)
            .ok_or_else(|| {
                DBError::query(format!(
                    "Right input variable '{}' not found",
                    self.right_input_var
                ))
            })?;

        Ok(())
    }

    fn collect_valid_keys(&self, values: &[Value]) -> DBResult<HashSet<List>> {
        let mut valid_keys = HashSet::new();
        let mut expr_context = DefaultExpressionContext::new();

        for value in values {
            expr_context.set_variable("_".to_string(), value.clone());

            if self.key_cols.is_empty() {
                continue;
            }

            let mut key_list = List {
                values: Vec::with_capacity(self.key_cols.len()),
            };

            for col in &self.key_cols {
                let val = ExpressionEvaluator::evaluate(col, &mut expr_context)
                    .map_err(|e| DBError::query(e.to_string()))?;
                key_list.values.push(val);
            }

            valid_keys.insert(key_list);
        }

        Ok(valid_keys)
    }

    fn collect_valid_single_key(&self, values: &[Value]) -> DBResult<HashSet<Value>> {
        let mut valid_keys = HashSet::new();
        let mut expr_context = DefaultExpressionContext::new();

        for value in values {
            expr_context.set_variable("_".to_string(), value.clone());

            if self.key_cols.is_empty() {
                continue;
            }

            let val = ExpressionEvaluator::evaluate(&self.key_cols[0], &mut expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;
            valid_keys.insert(val);
        }

        Ok(valid_keys)
    }

    fn execute_pattern_apply(&mut self) -> DBResult<DataSet> {
        self.check_bi_input_data_sets()?;

        let left_result = self
            .base
            .context
            .get_result(&self.left_input_var)
            .ok_or_else(|| {
                DBError::query(format!(
                    "Left input variable '{}' not found",
                    self.left_input_var
                ))
            })?;

        let right_result = self
            .base
            .context
            .get_result(&self.right_input_var)
            .ok_or_else(|| {
                DBError::query(format!(
                    "Right input variable '{}' not found",
                    self.right_input_var
                ))
            })?;

        let left_values = execution_result_to_values(&left_result)?;
        let right_values = execution_result_to_values(&right_result)?;

        let mut expr_context = DefaultExpressionContext::new();

        let result = if self.key_cols.is_empty() {
            let all_valid = !right_values.is_empty();
            let final_valid = all_valid ^ self.is_anti_predicate;
            self.apply_zero_key(&left_values, final_valid)
        } else if self.key_cols.len() == 1 {
            let valid_keys = self.collect_valid_single_key(&right_values)?;
            self.apply_single_key(&left_values, &valid_keys, &mut expr_context)?
        } else {
            let valid_keys = self.collect_valid_keys(&right_values)?;
            self.apply_multi_key(&left_values, &valid_keys, &mut expr_context)?
        };

        Ok(result)
    }

    fn apply_zero_key(&self, left_values: &[Value], all_valid: bool) -> DataSet {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        if all_valid {
            for value in left_values {
                dataset.rows.push(vec![value.clone()]);
            }
        }

        dataset
    }

    fn apply_single_key<C: ExpressionContext + Send>(
        &self,
        left_values: &[Value],
        valid_keys: &HashSet<Value>,
        expr_context: &mut C,
    ) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        for value in left_values {
            expr_context.set_variable("_".to_string(), value.clone());

            let key_val = ExpressionEvaluator::evaluate(&self.key_cols[0], expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            let apply_flag = (valid_keys.contains(&key_val)) ^ self.is_anti_predicate;

            if apply_flag {
                dataset.rows.push(vec![value.clone()]);
            }
        }

        Ok(dataset)
    }

    fn apply_multi_key<C: ExpressionContext + Send>(
        &self,
        left_values: &[Value],
        valid_keys: &HashSet<List>,
        expr_context: &mut C,
    ) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        for value in left_values {
            expr_context.set_variable("_".to_string(), value.clone());

            let mut key_list = List {
                values: Vec::with_capacity(self.key_cols.len()),
            };

            for col in &self.key_cols {
                let val = ExpressionEvaluator::evaluate(col, expr_context)
                    .map_err(|e| DBError::query(e.to_string()))?;
                key_list.values.push(val);
            }

            let apply_flag = (valid_keys.contains(&key_list)) ^ self.is_anti_predicate;

            if apply_flag {
                dataset.rows.push(vec![value.clone()]);
            }
        }

        Ok(dataset)
    }
}

impl_executor_with_execute!(PatternApplyExecutor, execute_pattern_apply);
impl_has_storage!(PatternApplyExecutor);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_pattern_apply_single_key_positive() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());

        let left_values = vec![Value::Int(1), Value::Int(2), Value::Int(3)];
        let right_values = vec![Value::Int(2), Value::Int(4)];
        let left_dataset = DataSet::from_rows(
            left_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );
        let right_dataset = DataSet::from_rows(
            right_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let key_cols = vec![Expression::variable("_")];
        let config = PatternApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            key_cols,
            col_names: vec!["matched".to_string()],
            is_anti_predicate: false,
        };

        let mut executor = PatternApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute pattern apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 1);
            assert_eq!(dataset.rows[0][0], Value::Int(2));
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_pattern_apply_single_key_anti_predicate() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());

        let left_values = vec![Value::Int(1), Value::Int(2), Value::Int(3)];
        let right_values = vec![Value::Int(2), Value::Int(4)];
        let left_dataset = DataSet::from_rows(
            left_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );
        let right_dataset = DataSet::from_rows(
            right_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let key_cols = vec![Expression::variable("_")];
        let config = PatternApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            key_cols,
            col_names: vec!["matched".to_string()],
            is_anti_predicate: true,
        };

        let mut executor = PatternApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute pattern apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 2);
            let result_values: Vec<Value> = dataset.rows.iter().map(|r| r[0].clone()).collect();
            assert!(result_values.contains(&Value::Int(1)));
            assert!(result_values.contains(&Value::Int(3)));
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_pattern_apply_zero_key_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());

        let left_values = vec![Value::Int(1), Value::Int(2)];
        let right_values = vec![Value::Int(10), Value::Int(20)];
        let left_dataset = DataSet::from_rows(
            left_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );
        let right_dataset = DataSet::from_rows(
            right_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let config = PatternApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            key_cols: vec![],
            col_names: vec!["matched".to_string()],
            is_anti_predicate: false,
        };

        let mut executor = PatternApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute pattern apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 2);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_pattern_apply_zero_key_not_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());

        let left_values = vec![Value::Int(1), Value::Int(2)];
        let right_values: Vec<Value> = vec![];
        let left_dataset = DataSet::from_rows(
            left_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );
        let right_dataset = DataSet::from_rows(
            right_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let config = PatternApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            key_cols: vec![],
            col_names: vec!["matched".to_string()],
            is_anti_predicate: false,
        };

        let mut executor = PatternApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute pattern apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert!(dataset.rows.is_empty());
        } else {
            panic!("Expected DataSet result");
        }
    }
}

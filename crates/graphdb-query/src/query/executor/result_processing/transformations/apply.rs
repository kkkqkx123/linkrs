//! Implementation of ApplyExecutor
//!
//! Handles Standard Apply for LATERAL correlated subqueries.
//! For each row from the left input, evaluates correlated column expressions
//! and joins with matching rows from the right input.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::value::list::List;
use crate::core::{Expression, Value};
#[cfg(test)]
use crate::query::executor::base::Executor;
use crate::query::executor::base::{
    BaseExecutor, ExecutionResult, ExecutorConfig, ApplyConfig,
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

pub struct ApplyExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    left_input_var: String,
    right_input_var: String,
    correlated_cols: Vec<Expression>,
    col_names: Vec<String>,
}

impl<S: StorageClient + Send + 'static> ApplyExecutor<S> {
    pub fn new(base_config: ExecutorConfig<S>, config: ApplyConfig) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "ApplyExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            left_input_var: config.left_input_var,
            right_input_var: config.right_input_var,
            correlated_cols: config.correlated_cols,
            col_names: config.col_names,
        }
    }

    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        context: crate::query::executor::base::ExecutionContext,
        config: ApplyConfig,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context(
                id,
                "ApplyExecutor".to_string(),
                storage,
                context,
            ),
            left_input_var: config.left_input_var,
            right_input_var: config.right_input_var,
            correlated_cols: config.correlated_cols,
            col_names: config.col_names,
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

    fn build_correlated_key<C: ExpressionContext>(
        &self,
        expr_context: &mut C,
    ) -> DBResult<List> {
        let mut key_list = List {
            values: Vec::with_capacity(self.correlated_cols.len()),
        };

        for col in &self.correlated_cols {
            let val = ExpressionEvaluator::evaluate(col, expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;
            key_list.values.push(val);
        }

        Ok(key_list)
    }

    fn execute_apply(&mut self) -> DBResult<DataSet> {
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

        let result = if self.correlated_cols.is_empty() {
            self.apply_zero_key(&left_values, &right_values)
        } else if self.correlated_cols.len() == 1 {
            let valid_keys = self.collect_valid_single_keys(&right_values, &mut expr_context)?;
            self.apply_single_key(&left_values, &valid_keys, &mut expr_context)?
        } else {
            let valid_keys = self.collect_valid_keys(&right_values, &mut expr_context)?;
            self.apply_multi_key(&left_values, &valid_keys, &mut expr_context)?
        };

        Ok(result)
    }

    fn collect_valid_keys(&self, right_values: &[Value], expr_context: &mut DefaultExpressionContext) -> DBResult<HashMap<usize, List>> {
        let mut valid_keys = HashMap::new();

        for (idx, value) in right_values.iter().enumerate() {
            expr_context.set_variable("_".to_string(), value.clone());

            let mut key_list = List {
                values: Vec::with_capacity(self.correlated_cols.len()),
            };

            for col in &self.correlated_cols {
                let val = ExpressionEvaluator::evaluate(col, expr_context)
                    .map_err(|e| DBError::query(e.to_string()))?;
                key_list.values.push(val);
            }

            valid_keys.entry(idx).or_insert(key_list);
        }

        Ok(valid_keys)
    }

    fn collect_valid_single_keys(&self, right_values: &[Value], expr_context: &mut DefaultExpressionContext) -> DBResult<HashMap<usize, Value>> {
        let mut valid_keys = HashMap::new();

        for (idx, value) in right_values.iter().enumerate() {
            expr_context.set_variable("_".to_string(), value.clone());

            let val = ExpressionEvaluator::evaluate(&self.correlated_cols[0], expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            valid_keys.entry(idx).or_insert(val);
        }

        Ok(valid_keys)
    }

    fn apply_zero_key(&self, left_values: &[Value], right_values: &[Value]) -> DataSet {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        if right_values.is_empty() {
            return dataset;
        }

        dataset.rows.reserve(left_values.len() * right_values.len());

        for left_value in left_values {
            for right_value in right_values {
                let mut row = vec![left_value.clone()];
                row.push(right_value.clone());
                dataset.rows.push(row);
            }
        }

        dataset
    }

    fn apply_single_key<C: ExpressionContext + Send>(
        &self,
        left_values: &[Value],
        right_keys: &HashMap<usize, Value>,
        expr_context: &mut C,
    ) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        let right_result = self
            .base
            .context
            .get_result(&self.right_input_var)
            .ok_or_else(|| DBError::query("Right input not found".to_string()))?;
        let right_values = execution_result_to_values(&right_result)?;

        for left_value in left_values {
            expr_context.set_variable("_".to_string(), left_value.clone());

            let left_key = ExpressionEvaluator::evaluate(&self.correlated_cols[0], expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            for (idx, right_key) in right_keys {
                if left_key == *right_key {
                    let mut row = vec![left_value.clone()];
                    row.push(right_values[*idx].clone());
                    dataset.rows.push(row);
                }
            }
        }

        Ok(dataset)
    }

    fn apply_multi_key<C: ExpressionContext + Send>(
        &self,
        left_values: &[Value],
        right_keys: &HashMap<usize, List>,
        expr_context: &mut C,
    ) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        let right_result = self
            .base
            .context
            .get_result(&self.right_input_var)
            .ok_or_else(|| DBError::query("Right input not found".to_string()))?;
        let right_values = execution_result_to_values(&right_result)?;

        for left_value in left_values {
            expr_context.set_variable("_".to_string(), left_value.clone());

            let left_key = self.build_correlated_key(expr_context)?;

            for (idx, right_key) in right_keys {
                if left_key == *right_key {
                    let mut row = vec![left_value.clone()];
                    row.push(right_values[*idx].clone());
                    dataset.rows.push(row);
                }
            }
        }

        Ok(dataset)
    }
}

impl_executor_with_execute!(ApplyExecutor, execute_apply);
impl_has_storage!(ApplyExecutor);

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
    fn test_apply_single_key_positive() {
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

        let correlated_cols = vec![Expression::variable("_")];
        let config = ApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            correlated_cols,
            col_names: vec!["left".to_string(), "right".to_string()],
        };

        let mut executor = ApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 1);
            assert_eq!(dataset.rows[0][0], Value::Int(2));
            assert_eq!(dataset.rows[0][1], Value::Int(2));
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_apply_zero_key() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
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

        let config = ApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            correlated_cols: vec![],
            col_names: vec!["left".to_string(), "right".to_string()],
        };

        let mut executor = ApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 4);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_apply_multi_key() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());

        let left_values = vec![
            Value::from((1, "A")),
            Value::from((1, "B")),
            Value::from((2, "A")),
        ];
        let right_values = vec![
            Value::from((1, "A")),
            Value::from((1, "B")),
            Value::from((1, "C")),
            Value::from((2, "A")),
        ];
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

        let correlated_cols = vec![
            Expression::subscript(Expression::variable("_"), Expression::literal(0i64)),
            Expression::subscript(Expression::variable("_"), Expression::literal(1i64)),
        ];
        let config = ApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            correlated_cols,
            col_names: vec!["left".to_string(), "right".to_string()],
        };

        let mut executor = ApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 3);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_apply_empty_right() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
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

        let correlated_cols = vec![Expression::variable("_")];
        let config = ApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            correlated_cols,
            col_names: vec!["left".to_string(), "right".to_string()],
        };

        let mut executor = ApplyExecutor::with_context(1, storage, context, config);

        let result = executor.execute().expect("Failed to execute apply");
        if let ExecutionResult::DataSet(dataset) = result {
            assert!(dataset.rows.is_empty());
        } else {
            panic!("Expected DataSet result");
        }
    }
}

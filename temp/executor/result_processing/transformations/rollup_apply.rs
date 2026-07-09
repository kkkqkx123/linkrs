//! Implementation of RollUpApplyExecutor
//!
//! Responsible for handling aggregation operations, which involve aggregating the values from the right input based on the keys from the left input.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::value::list::List;
use crate::core::{Expression, Path, Value};
#[cfg(test)]
use crate::query::executor::base::Executor;
use crate::query::executor::base::{
    BaseExecutor, ExecutionResult, ExecutorConfig, RollupApplyConfig,
};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::DefaultExpressionContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// RollUpApply executor
/// Used to aggregate the values from the right input based on the keys from the left input.
pub struct RollUpApplyExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    left_input_var: String,
    right_input_var: String,
    compare_cols: Vec<Expression>,
    collect_col: Expression,
    col_names: Vec<String>,
    movable: bool,
    path_mode: bool,
}

impl<S: StorageClient + Send + 'static> RollUpApplyExecutor<S> {
    pub fn new(base_config: ExecutorConfig<S>, config: RollupApplyConfig) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "RollUpApplyExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            left_input_var: config.left_input_var,
            right_input_var: config.right_input_var,
            compare_cols: config.compare_cols,
            collect_col: config.collect_col,
            col_names: config.col_names,
            movable: false,
            path_mode: false,
        }
    }

    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        context: crate::query::executor::base::ExecutionContext,
        config: RollupApplyConfig,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context(
                id,
                "RollUpApplyExecutor".to_string(),
                storage,
                context,
            ),
            left_input_var: config.left_input_var,
            right_input_var: config.right_input_var,
            compare_cols: config.compare_cols,
            collect_col: config.collect_col,
            col_names: config.col_names,
            movable: false,
            path_mode: false,
        }
    }

    pub fn with_path_mode(mut self, path_mode: bool) -> Self {
        self.path_mode = path_mode;
        self
    }

    fn build_path(&self, values: &[Value]) -> DBResult<Path> {
        let first_value = values
            .first()
            .ok_or_else(|| DBError::query("Path must have at least one vertex".to_string()))?;

        let first_vertex = match first_value {
            Value::Vertex(v) => v.as_ref().clone(),
            _ => return Err(DBError::query("First value must be a vertex".to_string())),
        };

        let mut path = Path::new(first_vertex);

        for value in values.iter().skip(1) {
            match value {
                Value::Edge(edge) => {
                    path.add_step(crate::core::vertex_edge_path::Step::new_with_edge(
                        crate::core::vertex_edge_path::Vertex::with_vid(*edge.dst()),
                        (**edge).clone(),
                    ));
                }
                Value::Vertex(vertex) => {
                    path.add_step(crate::core::vertex_edge_path::Step::new(
                        vertex.as_ref().clone(),
                        String::new(),
                        String::new(),
                        0,
                    ));
                }
                _ => return Err(DBError::query(format!("Invalid path element: {:?}", value))),
            }
        }

        Ok(path)
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

    fn build_hash_table<C: ExpressionContext>(
        &self,
        compare_cols: &[Expression],
        collect_col: &Expression,
        iter: &[Value],
        hash_table: &mut HashMap<List, List>,
        expr_context: &mut C,
    ) -> DBResult<()> {
        for value in iter {
            expr_context.set_variable("_".to_string(), value.clone());

            let mut key_list = List { values: Vec::new() };
            for col in compare_cols {
                let val = ExpressionEvaluator::evaluate(col, expr_context)
                    .map_err(|e| DBError::query(e.to_string()))?;
                key_list.values.push(val);
            }

            let collect_val = ExpressionEvaluator::evaluate(collect_col, expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            let entry = hash_table
                .entry(key_list)
                .or_insert_with(|| List { values: Vec::new() });
            entry.values.push(collect_val);
        }

        Ok(())
    }

    fn build_single_key_hash_table<C: ExpressionContext>(
        &self,
        compare_col: &Expression,
        collect_col: &Expression,
        iter: &[Value],
        hash_table: &mut HashMap<Value, List>,
        expr_context: &mut C,
    ) -> DBResult<()> {
        for value in iter {
            expr_context.set_variable("_".to_string(), value.clone());

            let key_val = ExpressionEvaluator::evaluate(compare_col, expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            let collect_val = ExpressionEvaluator::evaluate(collect_col, expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            let entry = hash_table
                .entry(key_val)
                .or_insert_with(|| List { values: Vec::new() });
            entry.values.push(collect_val);
        }

        Ok(())
    }

    fn build_zero_key_hash_table<C: ExpressionContext>(
        &self,
        collect_col: &Expression,
        iter: &[Value],
        hash_table: &mut List,
        expr_context: &mut C,
    ) -> DBResult<()> {
        hash_table.values.reserve(iter.len());

        for value in iter {
            expr_context.set_variable("_".to_string(), value.clone());

            let collect_val = ExpressionEvaluator::evaluate(collect_col, expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            hash_table.values.push(collect_val);
        }

        Ok(())
    }

    fn probe_zero_key<C: ExpressionContext>(
        &self,
        probe_iter: &[Value],
        hash_table: &List,
        expr_context: &mut C,
    ) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        dataset.rows.reserve(probe_iter.len());

        for value in probe_iter {
            expr_context.set_variable("_".to_string(), value.clone());

            let mut row = Vec::new();

            if self.movable {
                row.push(value.clone());
            }

            if self.path_mode {
                let path = self.build_path(&hash_table.values)?;
                row.push(Value::path(path));
            } else {
                row.push(Value::list(hash_table.clone()));
            }
            dataset.rows.push(row);
        }

        Ok(dataset)
    }

    fn probe_single_key<C: ExpressionContext>(
        &self,
        probe_key: &Expression,
        probe_iter: &[Value],
        hash_table: &HashMap<Value, List>,
        expr_context: &mut C,
    ) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        dataset.rows.reserve(probe_iter.len());

        for value in probe_iter {
            expr_context.set_variable("_".to_string(), value.clone());

            let key_val = ExpressionEvaluator::evaluate(probe_key, expr_context)
                .map_err(|e| DBError::query(e.to_string()))?;

            let vals = hash_table
                .get(&key_val)
                .cloned()
                .unwrap_or(List { values: Vec::new() });

            let mut row = Vec::new();

            if self.movable {
                row.push(value.clone());
            } else {
                row.push(key_val.clone());
            }

            if self.path_mode {
                let path = self.build_path(&vals.values)?;
                row.push(Value::path(path));
            } else {
                row.push(Value::list(vals));
            }
            dataset.rows.push(row);
        }

        Ok(dataset)
    }

    fn probe<C: ExpressionContext>(
        &self,
        probe_keys: &[Expression],
        probe_iter: &[Value],
        hash_table: &HashMap<List, List>,
        expr_context: &mut C,
    ) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        dataset.rows.reserve(probe_iter.len());

        for value in probe_iter {
            expr_context.set_variable("_".to_string(), value.clone());

            let mut key_list = List { values: Vec::new() };
            for col in probe_keys {
                let val = ExpressionEvaluator::evaluate(col, expr_context)
                    .map_err(|e| DBError::query(e.to_string()))?;
                key_list.values.push(val);
            }

            let vals = hash_table
                .get(&key_list)
                .cloned()
                .unwrap_or(List { values: Vec::new() });

            let mut row = Vec::new();

            if self.movable {
                row.push(value.clone());
            }

            if self.path_mode {
                let path = self.build_path(&vals.values)?;
                row.push(Value::path(path));
            } else {
                row.push(Value::list(vals));
            }
            dataset.rows.push(row);
        }

        Ok(dataset)
    }

    fn execute_rollup_apply(&mut self) -> DBResult<DataSet> {
        self.check_bi_input_data_sets()?;

        let left_result = self
            .base
            .context
            .get_result(&self.left_input_var)
            .expect("Context should have left result");
        let right_result = self
            .base
            .context
            .get_result(&self.right_input_var)
            .expect("Context should have right result");

        let left_values: Vec<Value> = match left_result {
            ExecutionResult::DataSet(dataset) => dataset
                .rows
                .into_iter()
                .flat_map(|row| row.into_iter())
                .collect(),
            _ => return Err(DBError::query("Invalid left input result type".to_string())),
        };

        let right_values: Vec<Value> = match right_result {
            ExecutionResult::DataSet(dataset) => dataset
                .rows
                .into_iter()
                .flat_map(|row| row.into_iter())
                .collect(),
            _ => {
                return Err(DBError::query(
                    "Invalid right input result type".to_string(),
                ))
            }
        };

        let mut expr_context = DefaultExpressionContext::new();

        let result = if self.compare_cols.is_empty() {
            let mut hash_table = List { values: Vec::new() };
            self.build_zero_key_hash_table(
                &self.collect_col,
                &right_values,
                &mut hash_table,
                &mut expr_context,
            )?;
            self.probe_zero_key(&left_values, &hash_table, &mut expr_context)?
        } else if self.compare_cols.len() == 1 {
            let mut hash_table = HashMap::new();
            self.build_single_key_hash_table(
                &self.compare_cols[0],
                &self.collect_col,
                &right_values,
                &mut hash_table,
                &mut expr_context,
            )?;
            self.probe_single_key(
                &self.compare_cols[0],
                &left_values,
                &hash_table,
                &mut expr_context,
            )?
        } else {
            let mut hash_table = HashMap::new();
            self.build_hash_table(
                &self.compare_cols,
                &self.collect_col,
                &right_values,
                &mut hash_table,
                &mut expr_context,
            )?;
            self.probe(
                &self.compare_cols,
                &left_values,
                &hash_table,
                &mut expr_context,
            )?
        };

        Ok(result)
    }
}

impl_executor_with_execute!(RollUpApplyExecutor, execute_rollup_apply);
impl_has_storage!(RollUpApplyExecutor);

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
    fn test_rollup_apply_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        let left_dataset = DataSet::from_rows(
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            vec!["key".to_string()],
        );
        let right_dataset = DataSet::from_rows(
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(1)],
                vec![Value::Int(2)],
            ],
            vec!["key".to_string()],
        );

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());
        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let compare_cols = vec![Expression::variable("_")];
        let collect_col = Expression::variable("_");

        let config = RollupApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            compare_cols,
            collect_col,
            col_names: vec!["key".to_string(), "collected".to_string()],
        };

        let mut executor = RollUpApplyExecutor::with_context(1, storage, context, config);

        let result = executor
            .execute()
            .expect("Executor should execute successfully");

        if let ExecutionResult::DataSet(dataset) = result {
            // Result should have same number of rows as left input (2 rows)
            assert_eq!(dataset.rows.len(), 2);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_rollup_apply_zero_key() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let left_values = vec![Value::Int(1), Value::Int(2), Value::Int(3)];
        let right_values = vec![Value::Int(10), Value::Int(20)];
        let left_dataset = DataSet::from_rows(
            left_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );
        let right_dataset = DataSet::from_rows(
            right_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());
        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let compare_cols: Vec<Expression> = vec![];
        let collect_col = Expression::Variable("_".to_string());

        let config = RollupApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            compare_cols,
            collect_col,
            col_names: vec!["collected".to_string()],
        };

        let mut executor = RollUpApplyExecutor::with_context(2, storage, context, config);

        let result = executor
            .execute()
            .expect("Executor should execute successfully");

        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 3);
            for row in &dataset.rows {
                match &row[0] {
                    Value::List(list) => {
                        assert_eq!(list.len(), 2);
                    }
                    _ => panic!("Expected List value"),
                }
            }
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_rollup_apply_multi_key() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

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

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());
        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let compare_cols = vec![
            Expression::subscript(Expression::variable("_"), Expression::literal(0i64)),
            Expression::subscript(Expression::variable("_"), Expression::literal(1i64)),
        ];
        let collect_col = Expression::Variable("_".to_string());

        let config = RollupApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            compare_cols,
            collect_col,
            col_names: vec![
                "key0".to_string(),
                "key1".to_string(),
                "collected".to_string(),
            ],
        };

        let mut executor = RollUpApplyExecutor::with_context(3, storage, context, config);

        let result = executor
            .execute()
            .expect("Executor should execute successfully");

        if let ExecutionResult::DataSet(dataset) = result {
            assert_eq!(dataset.rows.len(), 3);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_rollup_apply_empty_right() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

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

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());
        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let compare_cols = vec![Expression::variable("_")];
        let collect_col = Expression::Variable("_".to_string());

        let config = RollupApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            compare_cols,
            collect_col,
            col_names: vec!["key".to_string(), "collected".to_string()],
        };

        let mut executor = RollUpApplyExecutor::with_context(4, storage, context, config);

        let result = executor
            .execute()
            .expect("Executor should execute successfully");

        if let ExecutionResult::DataSet(dataset) = result {
            // Result should have same number of rows as left input (2 rows)
            // Each row should have an empty list for the collected values
            assert_eq!(dataset.rows.len(), 2);
            assert_eq!(dataset.rows[0][0], Value::Int(1));
            assert_eq!(dataset.rows[0][1], Value::list(List::from(Vec::new())));
            assert_eq!(dataset.rows[1][0], Value::Int(2));
            assert_eq!(dataset.rows[1][1], Value::list(List::from(Vec::new())));
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_rollup_apply_empty_left() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let left_values: Vec<Value> = vec![];
        let right_values = vec![Value::Int(1), Value::Int(2)];

        let left_dataset = DataSet::from_rows(
            left_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );
        let right_dataset = DataSet::from_rows(
            right_values.clone().into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());
        context.set_result("left".to_string(), ExecutionResult::DataSet(left_dataset));
        context.set_result("right".to_string(), ExecutionResult::DataSet(right_dataset));

        let compare_cols = vec![Expression::literal(0i64)];
        let collect_col = Expression::Variable("_".to_string());

        let config = RollupApplyConfig {
            left_input_var: "left".to_string(),
            right_input_var: "right".to_string(),
            compare_cols,
            collect_col,
            col_names: vec!["key".to_string(), "collected".to_string()],
        };

        let mut executor = RollUpApplyExecutor::with_context(5, storage, context, config);

        let result = executor
            .execute()
            .expect("Executor should execute successfully");

        if let ExecutionResult::DataSet(dataset) = result {
            assert!(dataset.rows.is_empty());
        } else {
            panic!("Expected DataSet result");
        }
    }
}

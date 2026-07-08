//! Implementation of UnwindExecutor
//!
//! Responsible for handling the list expansion process, expanding each element in the list into a separate row.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::{Expression, Value};
use crate::query::executor::base::{
    BaseExecutor, ExecutionResult, Executor, ExecutorEnum, InputExecutor,
};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::{
    DefaultExpressionContext, ExpressionContext as EvalContext,
};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Unwind Actuator
/// Used to expand each element in the list into a separate row.
pub struct UnwindExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    /// The expression to be expanded
    unwind_expression: Expression,
    /// Column names
    col_names: Vec<String>,
    /// Input executor
    input_executor: Option<Box<ExecutorEnum<S>>>,
}

impl<S: StorageClient + Send + 'static> UnwindExecutor<S> {
    /// Create a new UnwindExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        _input_var: String,
        unwind_expression: Expression,
        col_names: Vec<String>,
        _from_pipe: bool,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "UnwindExecutor".to_string(), storage, expr_context),
            unwind_expression,
            col_names,
            input_executor: None,
        }
    }

    /// Create an UnwindExecutor with context information
    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        _input_var: String,
        unwind_expression: Expression,
        col_names: Vec<String>,
        _from_pipe: bool,
        context: crate::query::executor::base::ExecutionContext,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context(id, "UnwindExecutor".to_string(), storage, context),
            unwind_expression,
            col_names,
            input_executor: None,
        }
    }

    /// Extract a list from a value.
    fn extract_list(&self, val: &Value) -> Vec<Value> {
        match val {
            Value::List(list) => list.clone().into_vec(),
            Value::Null(_) | Value::Empty => vec![],
            _ => vec![val.clone()],
        }
    }

    fn execute_unwind(&mut self) -> DBResult<DataSet> {
        let mut expr_context = DefaultExpressionContext::new();
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            ExecutionResult::DataSet(DataSet::new())
        };

        match input_result {
            ExecutionResult::DataSet(input_data) => {
                let col_names = input_data.col_names.clone();
                if input_data.rows.is_empty() {
                    let unwind_value =
                        ExpressionEvaluator::evaluate(&self.unwind_expression, &mut expr_context)
                            .map_err(|e| DBError::query(e.to_string()))?;

                    let list_values = self.extract_list(&unwind_value);

                    for list_item in list_values {
                        dataset.rows.push(vec![list_item]);
                    }
                } else {
                    for row in input_data.rows {
                        for (i, value) in row.iter().enumerate() {
                            if i < col_names.len() {
                                expr_context.set_variable(col_names[i].clone(), value.clone());

                                if col_names[i].contains('.') {
                                    if let Some(dot_pos) = col_names[i].find('.') {
                                        let var_name = &col_names[i][..dot_pos];
                                        expr_context
                                            .set_variable(var_name.to_string(), value.clone());
                                    }
                                }
                            }
                        }

                        let unwind_value = ExpressionEvaluator::evaluate(
                            &self.unwind_expression,
                            &mut expr_context,
                        )
                        .map_err(|e| DBError::query(e.to_string()))?;

                        let list_values = self.extract_list(&unwind_value);

                        for list_item in list_values {
                            let mut new_row = row.clone();
                            new_row.push(list_item);
                            dataset.rows.push(new_row);
                        }
                    }
                }
            }
            ExecutionResult::Success
            | ExecutionResult::Empty
            | ExecutionResult::SpaceSwitched(_) => {
                let unwind_value =
                    ExpressionEvaluator::evaluate(&self.unwind_expression, &mut expr_context)
                        .map_err(|e| DBError::query(e.to_string()))?;

                let list_values = self.extract_list(&unwind_value);

                for list_item in list_values {
                    dataset.rows.push(vec![list_item]);
                }
            }
            ExecutionResult::Error(e) => {
                return Err(DBError::query(format!("Error in input result: {}", e)));
            }
        }

        Ok(dataset)
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for UnwindExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_ref().map(|b| b.as_ref())
    }
}

impl_executor_with_execute!(UnwindExecutor, execute_unwind);
impl_has_storage!(UnwindExecutor);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Expression, List, Value};
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_unwind_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        let list_value = Value::list(List::from(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ]));

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context);

        let unwind_expression = Expression::Literal(list_value.clone());
        let mut executor = UnwindExecutor::with_context(
            1,
            storage,
            "input".to_string(),
            unwind_expression,
            vec!["unwound".to_string()],
            false,
            context,
        );

        let result = executor
            .execute()
            .expect("Executor should execute successfully");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 3);
                assert_eq!(dataset.rows[0].len(), 1);
                assert_eq!(dataset.rows[0][0], Value::Int(1));
                assert_eq!(dataset.rows[1][0], Value::Int(2));
                assert_eq!(dataset.rows[2][0], Value::Int(3));
            }
            _ => panic!("Expected DataSet result"),
        }
    }
}

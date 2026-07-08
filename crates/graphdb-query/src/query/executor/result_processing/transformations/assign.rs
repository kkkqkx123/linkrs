//! Implementation of AssignExecutor
//!
//! Responsible for handling variable assignment operations, assigning the results of expressions to variables.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::Expression;
use crate::core::Value;
use crate::query::executor::base::BaseExecutor;
use crate::query::executor::base::{ExecutionResult, Executor};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Assign an executor
/// Used to assign the result of an expression to a variable
pub struct AssignExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    /// List of assignment items (variable name, expression)
    assign_items: Vec<(String, Expression)>,
}

impl<S: StorageClient + Send + 'static> AssignExecutor<S> {
    /// Create a new AssignExecutor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        assign_items: Vec<(String, Expression)>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "AssignExecutor".to_string(), storage, expr_context),
            assign_items,
        }
    }

    /// Create an AssignExecutor with context.
    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        assign_items: Vec<(String, Expression)>,
        context: crate::query::executor::base::ExecutionContext,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context_and_description(
                id,
                "AssignExecutor".to_string(),
                "Assign executor - assigns expression results to variables".to_string(),
                storage,
                context,
            ),
            assign_items,
        }
    }

    /// Perform an assignment operation
    fn execute_assign(&mut self) -> DBResult<()> {
        for (var_name, expression) in &self.assign_items {
            let value = ExpressionEvaluator::evaluate(expression, &mut self.base.context)
                .map_err(|e| DBError::query(e.to_string()))?;

            match &value {
                Value::DataSet(dataset) => {
                    self.base.context.set_result(
                        var_name.clone(),
                        ExecutionResult::DataSet((**dataset).clone()),
                    );
                }
                _ => {
                    self.base.context.set_result(
                        var_name.clone(),
                        ExecutionResult::DataSet(DataSet::from_rows(
                            vec![vec![value.clone()]],
                            vec![var_name.clone()],
                        )),
                    );
                }
            }

            self.base.context.set_variable(var_name.clone(), value);
        }

        Ok(())
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for AssignExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self.execute_assign()?;
        Ok(ExecutionResult::Success)
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl_has_storage!(AssignExecutor);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_assign_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        // Create an assignment item
        let assign_items = vec![
            ("var1".to_string(), Expression::int(42)),
            ("var2".to_string(), Expression::literal("hello")),
        ];

        let mut executor = AssignExecutor::new(1, storage, assign_items, expr_context);

        // Performing an assignment
        let result = executor
            .execute()
            .expect("Executor should execute successfully");
        assert!(matches!(result, ExecutionResult::Success));

        // Check whether the variables are set correctly.
        assert_eq!(
            executor.base.context.get_variable("var1"),
            Some(Value::Int(42))
        );
        assert_eq!(
            executor.base.context.get_variable("var2"),
            Some(Value::String("hello".to_string()))
        );
    }
}

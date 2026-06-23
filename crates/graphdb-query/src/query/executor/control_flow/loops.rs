//! Loop Executor Module
//!
//! Actuators related to loop control
//!

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::Expression;
use crate::core::Value;
use crate::query::core::LoopExecutionState;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{
    BaseExecutor, ExecutionResult, Executor, ExecutorConfig, HasStorage, LoopConfig,
};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::DefaultExpressionContext;
use crate::query::executor::utils::recursion_detector::{
    ExecutorSafetyConfig, ExecutorSafetyValidator, RecursionDetector,
};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

// Use new type aliases (LoopState is an alias for LoopExecutionState, for backward compatibility).
pub use crate::query::core::LoopExecutionState as LoopState;

/// LoopExecutor – An executor for loop control and execution
///
/// Implement loop control logic that supports both conditional loops and counting loops.
/// It includes a recursive detection mechanism to prevent the loop executor from self-referencing.
pub struct LoopExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    condition: Option<Expression>,
    body_executor: Box<ExecutorEnum<S>>,
    max_iterations: Option<usize>,
    current_iteration: usize,
    loop_state: LoopExecutionState,
    results: Vec<ExecutionResult>,
    loop_context: DefaultExpressionContext,
    recursion_detector: RecursionDetector,
    safety_validator: ExecutorSafetyValidator,
}

impl<S: StorageClient> LoopExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        condition: Option<Expression>,
        body_executor: ExecutorEnum<S>,
        max_iterations: Option<usize>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        let recursion_detector = RecursionDetector::new(100);
        let safety_validator = ExecutorSafetyValidator::new(ExecutorSafetyConfig::default());

        Self {
            base: BaseExecutor::new(id, "LoopExecutor".to_string(), storage, expr_context),
            condition,
            body_executor: Box::new(body_executor),
            max_iterations,
            current_iteration: 0,
            loop_state: LoopExecutionState::NotStarted,
            results: Vec::new(),
            loop_context: DefaultExpressionContext::new(),
            recursion_detector,
            safety_validator,
        }
    }
}

impl<S: StorageClient + Send + 'static> LoopExecutor<S> {
    /// Verify whether the loop executor contains any self-references.
    pub fn validate_no_self_reference(&self) -> Result<(), DBError> {
        if self.body_executor.id() == self.base.id {
            return Err(DBError::query(
                "Loop executors cannot be self-referential".to_string(),
            ));
        }
        Ok(())
    }

    fn evaluate_condition(&mut self) -> DBResult<bool> {
        match &self.condition {
            Some(expression) => {
                let result = ExpressionEvaluator::evaluate(expression, &mut self.loop_context)
                    .map_err(|e| {
                        DBError::from(
                            crate::query::executor::expression::ExpressionError::function_error(
                                e.to_string(),
                            ),
                        )
                    })?;

                Ok(self.value_to_bool(&result))
            }
            None => Ok(true),
        }
    }

    /// Convert the value to a boolean value.
    fn value_to_bool(&self, value: &Value) -> bool {
        match value {
            Value::Bool(b) => *b,
            Value::Null(_) => false,
            Value::Int(0) => false,
            Value::Float(0.0) => false,
            Value::String(s) if s.is_empty() => false,
            Value::List(l) if l.is_empty() => false,
            Value::Map(m) if m.is_empty() => false,
            _ => true,
        }
    }

    /// Check whether the loop should continue.
    fn should_continue(&self) -> bool {
        if let LoopExecutionState::Error(_) = self.loop_state {
            return false;
        }

        if let Some(max_iter) = self.max_iterations {
            if self.current_iteration >= max_iter {
                return false;
            }
        }

        true
    }

    fn execute_iteration(&mut self) -> DBResult<ExecutionResult> {
        // 注意：current_iteration 已经在 execute() 方法中递增
        self.loop_context.set_variable(
            "__iteration".to_string(),
            Value::BigInt(self.current_iteration as i64),
        );

        let result = self.body_executor.execute()?;

        self.body_executor.close()?;
        self.body_executor.open()?;

        Ok(result)
    }

    /// Collect all the results of the loops.
    fn collect_results(&self) -> ExecutionResult {
        if self.results.is_empty() {
            return ExecutionResult::Success;
        }

        let mut all_datasets = Vec::new();
        let mut has_error = false;

        for result in &self.results {
            match result {
                ExecutionResult::DataSet(dataset) => {
                    all_datasets.push(dataset.clone());
                }
                ExecutionResult::Success
                | ExecutionResult::Empty
                | ExecutionResult::SpaceSwitched(_) => {}
                ExecutionResult::Error(_) => {
                    has_error = true;
                }
            }
        }

        if has_error {
            return ExecutionResult::Error("Loop execution had errors".to_string());
        }

        if all_datasets.is_empty() {
            return ExecutionResult::Success;
        }

        if all_datasets.len() == 1 {
            ExecutionResult::DataSet(
                all_datasets
                    .into_iter()
                    .next()
                    .expect("Failed to get next dataset"),
            )
        } else {
            let first_dataset = all_datasets
                .first()
                .cloned()
                .expect("Failed to get first dataset");
            let mut combined_rows = first_dataset.rows;
            let combined_col_names = first_dataset.col_names;

            for dataset in all_datasets.into_iter().skip(1) {
                combined_rows.extend(dataset.rows);
            }

            ExecutionResult::DataSet(DataSet {
                rows: combined_rows,
                col_names: combined_col_names,
            })
        }
    }

    /// Setting the loop variable
    pub fn set_loop_variable(&mut self, name: String, value: Value) {
        self.loop_context.set_variable(name, value);
    }

    /// Get the current iteration count.
    pub fn current_iteration(&self) -> usize {
        self.current_iteration
    }

    /// Obtaining the cycle status
    pub fn loop_state(&self) -> &LoopState {
        &self.loop_state
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for LoopExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self.validate_no_self_reference()?;

        self.safety_validator
            .validate_loop_config(self.max_iterations)?;

        self.recursion_detector
            .validate_executor(self.body_executor.id(), self.body_executor.name())?;

        self.loop_state = LoopExecutionState::Running { iteration: 0 };
        self.results.clear();
        self.current_iteration = 0;

        self.body_executor.open()?;

        while self.should_continue() {
            self.current_iteration += 1;
            self.loop_state = LoopExecutionState::Running {
                iteration: self.current_iteration,
            };

            self.loop_context.set_variable(
                "__iteration".to_string(),
                Value::BigInt(self.current_iteration as i64),
            );

            let should_continue = match self.evaluate_condition() {
                Ok(continue_flag) => continue_flag,
                Err(e) => {
                    self.loop_state = LoopExecutionState::Error(e.to_string());
                    break;
                }
            };

            if !should_continue {
                break;
            }

            match self.execute_iteration() {
                Ok(result) => {
                    self.results.push(result);
                }
                Err(e) => {
                    self.loop_state = LoopExecutionState::Error(e.to_string());
                    break;
                }
            }
        }

        let _ = self.body_executor.close();

        self.recursion_detector.leave_executor();

        if !matches!(self.loop_state, LoopExecutionState::Error(_)) {
            self.loop_state = LoopExecutionState::Finished;
        }

        Ok(self.collect_results())
    }

    fn open(&mut self) -> DBResult<()> {
        self.loop_state = LoopExecutionState::NotStarted;
        self.current_iteration = 0;
        self.results.clear();
        self.loop_context = DefaultExpressionContext::new();

        self.body_executor.open()?;
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        self.body_executor.close()?;

        self.results.clear();
        self.loop_context = DefaultExpressionContext::new();

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

impl<S: StorageClient + Send + 'static> HasStorage<S> for LoopExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

/// WhileLoopExecutor – An executor for executing conditional loops
///
/// Specifically designed for implementing WHILE loops
pub struct WhileLoopExecutor<S: StorageClient + Send + 'static> {
    inner: LoopExecutor<S>,
}

impl<S: StorageClient + Send + 'static> WhileLoopExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        condition: Expression,
        body_executor: ExecutorEnum<S>,
        max_iterations: Option<usize>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            inner: LoopExecutor::new(
                id,
                storage,
                Some(condition),
                body_executor,
                max_iterations,
                expr_context,
            ),
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for WhileLoopExecutor<S> {
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
        "WhileLoopExecutor"
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

impl<S: StorageClient + Send + 'static> HasStorage<S> for WhileLoopExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.inner.get_storage()
    }
}

/// ForLoopExecutor – An executor for counting loop executions
///
/// Specifically designed for implementing FOR loops
pub struct ForLoopExecutor<S: StorageClient + Send + 'static> {
    inner: LoopExecutor<S>,
    start: i64,
    end: i64,
    step: i64,
    loop_var: String,
}

/// For loop configuration
#[derive(Debug)]
pub struct ForLoopConfig<S: StorageClient + Send + 'static> {
    pub loop_var: String,
    pub start: i64,
    pub end: i64,
    pub step: i64,
    pub body_executor: ExecutorEnum<S>,
}

impl<S: StorageClient + Send + 'static> ForLoopExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
        config: ForLoopConfig<S>,
    ) -> Self {
        let base_config = ExecutorConfig::new(id, storage, expr_context);
        let _loop_config = LoopConfig {
            loop_var: config.loop_var.clone(),
            loop_condition: crate::core::Expression::Literal(crate::core::Value::Bool(true)),
        };

        let max_iterations =
            Some(((config.end - config.start).abs() / config.step.abs() + 1) as usize);

        let mut executor = LoopExecutor::new(
            base_config.id,
            base_config.storage,
            None,
            config.body_executor,
            max_iterations,
            base_config.expr_context,
        );

        executor.set_loop_variable(config.loop_var.clone(), Value::BigInt(config.start));

        Self {
            inner: executor,
            start: config.start,
            end: config.end,
            step: config.step,
            loop_var: config.loop_var,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for ForLoopExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self.inner.open()?;

        let mut current = self.start;
        let mut results = Vec::new();

        while (self.step > 0 && current <= self.end) || (self.step < 0 && current >= self.end) {
            self.inner
                .set_loop_variable(self.loop_var.clone(), Value::BigInt(current));

            let result = self.inner.execute_iteration()?;
            results.push(result);

            current += self.step;
        }

        self.inner.close()?;

        self.inner.results = results;
        self.inner.loop_state = LoopState::Finished;
        self.inner.current_iteration =
            ((self.end - self.start).abs() / self.step.abs() + 1) as usize;
        Ok(self.inner.collect_results())
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
        "ForLoopExecutor"
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

impl<S: StorageClient + Send + 'static> HasStorage<S> for ForLoopExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.inner.get_storage()
    }
}

/// SelectExecutor – An executor for conditional branch execution
///
/// Implement conditional branching logic to choose and execute either the if or else branch based on a specified condition.
pub struct SelectExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    condition: Expression,
    if_branch: Box<ExecutorEnum<S>>,
    else_branch: Option<Box<ExecutorEnum<S>>>,
    current_result: Option<ExecutionResult>,
}

impl<S: StorageClient> SelectExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        condition: Expression,
        if_branch: ExecutorEnum<S>,
        else_branch: Option<ExecutorEnum<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "SelectExecutor".to_string(), storage, expr_context),
            condition,
            if_branch: Box::new(if_branch),
            else_branch: else_branch.map(Box::new),
            current_result: None,
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for SelectExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let mut context = crate::query::executor::expression::DefaultExpressionContext::new();

        let condition_result = ExpressionEvaluator::evaluate(&self.condition, &mut context)
            .map_err(|e| {
                DBError::from(
                    crate::query::executor::expression::ExpressionError::function_error(
                        e.to_string(),
                    ),
                )
            })?;

        let condition_value = match condition_result {
            Value::Bool(b) => b,
            Value::Int(i) => i != 0,
            Value::Float(f) => f != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Map(m) => !m.is_empty(),
            Value::Null(_) => false,
            Value::Empty => false,
            _ => true,
        };

        let branch_to_execute = if condition_value {
            &mut self.if_branch
        } else {
            match self.else_branch {
                Some(ref mut branch) => branch,
                None => {
                    return Ok(ExecutionResult::Success);
                }
            }
        };

        branch_to_execute.open()?;
        let result = branch_to_execute.execute()?;
        branch_to_execute.close()?;

        self.current_result = Some(result.clone());
        Ok(result)
    }

    fn open(&mut self) -> DBResult<()> {
        self.if_branch.open()?;
        if let Some(ref mut else_branch) = self.else_branch {
            else_branch.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        self.if_branch.close()?;
        if let Some(ref mut else_branch) = self.else_branch {
            else_branch.close()?;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.if_branch.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        "Select executor - conditional branch execution"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> HasStorage<S> for SelectExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base
            .storage
            .as_ref()
            .expect("SelectExecutor storage should be set")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::BinaryOperator;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;
    use ExpressionAnalysisContext;

    #[test]
    fn test_while_loop_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let storage_clone = storage.clone();

        let condition = Expression::binary(
            Expression::variable("__iteration"),
            BinaryOperator::LessThan,
            Expression::int(3),
        );

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let body_executor = ExecutorEnum::Base(BaseExecutor::new(
            2,
            "TestExecutor".to_string(),
            storage_clone,
            expr_context.clone(),
        ));

        let mut executor =
            WhileLoopExecutor::new(1, storage, condition, body_executor, Some(5), expr_context);

        let result = executor.execute().expect("Failed to execute");

        match result {
            ExecutionResult::Success => {
                assert_eq!(executor.inner.current_iteration(), 3);
                assert_eq!(executor.inner.loop_state(), &LoopState::Finished);
            }
            _ => panic!("Expected Success result, got: {:?}", result),
        }
    }

    #[test]
    fn test_for_loop_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let storage_clone = storage.clone();

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let body_executor = ExecutorEnum::Base(BaseExecutor::new(
            2,
            "TestExecutor".to_string(),
            storage_clone,
            expr_context.clone(),
        ));

        let config = ForLoopConfig {
            loop_var: "i".to_string(),
            start: 1,
            end: 3,
            step: 1,
            body_executor,
        };
        let mut executor = ForLoopExecutor::new(1, storage, expr_context, config);

        let result = executor.execute().expect("Failed to execute");

        match result {
            ExecutionResult::Success => {
                assert_eq!(executor.inner.current_iteration(), 3);
                assert_eq!(executor.inner.loop_state(), &LoopState::Finished);
            }
            _ => panic!("Expected Success result"),
        }
    }
}

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBResult;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{BaseExecutor, InputExecutor};
use crate::query::executor::base::{ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// ArgumentExecutor – An argument executor
///
/// Used to obtain a named alias from another already executed operation.
pub struct ArgumentExecutor<S: StorageClient + 'static> {
    base: BaseExecutor<S>,
    var: String,
    input_executor: Option<Box<ExecutorEnum<S>>>,
}

impl<S: StorageClient + 'static> ArgumentExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        var: &str,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ArgumentExecutor".to_string(), storage, expr_context),
            var: var.to_string(),
            input_executor: None,
        }
    }

    pub fn var(&self) -> &str {
        &self.var
    }

    /// Set the variable values in the execution context.
    pub fn set_variable(&mut self, name: String, value: crate::core::Value) {
        self.base.context.set_variable(name, value);
    }

    /// Set the intermediate results to the execution context.
    pub fn set_result(&mut self, name: String, result: ExecutionResult) {
        self.base.context.set_result(name, result);
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for ArgumentExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // First, execute the input executor to obtain the results.
        let _input_result = if let Some(input) = &mut self.input_executor {
            input.open()?;
            let result = input.execute()?;
            input.close()?;
            Some(result)
        } else {
            None
        };

        // Obtain the variable values from the execution context.
        if let Some(var_value) = self.base.context.get_variable(&self.var) {
            Ok(ExecutionResult::DataSet(DataSet::from_rows(
                vec![vec![var_value.clone()]],
                vec!["value".to_string()],
            )))
        } else if let Some(result) = self.base.context.get_result(&self.var) {
            Ok(result.clone())
        } else {
            // Return empty dataset for standalone operations (like UNWIND without input)
            Ok(ExecutionResult::DataSet(DataSet::new()))
        }
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
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
        &self.base.description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for ArgumentExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + 'static> HasStorage<S> for ArgumentExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not set")
    }
}

/// PassThroughExecutor – A direct execution engine
///
/// Nodes used for passthrough scenarios: They simply transmit the input data as is.
pub struct PassThroughExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    input_executor: Option<Box<ExecutorEnum<S>>>,
}

impl<S: StorageClient> PassThroughExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "PassThroughExecutor".to_string(), storage, expr_context),
            input_executor: None,
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for PassThroughExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        if let Some(input) = &mut self.input_executor {
            input.open()?;
            let result = input.execute()?;
            input.close()?;
            Ok(result)
        } else {
            Ok(ExecutionResult::Success)
        }
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
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
        &self.base.description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for PassThroughExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + 'static> HasStorage<S> for PassThroughExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not set")
    }
}

/// DataCollectExecutor – The data collection executor
///
/// Used for collecting and aggregating data
pub struct DataCollectExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    input_executor: Option<Box<ExecutorEnum<S>>>,
    collected_data: Vec<ExecutionResult>,
}

impl<S: StorageClient> DataCollectExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DataCollectExecutor".to_string(), storage, expr_context),
            input_executor: None,
            collected_data: Vec::new(),
        }
    }

    pub fn collected_data(&self) -> &[ExecutionResult] {
        &self.collected_data
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for DataCollectExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self.collected_data.clear();

        if let Some(input) = &mut self.input_executor {
            input.open()?;
            let result = input.execute()?;
            input.close()?;
            self.collected_data.push(result);
        }

        Ok(ExecutionResult::Success)
    }

    fn open(&mut self) -> DBResult<()> {
        self.collected_data.clear();
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
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
        &self.base.description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for DataCollectExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_ref().map(|e| e.as_ref())
    }
}

impl<S: StorageClient + Send + 'static> HasStorage<S> for DataCollectExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not set")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::storage::MockStorage;

    #[test]
    fn test_argument_executor_creation() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = ArgumentExecutor::<MockStorage>::new(1, storage, "test_var", expr_context);
        assert_eq!(executor.id(), 1);
        assert_eq!(executor.var(), "test_var");
        assert_eq!(executor.name(), "ArgumentExecutor");
    }

    #[test]
    fn test_argument_executor_with_variable() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ArgumentExecutor::<MockStorage>::new(1, storage, "my_var", expr_context);

        // Set the variable values
        executor.set_variable(
            "my_var".to_string(),
            Value::String("test_value".to_string()),
        );

        // Execute and verify the result.
        executor.open().expect("Failed to open executor");
        let result = executor.execute().expect("Execution failed");
        executor.close().expect("Failed to close executor");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 1);
                assert_eq!(dataset.rows[0][0], Value::String("test_value".to_string()));
            }
            _ => panic!(
                "Expecting to return DataSet results, but getting {:?}",
                result
            ),
        }
    }

    #[test]
    fn test_argument_executor_with_result() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            ArgumentExecutor::<MockStorage>::new(1, storage, "my_result", expr_context);

        // Set intermediate results
        let test_result = ExecutionResult::DataSet(DataSet::from_rows(
            vec![vec![Value::Int(42)]],
            vec!["value".to_string()],
        ));
        executor.set_result("my_result".to_string(), test_result.clone());

        // Execute and verify the result.
        executor.open().expect("Failed to open executor");
        let result = executor.execute().expect("Execution failed");
        executor.close().expect("Failed to close executor");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.rows.len(), 1);
                assert_eq!(dataset.rows[0][0], Value::Int(42));
            }
            _ => panic!(
                "Expecting to return DataSet results, but getting {:?}",
                result
            ),
        }
    }

    #[test]
    fn test_argument_executor_variable_not_found() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            ArgumentExecutor::<MockStorage>::new(1, storage, "undefined_var", expr_context);

        executor.open().expect("Failed to open executor");
        let result = executor.execute();
        executor.close().expect("Failed to close executor");

        assert!(
            result.is_ok(),
            "Should return empty dataset when variable is not defined"
        );
        if let Ok(ExecutionResult::DataSet(dataset)) = result {
            assert!(dataset.rows.is_empty(), "Dataset should be empty");
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_pass_through_executor_creation() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = PassThroughExecutor::<MockStorage>::new(1, storage, expr_context);
        assert_eq!(executor.id(), 1);
        assert_eq!(executor.name(), "PassThroughExecutor");
    }

    #[test]
    fn test_data_collect_executor_creation() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = DataCollectExecutor::<MockStorage>::new(1, storage, expr_context);
        assert_eq!(executor.id(), 1);
        assert_eq!(executor.name(), "DataCollectExecutor");
        assert!(executor.collected_data().is_empty());
    }
}

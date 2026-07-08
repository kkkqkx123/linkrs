//! MaterializeExecutor – The materialization executor
//!
//! Materialize (cache) the input data in memory to optimize the use of Common Table Expressions (CTEs) or subqueries that are referenced multiple times.
//! Avoid duplicate calculations to improve query performance.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor};
use crate::storage::StorageClient;

/// Physical state
#[derive(Debug, Clone, PartialEq)]
pub enum MaterializeState {
    /// Not yet materialized
    Uninitialized,
    /// Materialized; the data is now available.
    Materialized,
    /// The materialization process failed.
    Failed(String),
}

/// MaterializeExecutor – The materialization executor
///
/// Cache the input data in memory; multiple reads are supported.
/// Mainly used for optimizing CTEs (Common Table Expressions) or subqueries that are referenced multiple times.
pub struct MaterializeExecutor<S: StorageClient + Send + 'static> {
    /// Basic Executor
    base: BaseExecutor<S>,
    /// Input actuator
    input_executor: Option<Box<ExecutorEnum<S>>>,
    /// Materialized state
    state: MaterializeState,
    /// Materialized data
    materialized_data: Option<ExecutionResult>,
    /// Memory limit (in bytes)
    memory_limit: usize,
    /// Current memory usage
    current_memory_usage: usize,
    /// Has it been consumed (for the one-time consumption mode)?
    consumed: bool,
}

impl<S: StorageClient + Send + 'static> MaterializeExecutor<S> {
    /// Create a new materialized executor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        memory_limit: Option<usize>,
        expr_context: Arc<crate::query::validator::context::ExpressionAnalysisContext>,
    ) -> Self {
        let base = BaseExecutor::new(id, "MaterializeExecutor".to_string(), storage, expr_context);

        Self {
            base,
            input_executor: None,
            state: MaterializeState::Uninitialized,
            materialized_data: None,
            memory_limit: memory_limit.unwrap_or(100 * 1024 * 1024), // Default size: 100 MB
            current_memory_usage: 0,
            consumed: false,
        }
    }

    /// Setting memory limits
    pub fn with_memory_limit(mut self, limit: usize) -> Self {
        self.memory_limit = limit;
        self
    }

    /// Obtaining the materialized state
    pub fn state(&self) -> &MaterializeState {
        &self.state
    }

    /// Check whether it has been materialized.
    pub fn is_materialized(&self) -> bool {
        matches!(self.state, MaterializeState::Materialized)
    }

    /// Obtain the materialized data (if it has already been materialized).
    pub fn get_materialized_data(&self) -> Option<&ExecutionResult> {
        self.materialized_data.as_ref()
    }

    /// Reset the consumption status to allow the re-reading of the materialized data.
    pub fn reset_consumed(&mut self) {
        self.consumed = false;
    }

    /// Materialized input data
    fn materialize_input(&mut self) -> DBResult<()> {
        if self.is_materialized() {
            return Ok(());
        }

        let input = self.input_executor.as_mut().ok_or_else(|| {
            DBError::query("Lack of inputs for materialized actuators".to_string())
        })?;

        let result = input.execute()?;

        // Estimating memory usage
        self.current_memory_usage = self.estimate_memory_usage(&result);

        if self.current_memory_usage > self.memory_limit {
            self.state = MaterializeState::Failed(format!(
                "物化数据大小({} bytes)超过内存限制({} bytes)",
                self.current_memory_usage, self.memory_limit
            ));
            return Err(DBError::query(
                "Physical data exceeds memory limits".to_string(),
            ));
        }

        self.materialized_data = Some(result);
        self.state = MaterializeState::Materialized;

        Ok(())
    }

    /// Estimate the memory usage of the execution results.
    fn estimate_memory_usage(&self, result: &ExecutionResult) -> usize {
        match result {
            ExecutionResult::Empty => 0,
            ExecutionResult::DataSet(dataset) => {
                dataset.rows.iter().map(std::mem::size_of_val).sum()
            }
            _ => 1024,
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for MaterializeExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // If the data has not yet been materialized, materialize it first.
        if !self.is_materialized() {
            self.materialize_input()?;
        }

        // Cloning of the materialized data
        self.materialized_data
            .clone()
            .ok_or_else(|| DBError::query("Physical data not available".to_string()))
    }

    fn open(&mut self) -> DBResult<()> {
        if let Some(ref mut input) = self.input_executor {
            input.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        if let Some(ref mut input) = self.input_executor {
            input.close()?;
        }
        // Clean up physicalized data to free up memory.
        self.materialized_data = None;
        self.state = MaterializeState::Uninitialized;
        self.current_memory_usage = 0;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.input_executor
            .as_ref()
            .map(|input| input.is_open())
            .unwrap_or(false)
    }

    fn id(&self) -> i64 {
        self.base.id()
    }

    fn name(&self) -> &str {
        "MaterializeExecutor"
    }

    fn description(&self) -> &str {
        "Materializes input data to memory for reuse"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for MaterializeExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_ref().map(|boxed| boxed.as_ref())
    }
}

#[cfg(test)]
mod tests {
    // Since the StorageClient is required, only compile-time checks are performed here.
    // The actual tests should be carried out during the integration testing.
}

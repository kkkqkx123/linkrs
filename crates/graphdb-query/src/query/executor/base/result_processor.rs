//! Result handler trait definition
//!
//! This module provides trait and type definitions related to the results processor.
//! Unified result processing abstraction layer used by all result processing executors.

use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::query::executor::base::ExecutionResult;
use crate::query::DataSet;
use parking_lot::RwLock;

/// Result Processor Context
///
/// Provides contextual information required for actuator operation
#[derive(Debug, Clone)]
pub struct ResultProcessorContext {
    /// Memory limit (bytes)
    pub memory_limit: Option<usize>,
    /// Whether to enable parallel processing
    pub enable_parallel: bool,
    /// parallelism
    pub parallel_degree: Option<usize>,
    /// Whether disk overflow is enabled
    pub enable_disk_spill: bool,
    /// Temporary directory path
    pub temp_dir: Option<String>,
}

impl Default for ResultProcessorContext {
    fn default() -> Self {
        Self {
            memory_limit: Some(100 * 1024 * 1024), // Default 100MB
            enable_parallel: true,
            parallel_degree: None, // Use the system default
            enable_disk_spill: false,
            temp_dir: None,
        }
    }
}

/// Results Processor Unified Interface
///
/// All result processing actuators should implement this interface
pub trait ResultProcessor {
    /// Processes input data and returns results
    fn process(&mut self, input: ExecutionResult) -> DBResult<ExecutionResult>;

    /// Setting Input Data
    fn set_input(&mut self, input: ExecutionResult);

    /// Get current input data
    fn get_input(&self) -> Option<&ExecutionResult>;

    /// Get processing context
    fn context(&self) -> &ResultProcessorContext;

    /// Setting the processing context
    fn set_context(&mut self, context: ResultProcessorContext);

    /// Getting Memory Usage
    fn memory_usage(&self) -> usize;

    /// Reset processor state
    fn reset(&mut self);

    /// Verify that the input data is valid
    fn validate_input(&self, input: &ExecutionResult) -> DBResult<()> {
        match input {
            ExecutionResult::DataSet(_) => Ok(()),
            ExecutionResult::Success => Ok(()),
            ExecutionResult::Empty => Ok(()),
            ExecutionResult::SpaceSwitched(_) => Ok(()),
            ExecutionResult::Error(_) => Ok(()),
        }
    }
}

/// Results processor base implementation
///
/// Provide generic result processor functionality, other actuators can inherit this base implementation
pub struct BaseResultProcessor<S> {
    /// Actuator ID
    pub id: i64,
    /// Actuator name
    pub name: String,
    /// Actuator Description
    pub description: String,
    /// Storage Engine References
    pub storage: Arc<RwLock<S>>,
    /// input data
    pub input: Option<ExecutionResult>,
    /// processing context
    pub context: ResultProcessorContext,
    /// Current memory usage
    pub memory_usage: usize,
    /// Implementation of statistical information
    pub stats: crate::query::executor::base::ExecutorStats,
}

impl<S> BaseResultProcessor<S> {
    /// Creating a new base results processor
    pub fn new(id: i64, name: String, description: String, storage: Arc<RwLock<S>>) -> Self {
        Self {
            id,
            name,
            description,
            storage,
            input: None,
            context: ResultProcessorContext::default(),
            memory_usage: 0,
            stats: crate::query::executor::base::ExecutorStats::new(),
        }
    }

    /// Getting implementation statistics
    pub fn get_stats(&self) -> &crate::query::executor::base::ExecutorStats {
        &self.stats
    }

    /// Get variable execution statistics
    pub fn get_stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        &mut self.stats
    }

    /// Setting Memory Limits
    pub fn with_memory_limit(mut self, limit: usize) -> Self {
        self.context.memory_limit = Some(limit);
        self
    }

    /// Enable parallel processing
    pub fn with_parallel(mut self, enable: bool) -> Self {
        self.context.enable_parallel = enable;
        self
    }

    /// Setting Parallelism
    pub fn with_parallel_degree(mut self, degree: usize) -> Self {
        self.context.parallel_degree = Some(degree);
        self
    }

    /// Enable Disk Overflow
    pub fn with_disk_spill(mut self, enable: bool) -> Self {
        self.context.enable_disk_spill = enable;
        self
    }

    /// Setting up a temporary directory
    pub fn with_temp_dir(mut self, dir: String) -> Self {
        self.context.temp_dir = Some(dir);
        self
    }

    /// Checking Memory Limits
    pub fn check_memory_limit(&self) -> DBResult<()> {
        if let Some(limit) = self.context.memory_limit {
            if self.memory_usage > limit {
                return Err(DBError::query(format!(
                    "Memory usage limit exceeded: {} > {}",
                    self.memory_usage, limit
                )));
            }
        }
        Ok(())
    }

    /// Update memory usage
    pub fn update_memory_usage(&mut self, delta: isize) {
        if delta >= 0 {
            self.memory_usage += delta as usize;
        } else if self.memory_usage >= (-delta) as usize {
            self.memory_usage -= (-delta) as usize;
        } else {
            self.memory_usage = 0;
        }
    }

    /// Estimating dataset memory usage
    pub fn estimate_dataset_memory_usage(dataset: &DataSet) -> usize {
        let mut usage = std::mem::size_of::<DataSet>();
        usage += dataset.col_names.len() * std::mem::size_of::<String>();

        for row in &dataset.rows {
            usage += std::mem::size_of::<Vec<crate::core::Value>>();
            for _value in row {
                usage += std::mem::size_of::<crate::core::Value>();
                // Here you can add a more precise estimate of the size of the value
            }
        }

        usage
    }

    /// Reset processor state
    pub fn reset_state(&mut self) {
        self.memory_usage = 0;
        self.input = None;
        self.stats = crate::query::executor::base::ExecutorStats::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::DataSet;

    #[test]
    fn test_result_processor_context_default() {
        let context = ResultProcessorContext::default();
        assert_eq!(context.memory_limit, Some(100 * 1024 * 1024));
        assert!(context.enable_parallel);
        assert!(context.parallel_degree.is_none());
        assert!(!context.enable_disk_spill);
        assert!(context.temp_dir.is_none());
    }

    #[test]
    fn test_estimate_dataset_memory_usage() {
        use crate::storage::MockStorage;

        let mut dataset = DataSet::new();
        dataset.col_names = vec!["col1".to_string(), "col2".to_string()];
        dataset.rows.push(vec![
            crate::core::Value::Int(1),
            crate::core::Value::String("test".to_string()),
        ]);

        // Testing Memory Usage Estimation
        let usage = BaseResultProcessor::<MockStorage>::estimate_dataset_memory_usage(&dataset);
        assert!(usage > 0);
    }
}

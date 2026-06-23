//! Recursive detector – Prevents the executor from making circular references.

use crate::core::error::{DBError, DBResult};
use std::collections::HashSet;

/// Recursive detector
#[derive(Debug, Clone)]
pub struct RecursionDetector {
    max_depth: usize,
    visited_stack: Vec<i64>,
    visited_set: HashSet<i64>,
    recursion_path: Vec<String>,
}

impl RecursionDetector {
    /// Create a new recursive detector.
    pub fn new(max_depth: usize) -> Self {
        Self {
            max_depth,
            visited_stack: Vec::new(),
            visited_set: HashSet::new(),
            recursion_path: Vec::new(),
        }
    }

    /// Verify whether the execution of the executor will lead to recursion.
    pub fn validate_executor(&mut self, executor_id: i64, executor_name: &str) -> DBResult<()> {
        // Check the depth of access.
        if self.visited_stack.len() >= self.max_depth {
            return Err(DBError::query(format!(
                "Executor call depth exceeds the maximum limit: Depth {}, Path {:?}",
                self.max_depth,
                self.get_recursion_path()
            )));
        }

        // Check for circular references.
        if self.visited_set.contains(&executor_id) {
            return Err(DBError::query(format!(
                "Detected an executor circular reference: {} (ID: {}) at path {:?}",
                executor_name,
                executor_id,
                self.get_recursion_path()
            )));
        }

        // Record visits
        self.visited_stack.push(executor_id);
        self.visited_set.insert(executor_id);
        self.recursion_path
            .push(format!("{}({})", executor_name, executor_id));

        Ok(())
    }

    /// Leave the current executor.
    pub fn leave_executor(&mut self) {
        if let Some(id) = self.visited_stack.pop() {
            self.visited_set.remove(&id);
        }
        self.recursion_path.pop();
    }

    /// Obtain the recursive path
    pub fn get_recursion_path(&self) -> Vec<String> {
        self.recursion_path.clone()
    }

    /// Reset the detector status.
    pub fn reset(&mut self) {
        self.visited_stack.clear();
        self.visited_set.clear();
        self.recursion_path.clear();
    }

    /// Get the current depth.
    pub fn current_depth(&self) -> usize {
        self.visited_stack.len()
    }

    /// Check whether the executor has been accessed.
    pub fn is_visited(&self, executor_id: i64) -> bool {
        self.visited_set.contains(&executor_id)
    }
}

/// Executor Verification Trait
pub trait ExecutorValidator {
    fn validate_no_recursion(&self, detector: &mut RecursionDetector) -> DBResult<()>;
}

/// Parallel computing configuration
///
/// Refer to `FLAGS_max_job_size` and `FLAGS_min_batch_size` from `nebula-graph`.
#[derive(Debug, Clone)]
pub struct ParallelConfig {
    /// Maximum number of parallel tasks (refer to nebula-graph’s FLAGS_max_job_size)
    pub max_job_size: usize,
    /// Minimum batch size (refer to FLAGS_min_batch_size in nebula-graph)
    pub min_batch_size: usize,
    /// Should parallel computing be enabled?
    pub enable_parallel: bool,
    /// Parallel computing threshold: Parallel processing is only used when the amount of data exceeds this value (refer to traverse_parallel_threshold_rows in nebula-graph).
    pub parallel_threshold: usize,
    /// The maximum amount of data that can be processed by a single-threaded system
    pub single_thread_limit: usize,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            max_job_size: 4,           // By default, 4 parallel tasks are executed.
            min_batch_size: 1000,      // Minimum batch size: 1000 lines
            enable_parallel: true,     // Parallel processing is enabled by default.
            parallel_threshold: 10000, // Parallel processing is only enabled when the amount of data exceeds 10,000 rows.
            single_thread_limit: 1000, // Use a single thread for texts with less than 1000 lines.
        }
    }
}

impl ParallelConfig {
    /// Calculating the batch size
    ///
    /// Refer to the implementation of Executor::getBatchSize in nebula-graph.
    /// batch size = max(min_batch_size, ceil(total_size / max_job_size))
    pub fn calculate_batch_size(&self, total_size: usize) -> usize {
        if total_size == 0 {
            return 0;
        }
        let batch_size_tmp = total_size.div_ceil(self.max_job_size);
        batch_size_tmp.max(self.min_batch_size)
    }

    /// Determine whether to use parallel computing.
    pub fn should_use_parallel(&self, total_size: usize) -> bool {
        self.enable_parallel && total_size >= self.parallel_threshold
    }

    /// Create a configuration suitable for small amounts of data.
    pub fn for_small_data() -> Self {
        Self {
            max_job_size: 2,
            min_batch_size: 100,
            enable_parallel: true,
            parallel_threshold: 500,
            single_thread_limit: 100,
        }
    }

    /// Create a configuration suitable for large amounts of data.
    pub fn for_large_data() -> Self {
        Self {
            max_job_size: 8,
            min_batch_size: 1000,
            enable_parallel: true,
            parallel_threshold: 50000,
            single_thread_limit: 1000,
        }
    }
}

/// Actuator safety configuration
#[derive(Debug, Clone)]
pub struct ExecutorSafetyConfig {
    pub max_recursion_depth: usize,
    pub max_loop_iterations: usize,
    pub max_expand_depth: usize,
    pub enable_recursion_detection: bool,
    /// Parallel Computing Configuration
    pub parallel_config: ParallelConfig,
}

impl Default for ExecutorSafetyConfig {
    fn default() -> Self {
        Self {
            max_recursion_depth: 1000,
            max_loop_iterations: 10000,
            max_expand_depth: 100,
            enable_recursion_detection: true,
            parallel_config: ParallelConfig::default(),
        }
    }
}

/// Actuator Safety Validator
#[derive(Debug)]
pub struct ExecutorSafetyValidator {
    config: ExecutorSafetyConfig,
    recursion_detector: RecursionDetector,
}

impl ExecutorSafetyValidator {
    /// Create a new security verifier
    pub fn new(config: ExecutorSafetyConfig) -> Self {
        Self {
            recursion_detector: RecursionDetector::new(config.max_recursion_depth),
            config,
        }
    }

    /// Verify the security of the executor chain.
    pub fn validate_executor_chain(
        &mut self,
        executor_id: i64,
        executor_name: &str,
    ) -> DBResult<()> {
        if self.config.enable_recursion_detection {
            self.recursion_detector
                .validate_executor(executor_id, executor_name)?;
        }
        Ok(())
    }

    /// Verify the configuration of the loop.
    pub fn validate_loop_config(&self, max_iterations: Option<usize>) -> DBResult<()> {
        if let Some(iterations) = max_iterations {
            if iterations > self.config.max_loop_iterations {
                return Err(DBError::query(format!(
                    "The maximum number of loop iterations {} exceeds the limit of {}.",
                    iterations, self.config.max_loop_iterations
                )));
            }
        }
        Ok(())
    }

    /// Verify the extended configuration.
    pub fn validate_expand_config(&self, max_depth: Option<usize>) -> DBResult<()> {
        if let Some(depth) = max_depth {
            if depth > self.config.max_expand_depth {
                return Err(DBError::query(format!(
                    "The maximum depth extension {} exceeds the limit of {}.",
                    depth, self.config.max_expand_depth
                )));
            }
        }
        Ok(())
    }

    /// Reset the verifier status.
    pub fn reset(&mut self) {
        self.recursion_detector.reset();
    }

    /// Get the current recursion depth
    pub fn current_depth(&self) -> usize {
        self.recursion_detector.current_depth()
    }
}

impl Default for ExecutorSafetyValidator {
    fn default() -> Self {
        Self::new(ExecutorSafetyConfig::default())
    }
}

/// Plan node validator for executor factory
///
/// Validates plan nodes before executor creation to ensure safety constraints.
pub struct PlanValidator;

impl PlanValidator {
    /// Create a new plan validator.
    pub fn new() -> Self {
        Self
    }

    /// Validate plan node safety constraints.
    ///
    /// Checks:
    /// - Expand node step limits
    /// - Loop node restrictions (must be manually constructed)
    pub fn validate(
        &self,
        plan_node: &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) -> crate::core::error::DBResult<()> {
        use crate::core::error::query::QueryError;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        match plan_node {
            PlanNodeEnum::Expand(node) => {
                let step_limit = node
                    .step_limit()
                    .and_then(|s| usize::try_from(s).ok())
                    .unwrap_or(10);
                if step_limit > 1000 {
                    return Err(QueryError::execution(format!(
                        "Expand executor step limit {} exceeds safety threshold 1000",
                        step_limit
                    ))
                    .into());
                }
            }
            PlanNodeEnum::Loop(_) => {
                return Err(QueryError::execution(
                    "Loop executor needs to be built manually, automatic creation through factory is not supported".to_string(),
                ).into());
            }
            _ => {}
        }
        Ok(())
    }
}

impl Default for PlanValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recursion_detection() {
        let mut detector = RecursionDetector::new(10);

        // Under normal circumstances
        assert!(detector.validate_executor(1, "TestExecutor").is_ok());
        assert!(detector.validate_executor(2, "AnotherExecutor").is_ok());

        // Circular Reference Detection
        assert!(detector.validate_executor(1, "TestExecutor").is_err());
    }

    #[test]
    fn test_max_depth_protection() {
        let mut detector = RecursionDetector::new(3);

        // Normal depth
        assert!(detector.validate_executor(1, "E1").is_ok());
        assert!(detector.validate_executor(2, "E2").is_ok());
        assert!(detector.validate_executor(3, "E3").is_ok());

        // Exceeding the maximum depth
        assert!(detector.validate_executor(4, "E4").is_err());
    }

    #[test]
    fn test_leave_executor() {
        let mut detector = RecursionDetector::new(10);

        // Enter the actuator.
        assert!(detector.validate_executor(1, "E1").is_ok());
        assert_eq!(detector.current_depth(), 1);

        // Leave the actuator.
        detector.leave_executor();
        assert_eq!(detector.current_depth(), 0);

        // It is possible to re-enter.
        assert!(detector.validate_executor(1, "E1").is_ok());
    }

    #[test]
    fn test_reset_detector() {
        let mut detector = RecursionDetector::new(3);

        // Access to multiple actuators
        assert!(detector.validate_executor(1, "E1").is_ok());
        assert!(detector.validate_executor(2, "E2").is_ok());

        // Reset
        detector.reset();

        // It should be possible to re-enter now.
        assert!(detector.validate_executor(1, "E1").is_ok());
        assert!(detector.validate_executor(2, "E2").is_ok());
    }

    #[test]
    fn test_safety_validator_loop_config() {
        let validator = ExecutorSafetyValidator::default();

        // Normal configuration
        assert!(validator.validate_loop_config(Some(100)).is_ok());
        assert!(validator.validate_loop_config(None).is_ok());

        // Exceeding the limit
        assert!(validator.validate_loop_config(Some(20000)).is_err());
    }

    #[test]
    fn test_safety_validator_expand_config() {
        let validator = ExecutorSafetyValidator::default();

        // Normal configuration
        assert!(validator.validate_expand_config(Some(50)).is_ok());
        assert!(validator.validate_expand_config(None).is_ok());

        // Exceeding the limits
        assert!(validator.validate_expand_config(Some(200)).is_err());
    }

    #[test]
    fn test_recursion_path_tracking() {
        let mut detector = RecursionDetector::new(10);

        detector
            .validate_executor(1, "E1")
            .expect("validate_executor should succeed");
        detector
            .validate_executor(2, "E2")
            .expect("validate_executor should succeed");
        detector
            .validate_executor(3, "E3")
            .expect("validate_executor should succeed");

        let path = detector.get_recursion_path();
        assert_eq!(path, vec!["E1(1)", "E2(2)", "E3(3)"]);
    }
}

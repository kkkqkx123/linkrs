//! Helper functions for transformation executors

use crate::core::error::{DBError, DBResult};
use crate::query::executor::base::ExecutionResult;

/// Helper function to get input variable from context with standard error handling
pub fn get_input_result(
    context: &crate::query::executor::base::ExecutionContext,
    var_name: &str,
) -> DBResult<ExecutionResult> {
    context
        .get_result(var_name)
        .ok_or_else(|| DBError::query(format!("Input variable '{}' not found", var_name)))
}

/// Helper function to convert ExecutionResult to Vec<Value>
pub fn execution_result_to_values(
    result: &ExecutionResult,
) -> Result<Vec<crate::core::Value>, DBError> {
    match result {
        ExecutionResult::DataSet(dataset) => {
            let values: Vec<crate::core::Value> = dataset
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

/// Helper function to check if input variable exists
pub fn require_input_result(
    context: &crate::query::executor::base::ExecutionContext,
    var_name: &str,
) -> DBResult<()> {
    if context.get_result(var_name).is_none() {
        Err(DBError::query(format!(
            "Input variable '{}' not found",
            var_name
        )))
    } else {
        Ok(())
    }
}

//! Type of execution result
//!
//! Defines the data structure of the actuator's execution result, supporting multiple result types.

use crate::core::error::DBError;
use crate::core::types::{SpaceStatus, SpaceSummary};
use crate::query::data_set::DataSet;

/// Type of execution result
///
/// Uniformly represents the execution results of all actuators and supports multiple data formats.
#[derive(Debug, Clone)]
pub enum ExecutionResult {
    /// Successful execution returns structured dataset (primary result type)
    DataSet {
        data: DataSet,
        /// Human-readable reason for the selected execution mode and partition
        /// layout.  Populated by the optimizer and surfaced for diagnostics.
        execution_mode_reason: Option<String>,
    },
    /// Successful execution, no data returned
    Empty,
    /// Successful execution, no data returned (alias)
    Success,
    /// Space switched successfully, contains the new space info
    SpaceSwitched(SpaceSummary),
    /// implementation error
    Error(String),
}

impl ExecutionResult {
    /// Get the count of elements in the result
    pub fn count(&self) -> usize {
        match self {
            ExecutionResult::DataSet { data, .. } => data.row_count(),
            ExecutionResult::Success => 0,
            ExecutionResult::Empty => 0,
            ExecutionResult::SpaceSwitched(_) => 0,
            ExecutionResult::Error(_) => 0,
        }
    }

    /// Creating an ExecutionResult from a DataSet
    pub fn from_data_set(data: DataSet) -> Self {
        ExecutionResult::DataSet {
            data,
            execution_mode_reason: None,
        }
    }

    /// Convert to DataSet
    pub fn to_data_set(&self) -> Option<&DataSet> {
        match self {
            ExecutionResult::DataSet { data, .. } => Some(data),
            _ => None,
        }
    }

    /// Execution mode reason attached by the optimizer, if any.
    pub fn execution_mode_reason(&self) -> Option<&str> {
        match self {
            ExecutionResult::DataSet {
                execution_mode_reason,
                ..
            } => execution_mode_reason.as_deref(),
            _ => None,
        }
    }

    /// Check if this is a space switched result
    pub fn is_space_switched(&self) -> bool {
        matches!(self, ExecutionResult::SpaceSwitched(_))
    }

    /// Get the space summary if this is a SpaceSwitched result
    pub fn space_summary(&self) -> Option<&SpaceSummary> {
        match self {
            ExecutionResult::SpaceSwitched(summary) => Some(summary),
            _ => None,
        }
    }
}

/// Result type alias
pub type DBResult<T> = Result<T, DBError>;

/// Support for traits that are converted to execution results
pub trait IntoExecutionResult {
    fn into_execution_result(self) -> ExecutionResult;
}

impl IntoExecutionResult for DataSet {
    fn into_execution_result(self) -> ExecutionResult {
        ExecutionResult::DataSet {
            data: self,
            execution_mode_reason: None,
        }
    }
}

impl IntoExecutionResult for () {
    fn into_execution_result(self) -> ExecutionResult {
        ExecutionResult::Success
    }
}

/// Check if a space is writable for write operations
pub fn check_space_writable(space: &SpaceSummary) -> Result<(), ExecutionResult> {
    match space.status {
        SpaceStatus::Online => Ok(()),
        SpaceStatus::Maintenance => Err(ExecutionResult::Error(format!(
            "Space '{}' is in maintenance mode. Write operations are not allowed.",
            space.name
        ))),
        SpaceStatus::ReadOnly => Err(ExecutionResult::Error(format!(
            "Space '{}' is in read-only mode. Write operations are not allowed.",
            space.name
        ))),
        SpaceStatus::Offline => Err(ExecutionResult::Error(format!(
            "Space '{}' is offline. Operations are not allowed.",
            space.name
        ))),
    }
}

/// Check if a space is accessible for read operations
pub fn check_space_accessible(space: &SpaceSummary) -> Result<(), ExecutionResult> {
    if space.is_accessible() {
        Ok(())
    } else {
        Err(ExecutionResult::Error(format!(
            "Space '{}' is offline. Operations are not allowed.",
            space.name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::DataType;

    fn create_test_summary_with_status(status: SpaceStatus) -> SpaceSummary {
        SpaceSummary {
            id: 1,
            name: "test_space".to_string(),
            vid_type: DataType::String,
            status,
        }
    }

    #[test]
    fn test_check_space_writable_online() {
        let summary = create_test_summary_with_status(SpaceStatus::Online);
        assert!(check_space_writable(&summary).is_ok());
    }

    #[test]
    fn test_check_space_writable_maintenance() {
        let summary = create_test_summary_with_status(SpaceStatus::Maintenance);
        let result = check_space_writable(&summary);
        assert!(result.is_err());
        if let Err(ExecutionResult::Error(msg)) = result {
            assert!(msg.contains("maintenance mode"));
        }
    }

    #[test]
    fn test_check_space_writable_readonly() {
        let summary = create_test_summary_with_status(SpaceStatus::ReadOnly);
        let result = check_space_writable(&summary);
        assert!(result.is_err());
        if let Err(ExecutionResult::Error(msg)) = result {
            assert!(msg.contains("read-only mode"));
        }
    }

    #[test]
    fn test_check_space_writable_offline() {
        let summary = create_test_summary_with_status(SpaceStatus::Offline);
        let result = check_space_writable(&summary);
        assert!(result.is_err());
        if let Err(ExecutionResult::Error(msg)) = result {
            assert!(msg.contains("offline"));
        }
    }

    #[test]
    fn test_check_space_accessible() {
        let online = create_test_summary_with_status(SpaceStatus::Online);
        let maintenance = create_test_summary_with_status(SpaceStatus::Maintenance);
        let readonly = create_test_summary_with_status(SpaceStatus::ReadOnly);
        let offline = create_test_summary_with_status(SpaceStatus::Offline);

        assert!(check_space_accessible(&online).is_ok());
        assert!(check_space_accessible(&maintenance).is_ok());
        assert!(check_space_accessible(&readonly).is_ok());
        assert!(check_space_accessible(&offline).is_err());
    }
}

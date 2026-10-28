//! Query execution state machines (executor-internal only).
//!
//! Ownership: execution cancellation is sourced solely from the executor
//! [`QueryRegistry`](crate::executor::streaming::query_registry::QueryRegistry)
//! token, display status from [`QueryManager`](crate::query_manager::QueryManager).
//! The state machines below never leave the executor: do not base display or
//! cancellation decisions on them, and do not introduce new state types for
//! cross-layer signalling.

use std::fmt;

/// Query execution status - Top-level execution process status
///
/// Indicates the lifecycle status of the entire query execution, which is used for query process management.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum QueryExecutionState {
    /// The query has been created and is waiting to be executed.
    #[default]
    Pending,
    /// The query is currently being executed.
    Running,
    /// The query execution has been completed.
    Completed,
    /// The query execution failed.
    Failed,
    /// The query has been cancelled.
    Cancelled,
    /// Query execution timed out.
    Timeout,
}

impl QueryExecutionState {
    /// Check whether the status is terminal.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            QueryExecutionState::Completed
                | QueryExecutionState::Failed
                | QueryExecutionState::Cancelled
                | QueryExecutionState::Timeout
        )
    }

    /// Check whether the status allows cancellation.
    pub fn can_cancel(&self) -> bool {
        matches!(
            self,
            QueryExecutionState::Pending | QueryExecutionState::Running
        )
    }

    /// Get the English description of the status.
    pub fn description(&self) -> &'static str {
        match self {
            QueryExecutionState::Pending => "Pending",
            QueryExecutionState::Running => "Running",
            QueryExecutionState::Completed => "Completed",
            QueryExecutionState::Failed => "Failed",
            QueryExecutionState::Cancelled => "Cancelled",
            QueryExecutionState::Timeout => "Timeout",
        }
    }
}

impl fmt::Display for QueryExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// Executor State - The operating status of a single executor
///
/// Indicates the execution status of a single executor instance, which is used for the management of the executor's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExecutorState {
    /// The executor has been created, but the execution has not yet begun.
    #[default]
    Initialized,
    /// The executor is currently in operation (i.e., it is performing its intended function).
    Executing,
    /// The executor has completed its execution.
    Completed,
    /// The execution of the executor failed.
    Failed,
    /// The executor has been cancelled.
    Cancelled,
    /// The executor has been paused (for breakpoint debugging purposes).
    Paused,
}

impl ExecutorState {
    /// Check whether the current status allows the transition to the target status.
    pub fn can_transition_to(&self, target: ExecutorState) -> bool {
        match (self, target) {
            // The initial state can be changed to either "Executing", "Failed", or "Cancelled".
            (ExecutorState::Initialized, ExecutorState::Executing) => true,
            (ExecutorState::Initialized, ExecutorState::Failed) => true,
            (ExecutorState::Initialized, ExecutorState::Cancelled) => true,
            // During the process, the status can be changed to Complete, Failed, Cancel, or Pause.
            (ExecutorState::Executing, ExecutorState::Completed) => true,
            (ExecutorState::Executing, ExecutorState::Failed) => true,
            (ExecutorState::Executing, ExecutorState::Cancelled) => true,
            (ExecutorState::Executing, ExecutorState::Paused) => true,
            // A pause can be used to resume the execution of a process, or to cancel or terminate it if it has failed.
            (ExecutorState::Paused, ExecutorState::Executing) => true,
            (ExecutorState::Paused, ExecutorState::Failed) => true,
            (ExecutorState::Paused, ExecutorState::Cancelled) => true,
            // The final state cannot be changed again.
            (ExecutorState::Completed, _) => false,
            (ExecutorState::Failed, _) => false,
            (ExecutorState::Cancelled, _) => false,
            _ => false,
        }
    }

    /// Get the English description of the status.
    pub fn description(&self) -> &'static str {
        match self {
            ExecutorState::Initialized => "Initialized",
            ExecutorState::Executing => "Executing",
            ExecutorState::Completed => "Completed",
            ExecutorState::Failed => "Failed",
            ExecutorState::Cancelled => "Cancelled",
            ExecutorState::Paused => "Paused",
        }
    }
}

impl fmt::Display for ExecutorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// Loop execution state – A status specifically used for loop control
///
/// Specifically designed for state management of the LoopExecutor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum LoopExecutionState {
    /// The loop has not started yet.
    #[default]
    NotStarted,
    /// In the process of cyclic execution…
    Running { iteration: usize },
    /// The loop has ended normally.
    Finished,
    /// The loop was terminated due to an error.
    Error(String),
    /// The loop terminates because the maximum number of iterations has been reached.
    MaxIterationsReached { max: usize },
}

impl LoopExecutionState {
    /// Get the current iteration count.
    pub fn iteration(&self) -> Option<usize> {
        match self {
            LoopExecutionState::Running { iteration } => Some(*iteration),
            _ => None,
        }
    }

    /// Check whether the loop has ended.
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            LoopExecutionState::Finished
                | LoopExecutionState::Error(_)
                | LoopExecutionState::MaxIterationsReached { .. }
        )
    }

    /// Get the English description of the status.
    pub fn description(&self) -> String {
        match self {
            LoopExecutionState::NotStarted => "Not Started".to_string(),
            LoopExecutionState::Running { iteration } => {
                format!("Running (iteration {})", iteration)
            }
            LoopExecutionState::Finished => "Finished".to_string(),
            LoopExecutionState::Error(msg) => format!("Error: {}", msg),
            LoopExecutionState::MaxIterationsReached { max } => {
                format!("Max iterations reached ({})", max)
            }
        }
    }
}

impl fmt::Display for LoopExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// Result Line Status – Status of single-line data processing
///
/// Indicates the status of the processing result for a single data record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RowStatus {
    /// Normal data
    #[default]
    Valid,
    /// The data has been filtered out.
    Filtered,
    /// The data is invalid.
    Invalid,
    /// The tags have been filtered out.
    TagFiltered,
}

impl RowStatus {
    /// Check whether the data is valid.
    pub fn is_valid(&self) -> bool {
        matches!(self, RowStatus::Valid)
    }

    /// Convert to an integer representation.
    pub fn to_i32(&self) -> i32 {
        match self {
            RowStatus::Valid => 0,
            RowStatus::Invalid => -1,
            RowStatus::Filtered => -2,
            RowStatus::TagFiltered => -3,
        }
    }
}

impl fmt::Display for RowStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowStatus::Valid => write!(f, "Effective"),
            RowStatus::Filtered => write!(f, "Filtered"),
            RowStatus::Invalid => write!(f, "Invalid"),
            RowStatus::TagFiltered => write!(f, "Tag filtering"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_execution_state_transitions() {
        assert!(!QueryExecutionState::Running.is_terminal());
        assert!(QueryExecutionState::Completed.is_terminal());
        assert!(QueryExecutionState::Pending.can_cancel());
        assert!(!QueryExecutionState::Completed.can_cancel());
    }

    #[test]
    fn test_executor_state_transitions() {
        assert!(ExecutorState::Initialized.can_transition_to(ExecutorState::Executing));
        assert!(!ExecutorState::Completed.can_transition_to(ExecutorState::Executing));
        assert!(ExecutorState::Executing.can_transition_to(ExecutorState::Paused));
        assert!(ExecutorState::Paused.can_transition_to(ExecutorState::Executing));
    }

    #[test]
    fn test_loop_execution_state() {
        let state = LoopExecutionState::Running { iteration: 5 };
        assert_eq!(state.iteration(), Some(5));
        assert!(!state.is_finished());

        let finished = LoopExecutionState::Finished;
        assert!(finished.is_finished());
    }

    #[test]
    fn test_row_status_conversion() {
        assert_eq!(RowStatus::Valid.to_i32(), 0);
        assert_eq!(RowStatus::Invalid.to_i32(), -1);
        assert_eq!(RowStatus::Filtered.to_i32(), -2);
        assert_eq!(RowStatus::TagFiltered.to_i32(), -3);
    }
}

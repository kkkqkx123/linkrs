//! Optimizer error type
//!
//! Define the error types related to query optimizers, including:
//! The cost calculation was incorrect.
//! Error in plan optimization
//! The statistical information is incorrect.
//! The rules were applied incorrectly.

use thiserror::Error;

/// Optimizer error type
#[derive(Error, Debug, Clone)]
pub enum OptimizeError {
    /// Node type not supported
    #[error("Unsupported node type: {0}")]
    UnsupportedNodeType(String),

    /// Statistical information is missing.
    #[error("Missing statistics: {0}")]
    MissingStatistics(String),

    /// Miscalculation of the costs
    #[error("Calculation error: {0}")]
    CalculationError(String),

    /// The plan to optimize the errors has failed.
    #[error("Plan optimization error: {0}")]
    PlanOptimizationError(String),

    /// The rule was applied incorrectly.
    #[error("Rule application error: {0}")]
    RuleApplicationError(String),

    /// There is an error in the statistical information.
    #[error("Statistics error: {0}")]
    StatisticsError(String),

    /// The index selection was incorrect.
    #[error("Index selection error: {0}")]
    IndexSelectionError(String),

    /// Error in the optimization of the connection sequence.
    #[error("Join order optimization error: {0}")]
    JoinOrderError(String),

    /// Expression conversion error.
    #[error("Expression transform error: {0}")]
    ExpressionTransformError(String),

    /// Internal optimization error
    #[error("Internal optimization error: {0}")]
    InternalError(String),

    /// Heuristic optimization failed
    #[error("Heuristic optimization failed: {0}")]
    HeuristicFailed(String),

    /// Cost-based optimization failed
    #[error("Cost-based optimization failed: {0}")]
    CostBasedFailed(String),

    /// Pipeline configuration error
    #[error("Pipeline configuration error: {0}")]
    ConfigurationError(String),
}

/// Optimizer result type
pub type OptimizeResult<T> = Result<T, OptimizeError>;

/// Cost calculation related errors
#[derive(Error, Debug, Clone)]
pub enum CostError {
    /// Unsupported node type
    #[error("Unsupported node type: {0}")]
    UnsupportedNodeType(String),

    /// Missing statistics
    #[error("Missing statistics: {0}")]
    MissingStatistics(String),

    /// Calculation error
    #[error("Calculation error: {0}")]
    CalculationError(String),
}

/// Type of result for the cost calculation
pub type CostResult<T> = Result<T, CostError>;

impl From<CostError> for OptimizeError {
    fn from(err: CostError) -> Self {
        match err {
            CostError::UnsupportedNodeType(msg) => OptimizeError::UnsupportedNodeType(msg),
            CostError::MissingStatistics(msg) => OptimizeError::MissingStatistics(msg),
            CostError::CalculationError(msg) => OptimizeError::CalculationError(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimize_error_display() {
        let err = OptimizeError::UnsupportedNodeType("UnknownNode".to_string());
        assert!(err.to_string().contains("Unsupported node type"));

        let err = OptimizeError::MissingStatistics("vertex_count".to_string());
        assert!(err.to_string().contains("Missing statistics"));
    }

    #[test]
    fn test_cost_error_conversion() {
        let cost_err = CostError::CalculationError("division by zero".to_string());
        let opt_err: OptimizeError = cost_err.into();
        assert!(matches!(opt_err, OptimizeError::CalculationError(_)));
    }
}

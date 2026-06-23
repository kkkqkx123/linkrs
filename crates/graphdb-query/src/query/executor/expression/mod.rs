pub mod error;
pub mod evaluation_context;
pub mod evaluator;
pub mod functions;

// Re-export the operator types from the core.
pub use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};

// Export the type tool from the core again.
pub use crate::core::TypeUtils;

// Re-export error types from local error module
pub use error::{ExpressionError, ExpressionErrorType, ExpressionPosition};

// Re-export the ExpressionContext trait and the evaluator from the evaluator module.
pub use evaluator::{ExpressionContext, ExpressionEvaluator};

// Re-export the context type from the evaluation_context module.
pub use evaluation_context::{DefaultExpressionContext, RowExpressionContext};

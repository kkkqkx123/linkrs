//! Expression Evaluation Context Module
//!
//! Provide context management during the evaluation of expressions, including functions, error handling, and other features.
//!
//! Note: This module provides an implementation of the runtime evaluation context.
//! For context analysis during compilation, please refer to `ExpressionAnalysisContext`.

pub mod default_context;
pub mod row_context;

// Re-export the default context type.
pub use default_context::DefaultExpressionContext;

// Re-export the ExpressionContext trait (from evaluator::traits)
pub use crate::query::executor::expression::evaluator::traits::ExpressionContext;

// Rederive the context type of the row.
pub use row_context::RowExpressionContext;

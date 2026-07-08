//! Validator Context Module
//!
//! Provide the contextual information required for the verification phase.

pub mod expression_context;

// Re-export from core - the actual implementation is in `crate::core::types::expr::expression_context`
pub use expression_context::{ExpressionAnalysisContext, OptimizationFlags};

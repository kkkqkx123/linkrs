//! Re-export ExpressionAnalysisContext from core
//!
//! This file exists to maintain backward compatibility with existing imports.
//! The actual implementation is now in `crate::core::types::expr::expression_context`.

pub use crate::core::types::expr::expression_context::{
    ExpressionAnalysisContext, OptimizationFlags,
};

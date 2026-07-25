//! Verification Results Information Module
//!
//! **DEPRECATED**: Types have been moved to `crate::query::binder::validation`.
//! This module retains the `ValidationInfo` extension methods used by the
//! legacy validator code paths.

// Re-export types from the canonical location.
pub use crate::query::binder::validation::{
    AggregateCallInfo, ClauseKind, HintSeverity, IndexHint, OptimizationHint,
    PathAnalysis, SemanticInfo, ValidatedStatement, ValidationInfo,
};

use std::collections::HashMap;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::semantic::{AliasType, ValueType};

impl ValidationInfo {
    /// Determine the type of the expression.
    pub fn get_expr_type(&self, expr: &ContextualExpression) -> Option<ValueType> {
        expr.data_type()
            .map(|data_type| ValueType::from_data_type(&data_type))
    }

    /// Analyze expressions using ExpressionAnalyzer.
    pub fn analyze_expression(
        &mut self,
        expr: &ContextualExpression,
        variable_types: Option<&HashMap<String, crate::core::DataType>>,
    ) -> Result<
        crate::query::validator::ExpressionAnalysisResult,
        crate::query::validator::error::ValidationError,
    > {
        use crate::query::validator::ExpressionAnalyzer;

        let analyzer = ExpressionAnalyzer::new();
        let result = analyzer.analyze(expr, variable_types)?;

        Ok(result)
    }
}

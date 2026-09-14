//! Shared projection conversion helpers.
//!
//! Single implementation for `RETURN / WITH / YIELD` item to `YieldColumn`
//! conversion and output alias validation. All planners must use this module
//! instead of private copies so the default alias rule stays consistent.

use crate::parser::ast::stmt::{ReturnItem, YieldItem};
use crate::planning::planner::PlannerError;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::expression_utils::generate_default_alias_from_contextual;
use graphdb_core::YieldColumn;

/// Default alias for a projection expression.
///
/// Single definition point; forwards to the core utility.
pub fn default_projection_alias(expression: &ContextualExpression) -> String {
    generate_default_alias_from_contextual(expression)
}

/// Convert one `ReturnItem` into a `YieldColumn`.
pub fn return_item_to_yield_column(item: &ReturnItem) -> YieldColumn {
    let (expression, alias) = match item {
        ReturnItem::Expression { expression, alias } => (expression.clone(), alias.clone()),
    };
    let alias = alias.unwrap_or_else(|| default_projection_alias(&expression));
    YieldColumn { expression, alias }
}

/// Convert one `YieldItem` into a `YieldColumn`.
pub fn yield_item_to_yield_column(item: &YieldItem) -> YieldColumn {
    let alias = item
        .alias
        .clone()
        .unwrap_or_else(|| default_projection_alias(&item.expression));
    YieldColumn {
        expression: item.expression.clone(),
        alias,
    }
}

/// Convert a `YieldItem` list into `YieldColumn`s.
pub fn yield_items_to_columns(items: &[YieldItem]) -> Result<Vec<YieldColumn>, PlannerError> {
    if items.is_empty() {
        return Err(PlannerError::PlanGenerationFailed(
            "YIELD clause missing yield item".to_string(),
        ));
    }
    Ok(items.iter().map(yield_item_to_yield_column).collect())
}

/// Output column types for a projection, derived from the yield columns.
///
/// Single definition point so every `LogicalProjectNode` carries the same
/// type information; without it the executor resolves `SlotLayout` types to
/// `None` and column-oriented consumers lose the typed path.
pub fn project_column_types(columns: &[YieldColumn]) -> Vec<graphdb_core::DataType> {
    columns
        .iter()
        .map(|col| {
            col.expression
                .data_type()
                .unwrap_or(graphdb_core::DataType::Unknown)
        })
        .collect()
}

/// Placeholder types for nodes whose output width is known but whose element
/// types cannot be resolved at plan time (system / management nodes).
///
/// Keeps `column_types.len() == col_names.len()` so downstream width estimates
/// and slot layouts never observe a truncated vector.
pub fn unknown_column_types(width: usize) -> Vec<graphdb_core::DataType> {
    vec![graphdb_core::DataType::Unknown; width]
}

/// Validate output aliases: non-empty and unique.
pub fn validate_output_aliases(aliases: &[String]) -> Result<(), PlannerError> {
    let mut seen = std::collections::HashSet::new();
    for (index, alias) in aliases.iter().enumerate() {
        if alias.is_empty() {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "output alias at index {index} has an empty alias"
            )));
        }
        if !seen.insert(alias.clone()) {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "duplicate output alias: {alias}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::Expression;
    use std::sync::Arc;

    fn ctx_expr(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn test_default_alias_variable() {
        let e = ctx_expr(Expression::Variable("n".to_string()));
        assert_eq!(default_projection_alias(&e), "n");
    }

    #[test]
    fn test_return_item_uses_alias_or_default() {
        let e = ctx_expr(Expression::Variable("n".to_string()));
        let item = ReturnItem::Expression {
            expression: e.clone(),
            alias: None,
        };
        assert_eq!(return_item_to_yield_column(&item).alias, "n");
        let item2 = ReturnItem::Expression {
            expression: e,
            alias: Some("x".to_string()),
        };
        assert_eq!(return_item_to_yield_column(&item2).alias, "x");
    }

    #[test]
    fn test_yield_items_empty_fails() {
        assert!(yield_items_to_columns(&[]).is_err());
    }

    #[test]
    fn test_validate_aliases_rejects_empty_and_duplicate() {
        assert!(validate_output_aliases(&["a".to_string(), String::new()]).is_err());
        assert!(validate_output_aliases(&["a".to_string(), "a".to_string()]).is_err());
        assert!(validate_output_aliases(&["a".to_string(), "b".to_string()]).is_ok());
    }
}

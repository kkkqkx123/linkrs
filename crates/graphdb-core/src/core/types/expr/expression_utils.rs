//! Expression Basic Utility Functions
//!
//! Provides basic utility functions for expression processing, including:
//! - String extraction from expressions
//! - Default alias generation
//! - Property reference extraction

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor_collectors::PropertyCollector;
use crate::core::types::expr::ExpressionVisitor;
use crate::core::Value;

/// Extracting String Values from Expressions
///
/// This method is used to extract a string value from a ContextualExpression.
/// Supports extraction from variables, literals (strings, integers, floats, booleans).
///
/// # Parameters
/// - `expr`: Contextual expression to be extracted from the string
///
/// # Returns
/// - `Ok(String)`: Extracted string value
/// - `Err(String)`: Error message when unable to extract string
pub fn extract_string_from_expr(expr: &ContextualExpression) -> Result<String, String> {
    if let Some(var_name) = expr.as_variable() {
        return Ok(var_name);
    }

    if let Some(literal) = expr.as_literal() {
        match literal {
            Value::String(s) => return Ok(s.clone()),
            Value::Int(i) => return Ok(i.to_string()),
            Value::BigInt(i) => return Ok(i.to_string()),
            Value::Float(f) => return Ok(f.to_string()),
            Value::Bool(b) => return Ok(b.to_string()),
            _ => return Err(format!("Cannot extract string from literal: {:?}", literal)),
        }
    }

    Err(format!(
        "Cannot extract string from expression: {}",
        expr.to_expression_string()
    ))
}

/// Generating default aliases from ContextualExpression
///
/// This method is used to generate a default alias for an expression.
/// Priority: Variable Name > Function Name > Property Name > Arithmetic Expression > Expression String
///
/// # Parameters
/// - `expression`: Contextual expression to generate an alias for
///
/// # Returns
/// Default alias generated
pub fn generate_default_alias_from_contextual(expression: &ContextualExpression) -> String {
    if let Some(var_name) = expression.as_variable() {
        return var_name;
    }

    if let Some(func_name) = expression.as_function_name() {
        return func_name.to_lowercase();
    }

    if expression.is_aggregate() {
        return "agg".to_string();
    }

    if expression.is_property() {
        return expression.to_expression_string();
    }

    if expression.is_binary() {
        return "expr".to_string();
    }

    expression.to_expression_string()
}

/// Extracting property references in context expressions
///
/// # Parameters
/// - `ctx_expr`: context expression
///
/// # Returns
/// Names of all attributes referenced in the expression
pub fn extract_property_refs(ctx_expr: &ContextualExpression) -> Vec<String> {
    let expr_meta = match ctx_expr.expression() {
        Some(e) => e,
        None => return Vec::new(),
    };
    let expr = expr_meta.inner();
    let mut collector = PropertyCollector::new();
    ExpressionVisitor::visit(&mut collector, expr);
    collector.properties
}

/// Extract grouping information from YieldColumns
///
/// Extracts grouping keys and aggregates from the YieldColumn list.
/// Expressions that contain aggregation functions are used as grouping items,
/// and expressions that do not are used as grouping keys.
///
/// # Parameters
/// - `yield_columns`: list of YieldColumns
///
/// # Returns
/// - (group_keys, group_items)
pub fn extract_group_info(
    yield_columns: &[crate::core::types::YieldColumn],
) -> (Vec<ContextualExpression>, Vec<ContextualExpression>) {
    let mut group_keys = Vec::new();
    let mut group_items = Vec::new();

    for column in yield_columns {
        if column.expression.contains_aggregate() {
            group_items.push(column.expression.clone());
        } else {
            group_keys.push(column.expression.clone());
        }
    }

    group_keys.dedup_by(|a, b| a.equals_by_content(b));
    group_items.dedup_by(|a, b| a.equals_by_content(b));

    (group_keys, group_items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::def::Expression;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::expr::ExpressionMeta;
    use std::sync::Arc;

    #[test]
    fn test_extract_string_from_expr_variable() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("test_var".to_string());
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let result = extract_string_from_expr(&ctx_expr);
        assert!(result.is_ok());
        assert_eq!(
            result.expect("Failed to extract variable string"),
            "test_var"
        );
    }

    #[test]
    fn test_extract_string_from_expr_literal_string() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Literal(Value::String("hello".to_string()));
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let result = extract_string_from_expr(&ctx_expr);
        assert!(result.is_ok());
        assert_eq!(result.expect("Failed to extract string literal"), "hello");
    }

    #[test]
    fn test_extract_string_from_expr_literal_int() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Literal(Value::Int(42));
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let result = extract_string_from_expr(&ctx_expr);
        assert!(result.is_ok());
        assert_eq!(result.expect("Failed to extract integer literal"), "42");
    }

    #[test]
    fn test_generate_default_alias_from_contextual_variable() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("my_var".to_string());
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let alias = generate_default_alias_from_contextual(&ctx_expr);
        assert_eq!(alias, "my_var");
    }
}

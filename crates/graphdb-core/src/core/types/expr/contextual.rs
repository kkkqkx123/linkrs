//! context expression (computing)
//!
//! This module defines ContextualExpression as a lightweight reference to an expression.
//! Holds the ExpressionId and Context references.

use std::sync::Arc;

use super::ExpressionAnalysisContext;
use super::{Expression, ExpressionId, ExpressionMeta};
use crate::core::types::DataType;
use crate::core::Value;
/// Enhanced expression metadata with query context references
///
/// Lightweight expression references, holding ExpressionId and Context references.
/// The ExpressionAnalysisContext provides access to full information about the expression, its type, constant value, etc.
#[derive(Debug, Clone)]
pub struct ContextualExpression {
    /// Expression ID
    id: ExpressionId,
    /// Query Context References
    context: Arc<ExpressionAnalysisContext>,
}

impl ContextualExpression {
    /// Creating Context Expressions
    pub fn new(id: ExpressionId, context: Arc<ExpressionAnalysisContext>) -> Self {
        Self { id, context }
    }

    /// Get expression ID
    pub fn id(&self) -> &ExpressionId {
        &self.id
    }

    /// Get expression metadata
    pub fn expression(&self) -> Option<Arc<ExpressionMeta>> {
        self.context.get_expression(&self.id)
    }

    /// Get a clone of the underlying Expression
    ///
    /// This method is used in scenarios where you need to manipulate the Expression directly.
    /// such as template extraction, parameterization, etc. The expression() method should be used in most scenarios
    ///
    /// # Restrictions on use
    /// This method can only be used at the Executor level and is not allowed to be called at any other level.
    /// Violation of this restriction would undermine the design principles of the expression system
    pub fn get_expression(&self) -> Option<Expression> {
        self.expression().map(|meta| meta.inner.as_ref().clone())
    }

    /// Consume self and get the underlying Expression
    ///
    /// This method is used in scenarios where you need to get ownership of an Expression instead of a reference.
    ///
    /// # Restrictions on use
    /// This method can only be used at the Executor level and is not allowed to be called at any other level.
    /// Violation of this restriction would undermine the design principles of the expression system
    pub fn into_expression(self) -> Expression {
        self.get_expression()
            .expect("Expression should exist in context")
    }

    /// Get expression type
    pub fn data_type(&self) -> Option<DataType> {
        self.context.get_type(&self.id)
    }

    /// Getting Constant Values
    pub fn constant_value(&self) -> Option<Value> {
        self.context.get_constant(&self.id)
    }

    /// Whether it is a constant or not
    pub fn is_constant(&self) -> bool {
        self.context.is_constant(&self.id)
    }

    /// Whether the type derivation has been done
    pub fn is_typed(&self) -> bool {
        self.context.is_typed(&self.id)
    }

    /// Whether or not the constants have been collapsed
    pub fn is_constant_folded(&self) -> bool {
        self.context.is_constant_folded(&self.id)
    }

    /// Whether or not it has been eliminated by a public subexpression
    pub fn is_cse_eliminated(&self) -> bool {
        self.context.is_cse_eliminated(&self.id)
    }

    /// Get expression context
    pub fn context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.context
    }

    /// Checking if an expression is a literal
    pub fn is_literal(&self) -> bool {
        self.expression().map(|e| e.is_literal()).unwrap_or(false)
    }

    /// Checking if an expression is a variable
    pub fn is_variable(&self) -> bool {
        self.expression().map(|e| e.is_variable()).unwrap_or(false)
    }

    /// Check if the expression is an aggregate expression
    pub fn is_aggregate(&self) -> bool {
        self.expression().map(|e| e.is_aggregate()).unwrap_or(false)
    }

    /// Check if the expression is a property access expression
    pub fn is_property(&self) -> bool {
        self.expression()
            .map(|e| e.inner().is_property())
            .unwrap_or(false)
    }

    /// Get variable name
    pub fn as_variable(&self) -> Option<String> {
        self.expression()
            .and_then(|e| e.as_variable().map(|s| s.to_string()))
    }

    /// Get Literals
    pub fn as_literal(&self) -> Option<Value> {
        self.expression().and_then(|e| e.as_literal().cloned())
    }

    /// Getting a list of variables
    pub fn get_variables(&self) -> Vec<String> {
        self.expression()
            .map(|e| e.get_variables())
            .unwrap_or_default()
    }

    /// Convert to string representation
    pub fn to_expression_string(&self) -> String {
        self.expression()
            .map(|e| e.to_expression_string())
            .unwrap_or_else(|| format!("<unknown expression {}>", self.id.0))
    }

    /// Checking for the inclusion of aggregate functions
    pub fn contains_aggregate(&self) -> bool {
        self.expression()
            .map(|e| e.contains_aggregate())
            .unwrap_or(false)
    }

    /// Check if the expression is a function call
    pub fn is_function(&self) -> bool {
        self.expression().map(|e| e.is_function()).unwrap_or(false)
    }

    /// Check if the expression is a path expression
    pub fn is_path(&self) -> bool {
        self.expression().map(|e| e.is_path()).unwrap_or(false)
    }

    /// Check if the expression is a path-building expression
    pub fn is_path_build(&self) -> bool {
        self.expression()
            .map(|e| e.is_path_build())
            .unwrap_or(false)
    }

    /// Check if the expression is a labeled expression
    pub fn is_label(&self) -> bool {
        self.expression().map(|e| e.is_label()).unwrap_or(false)
    }

    /// Check if the expression is binary
    pub fn is_binary(&self) -> bool {
        self.expression().map(|e| e.is_binary()).unwrap_or(false)
    }

    /// Check if the expression is a unary expression
    pub fn is_unary(&self) -> bool {
        self.expression().map(|e| e.is_unary()).unwrap_or(false)
    }

    /// Checks if an expression is a type conversion expression
    pub fn is_type_cast(&self) -> bool {
        self.expression().map(|e| e.is_type_cast()).unwrap_or(false)
    }

    /// Check if the expression is a subscript access expression
    pub fn is_subscript(&self) -> bool {
        self.expression().map(|e| e.is_subscript()).unwrap_or(false)
    }

    /// Check if the expression is a range expression
    pub fn is_range(&self) -> bool {
        self.expression().map(|e| e.is_range()).unwrap_or(false)
    }

    /// Check if the expression is a list expression
    pub fn is_list(&self) -> bool {
        self.expression().map(|e| e.is_list()).unwrap_or(false)
    }

    /// Check if the expression is a mapping expression
    pub fn is_map(&self) -> bool {
        self.expression().map(|e| e.is_map()).unwrap_or(false)
    }

    /// Checks if the expression is a Case expression
    pub fn is_case(&self) -> bool {
        self.expression().map(|e| e.is_case()).unwrap_or(false)
    }

    /// Check if the expression is a Reduce expression
    pub fn is_reduce(&self) -> bool {
        self.expression().map(|e| e.is_reduce()).unwrap_or(false)
    }

    /// Check whether the expression is a parameter expression.
    pub fn is_parameter(&self) -> bool {
        self.expression().map(|e| e.is_parameter()).unwrap_or(false)
    }

    /// Check whether the expression is a list comprehension.
    pub fn is_list_comprehension(&self) -> bool {
        self.expression()
            .map(|e| e.is_list_comprehension())
            .unwrap_or(false)
    }

    /// Obtain the function name (in the case of a function call)
    pub fn as_function_name(&self) -> Option<String> {
        self.expression().and_then(|e| e.as_function_name())
    }

    /// Obtain the attribute name (in the case of attribute access)
    pub fn as_property_name(&self) -> Option<String> {
        self.expression().and_then(|e| e.as_property_name())
    }

    /// Obtain the tag name (if it is a tag expression).
    pub fn as_label_name(&self) -> Option<String> {
        self.expression().and_then(|e| e.as_label_name())
    }

    /// Obtain the parameter name (if it is a parameter expression).
    pub fn as_parameter_name(&self) -> Option<String> {
        self.expression().and_then(|e| e.as_parameter_name())
    }

    /// Check whether the expression is an empty string.
    pub fn is_empty_string(&self) -> bool {
        self.as_literal()
            .and_then(|v| match v {
                Value::String(s) => Some(s.is_empty()),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Check whether the expression satisfies the IS NOT EMPTY condition.
    pub fn is_not_empty_condition(&self) -> bool {
        let s = self.to_expression_string();
        s.contains("IS NOT EMPTY") || s.contains("is not empty")
    }

    /// Compare whether two expressions are equal (based on the content of the expressions, not on their IDs).
    pub fn equals_by_content(&self, other: &Self) -> bool {
        if let (Some(expr1), Some(expr2)) = (self.expression(), other.expression()) {
            expr1.inner() == expr2.inner()
        } else {
            false
        }
    }
}

impl PartialEq for ContextualExpression {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.context, &other.context)
    }
}

impl Eq for ContextualExpression {}

impl std::hash::Hash for ContextualExpression {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        let ptr = Arc::as_ptr(&self.context) as usize;
        ptr.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_contextual_expression_creation() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ExpressionId::new(1);
        let ctx_expr = ContextualExpression::new(id, ctx);

        assert_eq!(ctx_expr.id().0, 1);
    }

    #[test]
    fn test_contextual_expression_with_registered() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::int(42);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        let ctx_expr = ContextualExpression::new(id.clone(), ctx);

        assert!(ctx_expr.expression().is_some());
        assert!(ctx_expr.is_literal());
        assert_eq!(ctx_expr.as_literal(), Some(Value::Int(42)));
    }

    #[test]
    fn test_contextual_expression_with_type() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::int(42);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        ctx.set_type(&id, DataType::Int);

        let ctx_expr = ContextualExpression::new(id.clone(), ctx);

        assert_eq!(ctx_expr.data_type(), Some(DataType::Int));
        assert!(ctx_expr.is_typed());
    }

    #[test]
    fn test_contextual_expression_with_constant() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::binary(
            Expression::literal(1),
            BinaryOperator::Add,
            Expression::literal(2),
        );
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        ctx.set_constant(&id, Value::Int(3));

        let ctx_expr = ContextualExpression::new(id.clone(), ctx);

        assert_eq!(ctx_expr.constant_value(), Some(Value::Int(3)));
        assert!(ctx_expr.is_constant());
        assert!(ctx_expr.is_constant_folded());
    }

    #[test]
    fn test_contextual_expression_is_variable() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::variable("x");
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        let ctx_expr = ContextualExpression::new(id.clone(), ctx);

        assert!(ctx_expr.is_variable());
        assert_eq!(ctx_expr.as_variable(), Some("x".to_string()));
    }

    #[test]
    fn test_contextual_expression_get_variables() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::binary(
            Expression::variable("a"),
            BinaryOperator::Add,
            Expression::variable("b"),
        );
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        let ctx_expr = ContextualExpression::new(id.clone(), ctx);

        let vars = ctx_expr.get_variables();
        assert_eq!(vars, vec!["a", "b"]);
    }

    #[test]
    fn test_contextual_expression_to_string() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::variable("x");
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        let ctx_expr = ContextualExpression::new(id.clone(), ctx);

        let s = ctx_expr.to_expression_string();
        assert!(s.contains("x"));
    }

    #[test]
    fn test_contextual_expression_partial_eq() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ExpressionId::new(1);

        let ctx_expr1 = ContextualExpression::new(id.clone(), ctx.clone());
        let ctx_expr2 = ContextualExpression::new(id, ctx);

        assert_eq!(ctx_expr1, ctx_expr2);
    }

    #[test]
    fn test_contextual_expression_partial_eq_different_context() {
        let ctx1 = Arc::new(ExpressionAnalysisContext::new());
        let ctx2 = Arc::new(ExpressionAnalysisContext::new());
        let id = ExpressionId::new(1);

        let ctx_expr1 = ContextualExpression::new(id.clone(), ctx1);
        let ctx_expr2 = ContextualExpression::new(id, ctx2);

        assert_ne!(ctx_expr1, ctx_expr2);
    }

    #[test]
    fn test_contextual_expression_context() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ExpressionId::new(1);
        let ctx_expr = ContextualExpression::new(id, ctx.clone());

        assert!(Arc::ptr_eq(ctx_expr.context(), &ctx));
    }
}

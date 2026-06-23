//! serializable expression
//!
//! This module defines SerializableExpression for storage and transmission.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::contextual::ContextualExpression;
use super::expression_context::ExpressionAnalysisContext;
use super::{Expression, ExpressionId, ExpressionMeta};
use crate::core::types::DataType;
use crate::core::Value;

/// Serializable expression references (for storage/transmission)
///
/// Contains complete information about the expression and can be serialized and deserialized.
/// For use in scenarios where serialization is required (e.g., network transfers, persistence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableExpression {
    pub id: ExpressionId,
    pub expression: Expression,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_type: Option<DataType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constant_value: Option<Value>,
}

impl SerializableExpression {
    /// Conversion from ContextualExpression to serializable form
    pub fn from_contextual(ctx_expr: &ContextualExpression) -> Self {
        let expr_meta = ctx_expr
            .expression()
            .expect("Expression not found in context");
        Self {
            id: ctx_expr.id().clone(),
            expression: expr_meta.inner().clone(),
            data_type: ctx_expr.data_type(),
            constant_value: ctx_expr.constant_value(),
        }
    }

    /// Convert to ContextualExpression
    pub fn to_contextual(self, ctx: Arc<ExpressionAnalysisContext>) -> ContextualExpression {
        let expr_meta = ExpressionMeta::new(self.expression).with_id(self.id.clone());
        ctx.register_expression(expr_meta);

        if let Some(data_type) = self.data_type {
            ctx.set_type(&self.id, data_type);
        }

        if let Some(constant_value) = self.constant_value {
            ctx.set_constant(&self.id, constant_value);
        }

        ContextualExpression::new(self.id, ctx)
    }

    /// Get expression ID
    pub fn id(&self) -> &ExpressionId {
        &self.id
    }

    /// Get expression
    pub fn expression(&self) -> &Expression {
        &self.expression
    }

    /// Getting the data type
    pub fn data_type(&self) -> Option<&DataType> {
        self.data_type.as_ref()
    }

    /// Getting Constant Values
    pub fn constant_value(&self) -> Option<&Value> {
        self.constant_value.as_ref()
    }

    /// Whether it is a constant or not
    pub fn is_constant(&self) -> bool {
        self.constant_value.is_some()
    }

    /// Checking if an expression is a literal
    pub fn is_literal(&self) -> bool {
        self.expression.is_literal()
    }

    /// Checking if an expression is a variable
    pub fn is_variable(&self) -> bool {
        self.expression.is_variable()
    }

    /// Check if the expression is an aggregate expression
    pub fn is_aggregate(&self) -> bool {
        self.expression.is_aggregate()
    }

    /// Get variable name
    pub fn as_variable(&self) -> Option<String> {
        self.expression.as_variable().map(|s| s.to_string())
    }

    /// Get Literals
    pub fn as_literal(&self) -> Option<Value> {
        self.expression.as_literal().cloned()
    }

    /// Getting a list of variables
    pub fn get_variables(&self) -> Vec<String> {
        self.expression.get_variables()
    }

    /// Convert to string representation
    pub fn to_expression_string(&self) -> String {
        self.expression.to_expression_string()
    }

    /// Checking for the inclusion of aggregate functions
    pub fn contains_aggregate(&self) -> bool {
        self.expression.contains_aggregate()
    }
}

impl PartialEq for SerializableExpression {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.expression == other.expression
    }
}

impl Eq for SerializableExpression {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_serializable_expression_creation() {
        let expr = Expression::int(42);
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: Some(DataType::Int),
            constant_value: Some(Value::Int(42)),
        };

        assert_eq!(ser_expr.id().0, 1);
        assert!(ser_expr.is_literal());
        assert_eq!(ser_expr.as_literal(), Some(Value::Int(42)));
        assert_eq!(ser_expr.data_type(), Some(&DataType::Int));
        assert_eq!(ser_expr.constant_value(), Some(&Value::Int(42)));
        assert!(ser_expr.is_constant());
    }

    #[test]
    fn test_serializable_expression_from_contextual() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        ctx.set_type(&id, DataType::Int);
        ctx.set_constant(&id, Value::Int(42));

        let ctx_expr = ContextualExpression::new(id, ctx);
        let ser_expr = SerializableExpression::from_contextual(&ctx_expr);

        assert_eq!(ser_expr.id().0, 0);
        assert_eq!(ser_expr.data_type(), Some(&DataType::Int));
        assert_eq!(ser_expr.constant_value(), Some(&Value::Int(42)));
    }

    #[test]
    fn test_serializable_expression_to_contextual() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::literal(42);
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: Some(DataType::Int),
            constant_value: Some(Value::Int(42)),
        };

        let ctx_expr = ser_expr.to_contextual(ctx);

        assert_eq!(ctx_expr.id().0, 1);
        assert_eq!(ctx_expr.data_type(), Some(DataType::Int));
        assert_eq!(ctx_expr.constant_value(), Some(Value::Int(42)));
        assert!(ctx_expr.is_constant());
    }

    #[test]
    fn test_serializable_expression_is_variable() {
        let expr = Expression::variable("x");
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: None,
            constant_value: None,
        };

        assert!(ser_expr.is_variable());
        assert_eq!(ser_expr.as_variable(), Some("x".to_string()));
    }

    #[test]
    fn test_serializable_expression_get_variables() {
        let expr = Expression::binary(
            Expression::variable("a"),
            BinaryOperator::Add,
            Expression::variable("b"),
        );
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: None,
            constant_value: None,
        };

        let vars = ser_expr.get_variables();
        assert_eq!(vars, vec!["a", "b"]);
    }

    #[test]
    fn test_serializable_expression_to_string() {
        let expr = Expression::variable("x");
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: None,
            constant_value: None,
        };

        let s = ser_expr.to_expression_string();
        assert!(s.contains("x"));
    }

    #[test]
    fn test_serializable_expression_serde() {
        let expr = Expression::literal(42);
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: Some(DataType::Int),
            constant_value: Some(Value::Int(42)),
        };

        let json = serde_json::to_string(&ser_expr).expect("Serialization should succeed");
        let decoded: SerializableExpression =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(ser_expr, decoded);
    }

    #[test]
    fn test_serializable_expression_partial_eq() {
        let expr1 = Expression::literal(42);
        let ser_expr1 = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr1,
            data_type: Some(DataType::Int),
            constant_value: Some(Value::Int(42)),
        };

        let expr2 = Expression::literal(42);
        let ser_expr2 = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr2,
            data_type: Some(DataType::Int),
            constant_value: Some(Value::Int(42)),
        };

        assert_eq!(ser_expr1, ser_expr2);
    }

    #[test]
    fn test_serializable_expression_partial_eq_different_id() {
        let expr1 = Expression::literal(42);
        let ser_expr1 = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr1,
            data_type: Some(DataType::Int),
            constant_value: Some(Value::Int(42)),
        };

        let expr2 = Expression::literal(42);
        let ser_expr2 = SerializableExpression {
            id: ExpressionId::new(2),
            expression: expr2,
            data_type: Some(DataType::Int),
            constant_value: Some(Value::Int(42)),
        };

        assert_ne!(ser_expr1, ser_expr2);
    }

    #[test]
    fn test_serializable_expression_is_aggregate() {
        use crate::core::types::operators::AggregateFunction;

        let expr = Expression::aggregate(
            AggregateFunction::Count(None),
            Expression::variable("x"),
            false,
        );
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: None,
            constant_value: None,
        };

        assert!(ser_expr.is_aggregate());
    }

    #[test]
    fn test_serializable_expression_contains_aggregate() {
        use crate::core::types::operators::AggregateFunction;

        let expr = Expression::aggregate(
            AggregateFunction::Count(None),
            Expression::variable("x"),
            false,
        );
        let ser_expr = SerializableExpression {
            id: ExpressionId::new(1),
            expression: expr,
            data_type: None,
            constant_value: None,
        };

        assert!(ser_expr.contains_aggregate());
    }
}

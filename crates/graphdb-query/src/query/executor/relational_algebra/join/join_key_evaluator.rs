//! The Join key evaluator
//!
//! Keys specifically designed for use in the Join operation, which support the evaluation of expressions into Value types that can be hashed.

use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::ExpressionError;

/// The Join key evaluator
///
/// An expression evaluator specifically designed for the Join operation evaluates expressions to a hashable Value type.
/// Using the unit struct pattern, with zero overhead.
#[derive(Debug)]
pub struct JoinKeyEvaluator;

impl JoinKeyEvaluator {
    pub fn evaluate_key<C: ExpressionContext>(
        expression: &Expression,
        context: &mut C,
    ) -> Result<Value, ExpressionError> {
        ExpressionEvaluator::evaluate(expression, context)
    }

    pub fn evaluate_keys<C: ExpressionContext>(
        exprs: &[Expression],
        context: &mut C,
    ) -> Result<Vec<Value>, ExpressionError> {
        let mut keys = Vec::with_capacity(exprs.len());
        for expression in exprs {
            keys.push(Self::evaluate_key(expression, context)?);
        }
        Ok(keys)
    }

    pub fn is_simple_variable(expression: &Expression) -> bool {
        matches!(expression, Expression::Variable(_))
    }

    pub fn is_simple_property(expression: &Expression) -> bool {
        matches!(expression, Expression::Property { .. })
    }

    pub fn get_variable_name(expression: &Expression) -> Option<&str> {
        match expression {
            Expression::Variable(name) => Some(name),
            _ => None,
        }
    }

    pub fn get_property_info(expression: &Expression) -> Option<(&Expression, &str)> {
        match expression {
            Expression::Property { object, property } => Some((object.as_ref(), property)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;

    #[test]
    fn test_is_simple_variable() {
        let var_expression = Expression::Variable("id".to_string());
        assert!(JoinKeyEvaluator::is_simple_variable(&var_expression));

        let lit_expression = Expression::Literal(Value::Int(42));
        assert!(!JoinKeyEvaluator::is_simple_variable(&lit_expression));
    }

    #[test]
    fn test_get_variable_name() {
        let var_expression = Expression::Variable("name".to_string());
        assert_eq!(
            JoinKeyEvaluator::get_variable_name(&var_expression),
            Some("name")
        );

        let lit_expression = Expression::Literal(Value::Int(42));
        assert_eq!(JoinKeyEvaluator::get_variable_name(&lit_expression), None);
    }

    #[test]
    fn test_is_simple_property() {
        let prop_expression = Expression::Property {
            object: Box::new(Expression::Variable("person".to_string())),
            property: "age".to_string(),
        };
        assert!(JoinKeyEvaluator::is_simple_property(&prop_expression));

        let var_expression = Expression::Variable("id".to_string());
        assert!(!JoinKeyEvaluator::is_simple_property(&var_expression));
    }

    #[test]
    fn test_get_property_info() {
        let prop_expression = Expression::Property {
            object: Box::new(Expression::Variable("person".to_string())),
            property: "age".to_string(),
        };
        let (object, property) = JoinKeyEvaluator::get_property_info(&prop_expression)
            .expect("get_property_info should succeed");
        assert!(matches!(object, Expression::Variable(_)));
        assert_eq!(property, "age");
    }
}

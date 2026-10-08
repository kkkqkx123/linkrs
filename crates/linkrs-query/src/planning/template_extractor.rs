//! Query Template Extractor
//!
//! This module provides functions for parameterizing query requests and extracting templates, which are used for planning the caching process.
//! Replace the specific parameter values with placeholders, so that queries with semantically equivalent content can share the cache.

use linkrs_core::types::expr::Expression;
use linkrs_core::{NullType, Value};

mod parameterize;
mod transform;

pub use parameterize::ParameterizingTransformer;
pub use transform::TemplateExtractor;

/// Parameterized results
#[derive(Debug, Clone)]
pub struct ParameterizedResult {
    /// Parameterized expression
    pub expression: Expression,
    /// List of extracted parameter values
    pub parameters: Vec<Value>,
    /// Parameter counter
    param_count: usize,
}

impl ParameterizedResult {
    fn new() -> Self {
        Self {
            expression: Expression::Literal(Value::Null(NullType::Null)),
            parameters: Vec::new(),
            param_count: 0,
        }
    }

    fn next_param_name(&mut self) -> String {
        self.param_count += 1;
        format!("${}", self.param_count)
    }

    fn add_parameter(&mut self, value: Value) -> String {
        let name = self.next_param_name();
        self.parameters.push(value);
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_core::Value;

    #[test]
    fn test_parameterize_literal() {
        let mut transformer = ParameterizingTransformer::new();
        let expr = Expression::Literal(Value::Int(42));
        let result = transformer.parameterize_expression(&expr);

        assert_eq!(result.parameters.len(), 1);
        assert_eq!(result.parameters[0], Value::Int(42));
        assert!(matches!(result.expression, Expression::Variable(ref name) if name == "$1"));
    }

    #[test]
    fn test_parameterize_binary_expr() {
        use linkrs_core::types::operators::BinaryOperator;

        let mut transformer = ParameterizingTransformer::new();
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("age".to_string())),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Int(18))),
        };
        let result = transformer.parameterize_expression(&expr);

        assert_eq!(result.parameters.len(), 1);
        assert_eq!(result.parameters[0], Value::Int(18));
    }

    #[test]
    fn test_expr_to_template_string() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("$1".to_string())),
            op: linkrs_core::types::operators::BinaryOperator::Equal,
            right: Box::new(Expression::Variable("name".to_string())),
        };

        let template = TemplateExtractor::expr_to_template_string(&expr);
        assert!(template.contains("$1"));
        assert!(template.contains("name"));
    }
}

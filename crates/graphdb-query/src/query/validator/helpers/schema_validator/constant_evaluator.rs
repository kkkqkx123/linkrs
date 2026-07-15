use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::validator::error::{ValidationError as CoreValidationError, ValidationErrorType};

use super::schema_lookup::SchemaValidator;

impl SchemaValidator {
    pub fn is_evaluable_expr(&self, expr: &ContextualExpression) -> bool {
        if let Some(e) = expr.get_expression() {
            self.is_evaluable_expr_internal(&e)
        } else {
            false
        }
    }

    fn is_evaluable_expr_internal(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Literal(_) => true,
            Expression::Variable(_) => true,
            Expression::List(list) => list.iter().all(|e| self.is_evaluable_expr_internal(e)),
            Expression::Map(map) => map.iter().all(|(_, e)| self.is_evaluable_expr_internal(e)),
            Expression::Function { .. } => true,
            _ => false,
        }
    }

    pub fn evaluate_expression(
        &self,
        expr: &ContextualExpression,
    ) -> Result<Value, CoreValidationError> {
        if let Some(e) = expr.get_expression() {
            self.evaluate_expression_internal(&e)
        } else {
            Err(CoreValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    fn evaluate_expression_internal(
        &self,
        expr: &Expression,
    ) -> Result<Value, CoreValidationError> {
        match expr {
            Expression::Literal(value) => Ok(value.clone()),
            Expression::Variable(name) => Ok(Value::String(format!("${}", name))),
            Expression::List(list) => {
                let values: Result<Vec<_>, _> = list
                    .iter()
                    .map(|e| self.evaluate_expression_internal(e))
                    .collect();
                Ok(Value::list(crate::core::value::List { values: values? }))
            }
            Expression::Map(map) => {
                let mut result = std::collections::HashMap::new();
                for (k, v) in map {
                    result.insert(k.clone(), self.evaluate_expression_internal(v)?);
                }
                Ok(Value::map(result))
            }
            _ => Err(CoreValidationError::new(
                format!("Unable to evaluate expression: {:?}", expr),
                ValidationErrorType::SemanticError,
            )),
        }
    }
}

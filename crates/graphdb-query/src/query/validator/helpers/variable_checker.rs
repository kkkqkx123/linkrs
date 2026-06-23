//! Variable checking tool
//! Responsible for verifying the scope of variables, the naming conventions, and their usage.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::AliasType;
use std::collections::HashMap;

pub struct VariableChecker;

impl Default for VariableChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl VariableChecker {
    pub fn new() -> Self {
        Self
    }

    pub fn validate_variable_scope(
        &self,
        expression: &ContextualExpression,
        available_aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        if let Some(expr_meta) = expression.expression() {
            let expr = expr_meta.inner();
            let variables = self.extract_variables_internal(expr);

            for var in &variables {
                self.validate_variable_usage(var, available_aliases)?;
            }
        }

        Ok(())
    }

    pub fn validate_variable_name_format(&self, var: &str) -> Result<(), ValidationError> {
        if var.is_empty() {
            return Err(ValidationError::new(
                "The variable name cannot be null".to_string(),
                ValidationErrorType::SyntaxError,
            ));
        }

        let first_char = var.chars().next().ok_or_else(|| {
            ValidationError::new(
                "The variable name cannot be null".to_string(),
                ValidationErrorType::SyntaxError,
            )
        })?;
        if !first_char.is_alphabetic() && first_char != '_' {
            return Err(ValidationError::new(
                format!(
                    "Variable names must begin with a letter or an underscore: {:?}",
                    var
                ),
                ValidationErrorType::SyntaxError,
            ));
        }

        if !var.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(ValidationError::new(
                format!(
                    "Variable names can only contain letters, numbers and underscores: {:?}",
                    var
                ),
                ValidationErrorType::SyntaxError,
            ));
        }

        if var.len() > 255 {
            return Err(ValidationError::new(
                format!("Variable names too long: {:?}", var),
                ValidationErrorType::SyntaxError,
            ));
        }

        Ok(())
    }

    fn validate_variable_usage(
        &self,
        var: &str,
        available_aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        self.validate_variable_name_format(var)?;

        if !available_aliases.contains_key(var) {
            return Err(ValidationError::new(
                format!("Variable {:?} Undefined", var),
                ValidationErrorType::VariableNotFound,
            ));
        }

        Ok(())
    }

    pub fn validate_variable_scope_simple(
        &self,
        variables: &[String],
    ) -> Result<(), ValidationError> {
        for var in variables {
            self.validate_variable_name_format(var)?;
        }
        Ok(())
    }

    pub fn extract_variables(&self, expression: &ContextualExpression) -> Vec<String> {
        if let Some(expr_meta) = expression.expression() {
            let expr = expr_meta.inner();
            return self.extract_variables_internal(expr);
        }
        Vec::new()
    }

    fn extract_variables_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> Vec<String> {
        let mut variables = Vec::new();
        self.collect_variables_internal(expression, &mut variables);
        variables
    }

    fn collect_variables_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        variables: &mut Vec<String>,
    ) {
        match expression {
            crate::core::types::expr::Expression::Variable(name) if !variables.contains(name) => {
                variables.push(name.clone());
            }
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                self.collect_variables_internal(left, variables);
                self.collect_variables_internal(right, variables);
            }
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.collect_variables_internal(operand, variables);
            }
            crate::core::types::expr::Expression::Function { args, .. } => {
                for arg in args {
                    self.collect_variables_internal(arg, variables);
                }
            }
            crate::core::types::expr::Expression::Aggregate { arg, .. } => {
                self.collect_variables_internal(arg, variables);
            }
            crate::core::types::expr::Expression::Property {
                object: inner_expression,
                ..
            } => {
                self.collect_variables_internal(inner_expression, variables);
            }
            crate::core::types::expr::Expression::Subscript {
                collection: inner_expression,
                index,
            } => {
                self.collect_variables_internal(inner_expression, variables);
                self.collect_variables_internal(index, variables);
            }
            crate::core::types::expr::Expression::List(items) => {
                for item in items {
                    self.collect_variables_internal(item, variables);
                }
            }
            crate::core::types::expr::Expression::Map(pairs) => {
                for (_, value) in pairs {
                    self.collect_variables_internal(value, variables);
                }
            }
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test_expression) = test_expr {
                    self.collect_variables_internal(test_expression, variables);
                }
                for (when_expression, then_expression) in conditions {
                    self.collect_variables_internal(when_expression, variables);
                    self.collect_variables_internal(then_expression, variables);
                }
                if let Some(else_expression) = default {
                    self.collect_variables_internal(else_expression, variables);
                }
            }
            _ => {}
        }
    }

    pub fn contains_variable(&self, expression: &ContextualExpression, var: &str) -> bool {
        if let Some(expr_meta) = expression.expression() {
            let expr = expr_meta.inner();
            return self.contains_variable_internal(expr, var);
        }
        false
    }

    fn contains_variable_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        var: &str,
    ) -> bool {
        match expression {
            crate::core::types::expr::Expression::Variable(name) => name == var,
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                self.contains_variable_internal(left, var)
                    || self.contains_variable_internal(right, var)
            }
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.contains_variable_internal(operand, var)
            }
            crate::core::types::expr::Expression::Function { args, .. } => args
                .iter()
                .any(|arg| self.contains_variable_internal(arg, var)),
            crate::core::types::expr::Expression::Aggregate { arg, .. } => {
                self.contains_variable_internal(arg, var)
            }
            crate::core::types::expr::Expression::Property {
                object: inner_expression,
                ..
            } => self.contains_variable_internal(inner_expression, var),
            crate::core::types::expr::Expression::Subscript {
                collection: inner_expression,
                index,
            } => {
                self.contains_variable_internal(inner_expression, var)
                    || self.contains_variable_internal(index, var)
            }
            crate::core::types::expr::Expression::List(items) => items
                .iter()
                .any(|item| self.contains_variable_internal(item, var)),
            crate::core::types::expr::Expression::Map(pairs) => pairs
                .iter()
                .any(|(_, value)| self.contains_variable_internal(value, var)),
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let mut has_var = false;
                if let Some(test_expression) = test_expr {
                    has_var = has_var || self.contains_variable_internal(test_expression, var);
                }
                for (when_expression, then_expression) in conditions {
                    has_var = has_var || self.contains_variable_internal(when_expression, var);
                    has_var = has_var || self.contains_variable_internal(then_expression, var);
                }
                if let Some(else_expression) = default {
                    has_var = has_var || self.contains_variable_internal(else_expression, var);
                }
                has_var
            }
            _ => false,
        }
    }

    pub fn validate_expression_variables(
        &self,
        expression: &ContextualExpression,
        available_aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        self.validate_variable_scope(expression, available_aliases)
    }

    pub fn is_arithmetic_expression(&self, expression: &ContextualExpression, var: &str) -> bool {
        if let Some(expr_meta) = expression.expression() {
            let expr = expr_meta.inner();
            return self.is_arithmetic_expression_internal(expr, var);
        }
        false
    }

    fn is_arithmetic_expression_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        var: &str,
    ) -> bool {
        match expression {
            crate::core::types::expr::Expression::Binary {
                op:
                    crate::core::types::operators::BinaryOperator::Add
                    | crate::core::types::operators::BinaryOperator::Subtract
                    | crate::core::types::operators::BinaryOperator::Multiply
                    | crate::core::types::operators::BinaryOperator::Divide
                    | crate::core::types::operators::BinaryOperator::Modulo,
                left,
                right,
            } => {
                self.contains_variable_internal(left, var)
                    || self.contains_variable_internal(right, var)
            }
            crate::core::types::expr::Expression::Unary {
                op:
                    crate::core::types::operators::UnaryOperator::Minus
                    | crate::core::types::operators::UnaryOperator::Plus,
                operand,
            } => self.contains_variable_internal(operand, var),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::ContextualExpression;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_variable_checker_creation() {
        let _checker = VariableChecker::new();
        // If the test is successful and you have reached this point, it means that everything has gone as planned.
    }

    #[test]
    fn test_validate_variable_name_format() {
        let checker = VariableChecker::new();

        assert!(checker.validate_variable_name_format("var").is_ok());
        assert!(checker.validate_variable_name_format("var1").is_ok());
        assert!(checker.validate_variable_name_format("var_name").is_ok());
        assert!(checker.validate_variable_name_format("_var").is_ok());

        assert!(checker.validate_variable_name_format("").is_err());
        assert!(checker.validate_variable_name_format("1var").is_err());
        assert!(checker.validate_variable_name_format("var-name").is_err());
        assert!(checker.validate_variable_name_format("var name").is_err());
    }

    #[test]
    fn test_contains_variable() {
        let checker = VariableChecker::new();

        let var_expression = Expression::Variable("test_var".to_string());
        let meta = ExpressionMeta::new(var_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(checker.contains_variable(&ctx_expr, "test_var"));
        assert!(!checker.contains_variable(&ctx_expr, "other_var"));

        let literal_expression = Expression::Literal(Value::Int(42));
        let meta = ExpressionMeta::new(literal_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(!checker.contains_variable(&ctx_expr, "test_var"));
    }

    #[test]
    fn test_is_arithmetic_expression() {
        let checker = VariableChecker::new();

        let add_expression = Expression::Binary {
            op: crate::core::BinaryOperator::Add,
            left: Box::new(Expression::Variable("var".to_string())),
            right: Box::new(Expression::Literal(Value::Int(1))),
        };
        let meta = ExpressionMeta::new(add_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(checker.is_arithmetic_expression(&ctx_expr, "var"));

        let eq_expression = Expression::Binary {
            op: crate::core::BinaryOperator::Equal,
            left: Box::new(Expression::Variable("var".to_string())),
            right: Box::new(Expression::Literal(Value::Int(1))),
        };
        let meta = ExpressionMeta::new(eq_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(!checker.is_arithmetic_expression(&ctx_expr, "var"));
    }

    #[test]
    fn test_extract_variables() {
        let checker = VariableChecker::new();

        let complex_expression = Expression::Binary {
            op: crate::core::BinaryOperator::Add,
            left: Box::new(Expression::Variable("var1".to_string())),
            right: Box::new(Expression::Variable("var2".to_string())),
        };
        let meta = ExpressionMeta::new(complex_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        let variables = checker.extract_variables(&ctx_expr);
        assert_eq!(variables.len(), 2);
        assert!(variables.contains(&"var1".to_string()));
        assert!(variables.contains(&"var2".to_string()));
    }
}

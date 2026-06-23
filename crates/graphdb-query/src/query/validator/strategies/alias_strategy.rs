//! Alias Verification Policy
//! Responsible for verifying alias references and their availability in expressions.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::AliasType;
use std::collections::HashMap;

/// Alias validation policy
pub struct AliasValidationStrategy;

impl Default for AliasValidationStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl AliasValidationStrategy {
    pub fn new() -> Self {
        Self
    }

    /// Verify the aliases in the list of expressions.
    pub fn validate_aliases(
        &self,
        exprs: &[ContextualExpression],
        aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        for expression in exprs {
            self.validate_expression_aliases(expression, aliases)?;
        }
        Ok(())
    }

    /// Verify aliases in a single expression
    pub fn validate_expression_aliases(
        &self,
        expression: &ContextualExpression,
        aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        // Obtain the Expression from ContextualExpression.
        let expr_meta = match expression.expression() {
            Some(e) => e,
            None => return Ok(()),
        };
        let expr = expr_meta.inner();

        // First, check whether the expression itself references an alias.
        if let Some(alias_name) = self.extract_alias_name_internal(expr) {
            if !aliases.contains_key(&alias_name) {
                return Err(ValidationError::new(
                    format!("Undefined variable aliases: {}", alias_name),
                    ValidationErrorType::AliasError,
                ));
            }
        }

        // Recursive verification of subexpressions
        self.validate_subexpressions_aliases_internal(expr, aliases)?;

        Ok(())
    }

    /// Extract the alias names from the expression.
    pub fn extract_alias_name(&self, expression: &ContextualExpression) -> Option<String> {
        let expr_meta = expression.expression()?;
        self.extract_alias_name_internal(expr_meta.inner())
    }

    /// Internal method: Extracting alias names from expressions
    fn extract_alias_name_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> Option<String> {
        match expression {
            crate::core::types::expr::Expression::Variable(name) => Some(name.clone()),
            crate::core::types::expr::Expression::Property { object, .. } => {
                self.extract_alias_name_internal(object)
            }
            crate::core::types::expr::Expression::Label(name) => Some(name.clone()),
            crate::core::types::expr::Expression::TagProperty { tag_name, .. } => {
                Some(tag_name.clone())
            }
            crate::core::types::expr::Expression::EdgeProperty { edge_name, .. } => {
                Some(edge_name.clone())
            }
            crate::core::types::expr::Expression::LabelTagProperty { tag, .. } => {
                self.extract_alias_name_internal(tag)
            }
            crate::core::types::expr::Expression::Parameter(name) => Some(name.clone()),
            crate::core::types::expr::Expression::ListComprehension { variable, .. } => {
                Some(variable.clone())
            }
            crate::core::types::expr::Expression::Reduce {
                accumulator,
                variable: _,
                ..
            } => Some(accumulator.clone()),
            crate::core::types::expr::Expression::PathBuild(items) => {
                if let Some(first) = items.first() {
                    self.extract_alias_name_internal(first)
                } else {
                    None
                }
            }
            crate::core::types::expr::Expression::Path(items) => {
                if let Some(first) = items.first() {
                    self.extract_alias_name_internal(first)
                } else {
                    None
                }
            }
            crate::core::types::expr::Expression::Subscript { collection, .. } => {
                self.extract_alias_name_internal(collection)
            }
            crate::core::types::expr::Expression::Range { collection, .. } => {
                self.extract_alias_name_internal(collection)
            }
            crate::core::types::expr::Expression::TypeCast { expression, .. } => {
                self.extract_alias_name_internal(expression)
            }
            crate::core::types::expr::Expression::Aggregate { arg, .. } => {
                self.extract_alias_name_internal(arg)
            }
            crate::core::types::expr::Expression::Function { name, args } => {
                match name.to_lowercase().as_str() {
                    "startnode" | "endnode" => {
                        if let Some(arg) = args.first() {
                            self.extract_alias_name_internal(arg)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.extract_alias_name_internal(operand)
            }
            crate::core::types::expr::Expression::Binary { left, .. } => {
                self.extract_alias_name_internal(left)
            }
            crate::core::types::expr::Expression::Case { test_expr, .. } => test_expr
                .as_ref()
                .and_then(|e| self.extract_alias_name_internal(e)),
            crate::core::types::expr::Expression::Literal(_)
            | crate::core::types::expr::Expression::List(_)
            | crate::core::types::expr::Expression::Map(_)
            | crate::core::types::expr::Expression::Vector(_)
            | crate::core::types::expr::Expression::Predicate { .. } => None,
        }
    }

    /// Internal method: Recursively verifying aliases in subexpressions
    fn validate_subexpressions_aliases_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        match expression {
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.validate_expression_aliases_internal(operand, aliases)
            }
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                self.validate_expression_aliases_internal(left, aliases)?;
                self.validate_expression_aliases_internal(right, aliases)
            }
            crate::core::types::expr::Expression::Function { args, .. } => {
                for arg in args {
                    self.validate_expression_aliases_internal(arg, aliases)?;
                }
                Ok(())
            }
            crate::core::types::expr::Expression::List(items) => {
                for item in items {
                    self.validate_expression_aliases_internal(item, aliases)?;
                }
                Ok(())
            }
            crate::core::types::expr::Expression::Map(items) => {
                for (_, value) in items {
                    self.validate_expression_aliases_internal(value, aliases)?;
                }
                Ok(())
            }
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test_expression) = test_expr {
                    self.validate_expression_aliases_internal(test_expression, aliases)?;
                }
                for (condition, value) in conditions {
                    self.validate_expression_aliases_internal(condition, aliases)?;
                    self.validate_expression_aliases_internal(value, aliases)?;
                }
                if let Some(default_expression) = default {
                    self.validate_expression_aliases_internal(default_expression, aliases)?;
                }
                Ok(())
            }
            crate::core::types::expr::Expression::Subscript { collection, index } => {
                self.validate_expression_aliases_internal(collection, aliases)?;
                self.validate_expression_aliases_internal(index, aliases)
            }
            crate::core::types::expr::Expression::Literal(_)
            | crate::core::types::expr::Expression::Property { .. }
            | crate::core::types::expr::Expression::Variable(_)
            | crate::core::types::expr::Expression::Label(_)
            | crate::core::types::expr::Expression::ListComprehension { .. }
            | crate::core::types::expr::Expression::TagProperty { .. }
            | crate::core::types::expr::Expression::EdgeProperty { .. }
            | crate::core::types::expr::Expression::LabelTagProperty { .. }
            | crate::core::types::expr::Expression::Predicate { .. }
            | crate::core::types::expr::Expression::Reduce { .. }
            | crate::core::types::expr::Expression::PathBuild(_)
            | crate::core::types::expr::Expression::Parameter(_)
            | crate::core::types::expr::Expression::Vector(_) => Ok(()),
            crate::core::types::expr::Expression::TypeCast { expression, .. } => {
                // Type conversion expressions require that their subexpressions be validated.
                self.validate_expression_aliases_internal(expression, aliases)
            }
            crate::core::types::expr::Expression::Aggregate { arg, .. } => {
                // Aggregate function expressions need to have their parameter expressions verified.
                self.validate_expression_aliases_internal(arg, aliases)
            }
            crate::core::types::expr::Expression::Range {
                collection,
                start,
                end,
            } => {
                // Range access expressions require the validation of both the set and the range expression itself.
                self.validate_expression_aliases_internal(collection, aliases)?;
                if let Some(start_expression) = start {
                    self.validate_expression_aliases_internal(start_expression, aliases)?;
                }
                if let Some(end_expression) = end {
                    self.validate_expression_aliases_internal(end_expression, aliases)?;
                }
                Ok(())
            }
            crate::core::types::expr::Expression::Path(items) => {
                // Path expressions need to have all their components verified.
                for item in items {
                    self.validate_expression_aliases_internal(item, aliases)?;
                }
                Ok(())
            }
        }
    }

    /// Internal method: Verifying aliases in a single expression
    fn validate_expression_aliases_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        // First, check whether the expression itself references an alias.
        if let Some(alias_name) = self.extract_alias_name_internal(expression) {
            if !aliases.contains_key(&alias_name) {
                return Err(ValidationError::new(
                    format!("Undefined variable aliases: {}", alias_name),
                    ValidationErrorType::AliasError,
                ));
            }
        }

        // Recursive verification of subexpressions
        self.validate_subexpressions_aliases_internal(expression, aliases)
    }

    /// Check whether the alias types match the way they are being used.
    pub fn check_alias(
        &self,
        ref_expression: &ContextualExpression,
        aliases_available: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        // Extract the alias names from the expression.
        if let Some(alias_name) = self.extract_alias_name(ref_expression) {
            if !aliases_available.contains_key(&alias_name) {
                return Err(ValidationError::new(
                    format!("Undefined aliases: {}", alias_name),
                    ValidationErrorType::AliasError,
                ));
            }
        }

        Ok(())
    }

    /// Taking into account the aliases…
    pub fn combine_aliases(
        &self,
        cur_aliases: &mut HashMap<String, AliasType>,
        last_aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        for (name, alias_type) in last_aliases {
            if !cur_aliases.contains_key(name)
                && cur_aliases
                    .insert(name.clone(), alias_type.clone())
                    .is_some()
            {
                return Err(ValidationError::new(
                    format!("`{}': duplicate defined aliases", name),
                    ValidationErrorType::AliasError,
                ));
            }
        }
        Ok(())
    }
}

impl AliasValidationStrategy {
    /// Obtain the policy name
    pub fn strategy_name(&self) -> &'static str {
        "AliasValidationStrategy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::Expression;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_alias_validation_strategy_creation() {
        let strategy = AliasValidationStrategy::new();
        assert_eq!(strategy.strategy_name(), "AliasValidationStrategy");
    }

    #[test]
    fn test_extract_alias_name() {
        let strategy = AliasValidationStrategy::new();

        // The test involves extracting aliases from variable expressions.
        let var_expression = Expression::Variable("test_var".to_string());
        let meta = ExpressionMeta::new(var_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert_eq!(
            strategy.extract_alias_name(&ctx_expr),
            Some("test_var".to_string())
        );

        // The test checks whether aliases can be extracted from constant expressions (it should return None).
        let const_expression = Expression::Literal(crate::core::Value::Int(42));
        let meta = ExpressionMeta::new(const_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert_eq!(strategy.extract_alias_name(&ctx_expr), None);
    }
}

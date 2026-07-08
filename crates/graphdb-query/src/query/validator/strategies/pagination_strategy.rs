//! Pagination validation strategy
//! Responsible for verifying expressions related to SKIP, LIMIT, and pagination.

use crate::core::types::expr::analysis_utils::is_evaluable;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::YieldColumn;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::{MatchStepRange, OrderByClauseContext, PaginationContext};

/// Pagination validation strategy
pub struct PaginationValidationStrategy;

impl Default for PaginationValidationStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl PaginationValidationStrategy {
    pub fn new() -> Self {
        Self
    }

    /// Verify the validity of the pagination parameters.
    pub fn validate_pagination(
        &self,
        skip_expression: Option<&ContextualExpression>,
        limit_expression: Option<&ContextualExpression>,
        context: &PaginationContext,
    ) -> Result<(), ValidationError> {
        // Verify the validity of the paging parameters.
        if context.skip < 0 {
            return Err(ValidationError::new(
                "SKIP cannot be negative".to_string(),
                ValidationErrorType::PaginationError,
            ));
        }
        if context.limit < 0 {
            return Err(ValidationError::new(
                "LIMIT cannot be negative".to_string(),
                ValidationErrorType::PaginationError,
            ));
        }

        // Verify the SKIP expression
        if let Some(expr) = skip_expression {
            self.validate_pagination_expression(expr, "SKIP")?;
        }

        // Verify the LIMIT expression
        if let Some(expr) = limit_expression {
            self.validate_pagination_expression(expr, "LIMIT")?;
        }

        Ok(())
    }

    /// Verify the pagination expression
    fn validate_pagination_expression(
        &self,
        expression: &ContextualExpression,
        clause_name: &str,
    ) -> Result<(), ValidationError> {
        let expr_meta = match expression.expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    format!("{} expression is invalid", clause_name),
                    ValidationErrorType::PaginationError,
                ))
            }
        };
        let expr = expr_meta.inner();

        self.validate_pagination_expression_internal(expr, clause_name)
    }

    /// Internal method: Verifying the pagination expression
    fn validate_pagination_expression_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
        clause_name: &str,
    ) -> Result<(), ValidationError> {
        if !is_evaluable(expression) {
            return Err(ValidationError::new(
                format!("The {} expression must be a constant expression that can be computed immediately.", clause_name),
                ValidationErrorType::PaginationError,
            ));
        }

        match expression {
            crate::core::types::expr::Expression::Literal(crate::core::Value::Int(n)) => {
                if *n >= 0 {
                    Ok(())
                } else {
                    Err(ValidationError::new(
                        format!(
                            "The {} expression must be a non-negative integer.",
                            clause_name
                        ),
                        ValidationErrorType::PaginationError,
                    ))
                }
            }
            crate::core::types::expr::Expression::Literal(_) => Err(ValidationError::new(
                format!(
                    "The {} expression must evaluate to an integer.",
                    clause_name
                ),
                ValidationErrorType::PaginationError,
            )),
            _ => {
                use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
                use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;

                let mut context = DefaultExpressionContext::new();
                match ExpressionEvaluator::evaluate(expression, &mut context) {
                    Ok(crate::core::Value::Int(n)) => {
                        if n >= 0 {
                            Ok(())
                        } else {
                            Err(ValidationError::new(
                                format!(
                                    "The {} expression must be a non-negative integer.",
                                    clause_name
                                ),
                                ValidationErrorType::PaginationError,
                            ))
                        }
                    }
                    Ok(_) => Err(ValidationError::new(
                        format!(
                            "The {} expression must evaluate to an integer.",
                            clause_name
                        ),
                        ValidationErrorType::PaginationError,
                    )),
                    Err(e) => Err(ValidationError::new(
                        format!("Failed to evaluate {} expression: {}", clause_name, e),
                        ValidationErrorType::PaginationError,
                    )),
                }
            }
        }
    }

    /// Verify the range of step numbers
    pub fn validate_step_range(&self, range: &MatchStepRange) -> Result<(), ValidationError> {
        if range.min > range.max {
            return Err(ValidationError::new(
                format!(
                    "The maximum number of hops must be greater than or equal to the minimum number of hops: {} vs {}",
                    range.max, range.min
                ),
                ValidationErrorType::PaginationError,
            ));
        }
        Ok(())
    }

    /// Verify the sorting clause
    pub fn validate_order_by(
        &self,
        _factors: &[ContextualExpression], // Sorting factor
        yield_columns: &[YieldColumn],
        context: &OrderByClauseContext,
    ) -> Result<(), ValidationError> {
        // Verify theOrderBy clause
        for &(index, _) in &context.indexed_order_factors {
            if index >= yield_columns.len() {
                return Err(ValidationError::new(
                    format!("Column index {} out of range", index),
                    ValidationErrorType::PaginationError,
                ));
            }
        }

        Ok(())
    }
}

impl PaginationValidationStrategy {
    /// Obtain the policy name
    pub fn strategy_name(&self) -> &'static str {
        "PaginationValidationStrategy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::{ContextualExpression, ExpressionMeta};
    use crate::core::Expression;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    /// Create a ContextualExpression from an Expression.
    fn create_contextual_expression(expr: Expression) -> ContextualExpression {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        ContextualExpression::new(id, expr_ctx)
    }

    #[test]
    fn test_pagination_validation_strategy_creation() {
        let strategy = PaginationValidationStrategy::new();
        assert_eq!(strategy.strategy_name(), "PaginationValidationStrategy");
    }

    #[test]
    fn test_validate_pagination() {
        let strategy = PaginationValidationStrategy::new();

        // Testing valid pagination expressions
        let skip_expression =
            create_contextual_expression(Expression::Literal(crate::core::Value::Int(1)));
        let limit_expression =
            create_contextual_expression(Expression::Literal(crate::core::Value::Int(10)));
        let pagination_ctx = PaginationContext { skip: 0, limit: 10 };

        assert!(strategy
            .validate_pagination(
                Some(&skip_expression),
                Some(&limit_expression),
                &pagination_ctx
            )
            .is_ok());

        // Testing invalid pagination parameters
        let invalid_pagination_ctx = PaginationContext {
            skip: -1,
            limit: 10,
        };
        assert!(strategy
            .validate_pagination(None, None, &invalid_pagination_ctx)
            .is_err());

        let invalid_pagination_ctx2 = PaginationContext { skip: 0, limit: -5 };
        assert!(strategy
            .validate_pagination(None, None, &invalid_pagination_ctx2)
            .is_err());
    }

    #[test]
    fn test_validate_step_range() {
        let strategy = PaginationValidationStrategy::new();

        // Test the valid range (min <= max).
        let valid_range = MatchStepRange::new(1, 3);
        assert!(strategy.validate_step_range(&valid_range).is_ok());

        // The range for which the test is invalid (min > max)
        let invalid_range = MatchStepRange::new(3, 1);
        assert!(strategy.validate_step_range(&invalid_range).is_err());
    }

    #[test]
    fn test_validate_order_by() {
        let strategy = PaginationValidationStrategy::new();

        // Create test data
        let yield_columns = vec![
            YieldColumn::new(
                create_contextual_expression(Expression::Literal(crate::core::Value::Int(1))),
                "col1".to_string(),
            ),
            YieldColumn::new(
                create_contextual_expression(Expression::Literal(crate::core::Value::Int(2))),
                "col2".to_string(),
            ),
        ];

        let valid_context = OrderByClauseContext {
            indexed_order_factors: vec![
                (0, crate::core::types::OrderDirection::Asc),
                (1, crate::core::types::OrderDirection::Desc),
            ],
        };

        assert!(strategy
            .validate_order_by(&[], &yield_columns, &valid_context)
            .is_ok());

        // Testing invalid indexes
        let invalid_context = OrderByClauseContext {
            indexed_order_factors: vec![(5, crate::core::types::OrderDirection::Asc)], // The index is out of range.
        };

        assert!(strategy
            .validate_order_by(&[], &yield_columns, &invalid_context)
            .is_err());
    }

    #[test]
    fn test_pagination_expr_validation() {
        let strategy = PaginationValidationStrategy::new();

        // Test valid integer expressions.
        let int_expression =
            create_contextual_expression(Expression::Literal(crate::core::Value::Int(10)));
        assert!(strategy
            .validate_pagination_expression(&int_expression, "LIMIT")
            .is_ok());

        let string_expression = create_contextual_expression(Expression::Literal(
            crate::core::Value::String("invalid".to_string()),
        ));
        assert!(strategy
            .validate_pagination_expression(&string_expression, "LIMIT")
            .is_err());
    }

    #[test]
    fn test_edge_cases() {
        let strategy = PaginationValidationStrategy::new();

        // Testing boundary cases
        let zero_pagination = PaginationContext { skip: 0, limit: 0 };
        assert!(strategy
            .validate_pagination(None, None, &zero_pagination)
            .is_ok());

        let large_pagination = PaginationContext {
            skip: 1000000,
            limit: 1000000,
        };
        assert!(strategy
            .validate_pagination(None, None, &large_pagination)
            .is_ok());
    }
}

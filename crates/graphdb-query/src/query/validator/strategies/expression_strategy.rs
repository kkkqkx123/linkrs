use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::DataType;
use crate::core::YieldColumn;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::{
    MatchClauseContext, ReturnClauseContext, UnwindClauseContext, WhereClauseContext,
    WithClauseContext, YieldClauseContext,
};

use super::expression_operations::ExpressionOperationsValidator;
use super::helpers::TypeValidator;
use super::helpers::VariableChecker;

/// Expression validation strategy
pub struct ExpressionValidationStrategy;

impl Default for ExpressionValidationStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpressionValidationStrategy {
    pub fn new() -> Self {
        Self
    }

    /// Verify the filtering criteria
    pub fn validate_filter(
        &self,
        filter: &ContextualExpression,
        context: &WhereClauseContext,
    ) -> Result<(), ValidationError> {
        // Get the Expression from ContextualExpression
        let expr_meta = match filter.expression() {
            Some(e) => e,
            None => return Ok(()),
        };
        let expr = expr_meta.inner();

        // The filter criteria must be of the boolean type or convertible to the boolean type.
        let type_validator = TypeValidator;
        let filter_type = type_validator.deduce_expression_type_full(expr, context);

        if !type_validator.are_types_compatible(&filter_type, &DataType::Bool) {
            return Err(ValidationError::new(
                format!(
                    "The filter condition must be of type Boolean, the current type is {:?}",
                    filter_type
                ),
                ValidationErrorType::TypeError,
            ));
        }

        // Verify the variable references in the expression.
        let var_validator = VariableChecker::new();
        var_validator.validate_expression_variables(filter, &context.aliases_available)?;

        // Verify the operation of the expression.
        let expr_validator = ExpressionOperationsValidator::new();
        expr_validator.validate_expression_operations(filter)?;

        Ok(())
    }

    /// Verify the Match path.
    pub fn validate_path(
        &self,
        path: &ContextualExpression,
        context: &MatchClauseContext,
    ) -> Result<(), ValidationError> {
        // Retrieve the Expression from ContextualExpression.
        let expr_meta = match path.expression() {
            Some(e) => e,
            None => return Ok(()),
        };
        let expr = expr_meta.inner();

        // Verify the type of the path expression.
        let type_validator = TypeValidator;
        let path_type = type_validator.deduce_expression_type_full(expr, context);

        // Path expressions should either be of the path type itself or be convertible into the path type.
        if !matches!(path_type, DataType::Path) && !matches!(path_type, DataType::Empty) {
            return Err(ValidationError::new(
                format!(
                    "Path expression type mismatch, expected path type, actually {:?}",
                    path_type
                ),
                ValidationErrorType::TypeError,
            ));
        }

        // Verify the variable references in the path.
        let var_validator = VariableChecker::new();
        var_validator.validate_expression_variables(path, &context.aliases_available)?;

        Ok(())
    }

    /// Verify the Return statement
    pub fn validate_return(
        &self,
        return_expression: &ContextualExpression,
        return_items: &[YieldColumn],
        context: &ReturnClauseContext,
    ) -> Result<(), ValidationError> {
        // Obtain the Expression from ContextualExpression.
        let expr_meta = match return_expression.expression() {
            Some(e) => e,
            None => return Ok(()),
        };
        let expr = expr_meta.inner();

        // Verify the type of the Return expression.
        let type_validator = TypeValidator;
        let _return_type = type_validator.deduce_expression_type_full(expr, context);

        // Check the use of aggregate functions in the Return item.
        for item in return_items {
            let item_expr_meta = match item.expression.expression() {
                Some(e) => e,
                None => continue,
            };
            let item_expr = item_expr_meta.inner();
            if type_validator.has_aggregate_expression_internal(item_expr) {
                // Verify whether the use of the aggregate functions is in line with the context.
                if !context.yield_clause.has_agg && context.yield_clause.group_keys.is_empty() {
                    return Err(ValidationError::new(
                        "When using an aggregate function in a GROUP BY clause, you must specify the GROUP BY key".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        // Verify the variable references in the expression.
        let var_validator = VariableChecker::new();
        var_validator
            .validate_expression_variables(return_expression, &context.aliases_available)?;

        Ok(())
    }

    /// Verify the “With” clause
    pub fn validate_with(
        &self,
        with_expression: &ContextualExpression,
        with_items: &[YieldColumn],
        context: &WithClauseContext,
    ) -> Result<(), ValidationError> {
        // The validation logic for the With clause is similar to that of the Return clause.
        let return_context = ReturnClauseContext {
            yield_clause: context.yield_clause.clone(),
            aliases_available: context.aliases_available.clone(),
            aliases_generated: context.aliases_generated.clone(),
            pagination: context.pagination.clone(),
            order_by: context.order_by.clone(),
            distinct: context.distinct,
            query_parts: context.query_parts.clone(),
            errors: context.errors.clone(),
        };
        self.validate_return(with_expression, with_items, &return_context)
    }

    /// Verify the Unwind clause
    pub fn validate_unwind(
        &self,
        unwind_expression: &ContextualExpression,
        context: &UnwindClauseContext,
    ) -> Result<(), ValidationError> {
        // Obtain the Expression from ContextualExpression.
        if let Some(expr) = unwind_expression.get_expression() {
            // The `Unwind` expression must be of list type or an iterable type.
            let type_validator = TypeValidator;
            let unwind_type = type_validator.deduce_expression_type_full(&expr, context);

            if unwind_type != DataType::List && unwind_type != DataType::Empty {
                return Err(ValidationError::new(
                    format!(
                        "Unwind expressions must be of list type, currently of type {:?}",
                        unwind_type
                    ),
                    ValidationErrorType::TypeError,
                ));
            }

            // Verify the variable references in the expression.
            let var_validator = VariableChecker::new();
            var_validator
                .validate_expression_variables(unwind_expression, &context.aliases_available)?;
        }

        Ok(())
    }

    /// Verify the Yield clause
    pub fn validate_yield(&self, context: &YieldClauseContext) -> Result<(), ValidationError> {
        // Verify each value in the Yield column.
        let type_validator = TypeValidator;
        let var_validator = VariableChecker::new();

        for column in &context.yield_columns {
            // Obtain the Expression from ContextualExpression.
            let expr_meta = match column.expression.expression() {
                Some(e) => e,
                None => continue,
            };
            let expr = expr_meta.inner();

            // Verify the type of the expression
            let _column_type = type_validator.deduce_expression_type_full(expr, context);

            // Verify the use of aggregate functions
            if type_validator.has_aggregate_expression_internal(expr)
                && !context.has_agg
                && context.group_keys.is_empty()
            {
                return Err(ValidationError::new(
                    "When using an aggregate function in a GROUP BY clause, you must specify the GROUP BY key".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }

            // Verify the variable references in the expression.
            var_validator
                .validate_expression_variables(&column.expression, &context.aliases_available)?;
        }

        // Verify the group key
        for group_key in &context.group_keys {
            let expr_meta = match group_key.expression() {
                Some(e) => e,
                None => continue,
            };
            let expr = expr_meta.inner();
            type_validator.validate_group_key_type(expr, context)?;
        }

        Ok(())
    }

    /// Verify a single path pattern
    pub fn validate_single_path_pattern(
        &self,
        pattern: &ContextualExpression,
        context: &mut MatchClauseContext,
    ) -> Result<(), ValidationError> {
        // Obtain the Expression from ContextualExpression.
        let expr_meta = match pattern.expression() {
            Some(e) => e,
            None => return Ok(()),
        };
        let expr = expr_meta.inner();

        // Verify the type of the path pattern.
        let type_validator = TypeValidator;
        let pattern_type = type_validator.deduce_expression_type_full(expr, context);

        if !matches!(pattern_type, DataType::Path) && !matches!(pattern_type, DataType::Empty) {
            return Err(ValidationError::new(
                format!(
                    "The path mode must be a path type, currently of type {:?}",
                    pattern_type
                ),
                ValidationErrorType::TypeError,
            ));
        }

        // Verify the variable references in the path pattern.
        let var_validator = VariableChecker::new();
        var_validator.validate_expression_variables(pattern, &context.aliases_available)?;

        Ok(())
    }
}

impl ExpressionValidationStrategy {
    /// Obtain the policy name
    pub fn strategy_name(&self) -> &'static str {
        "ExpressionValidationStrategy"
    }
}

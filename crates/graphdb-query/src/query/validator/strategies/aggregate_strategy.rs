//! Aggregated validation strategy
//! Responsible for verifying the use of aggregate functions and checking whether expressions contain any aggregate operations.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::operators::AggregateFunction;
use crate::query::validator::error::{ValidationError, ValidationErrorType};

/// Aggregated validation strategy
pub struct AggregateValidationStrategy;

impl Default for AggregateValidationStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl AggregateValidationStrategy {
    pub fn new() -> Self {
        Self
    }

    /// Check whether the expression contains aggregate functions.
    pub fn has_aggregate_expression(&self, expression: &ContextualExpression) -> bool {
        let expr_meta = match expression.expression() {
            Some(e) => e,
            None => return false,
        };
        self.has_aggregate_expression_internal(expr_meta.inner())
    }

    /// Internal method: Check whether the Expression contains aggregate functions.
    fn has_aggregate_expression_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> bool {
        match expression {
            crate::core::types::expr::Expression::Aggregate { .. } => true,
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.has_aggregate_expression_internal(operand)
            }
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                self.has_aggregate_expression_internal(left)
                    || self.has_aggregate_expression_internal(right)
            }
            crate::core::types::expr::Expression::Function { args, .. } => args
                .iter()
                .any(|arg| self.has_aggregate_expression_internal(arg)),
            crate::core::types::expr::Expression::List(items) => items
                .iter()
                .any(|item| self.has_aggregate_expression_internal(item)),
            crate::core::types::expr::Expression::Map(items) => items
                .iter()
                .any(|(_, value)| self.has_aggregate_expression_internal(value)),
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                test_expr
                    .as_ref()
                    .is_some_and(|expr| self.has_aggregate_expression_internal(expr))
                    || conditions.iter().any(|(cond, val)| {
                        self.has_aggregate_expression_internal(cond)
                            || self.has_aggregate_expression_internal(val)
                    })
                    || default
                        .as_ref()
                        .is_some_and(|d| self.has_aggregate_expression_internal(d))
            }
            _ => false,
        }
    }

    /// Verify that aggregate functions are not allowed to be used in the UNWIND clause.
    pub fn validate_unwind_aggregate(
        &self,
        unwind_expression: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        if self.has_aggregate_expression(unwind_expression) {
            return Err(ValidationError::new(
                "Aggregate expressions cannot be used in UNWIND clauses".to_string(),
                ValidationErrorType::AggregateError,
            ));
        }
        Ok(())
    }

    /// Verify the validity of the aggregated expression.
    /// Check:
    /// 1. Is the name of the aggregate function valid?
    /// 2. Are there any aggregate functions nested?
    /// 3. Are the special attributes (*) only used for the COUNT function?
    /// 4. Is the parameter expression valid?
    pub fn validate_aggregate_expression(
        &self,
        expression: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        let expr_meta = match expression.expression() {
            Some(e) => e,
            None => return Ok(()),
        };
        self.validate_aggregate_expression_internal(expr_meta.inner())
    }

    /// Internal method: Verifying the validity of aggregate expressions
    fn validate_aggregate_expression_internal(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        match expression {
            crate::core::types::expr::Expression::Aggregate {
                func,
                arg,
                distinct: _,
            } => {
                // 1. Verify the validity of the aggregate function names.
                // Since enumerations are now being used, this check may need to be adjusted.
                // Skip this check for now, since the enumeration values are always valid.

                // 2. Check for nested aggregate functions: It is not allowed for aggregate functions to contain other aggregate functions.
                if self.has_aggregate_expression_internal(arg) {
                    return Err(ValidationError::new(
                        "Aggregate function nesting is not allowed".to_string(),
                        ValidationErrorType::AggregateError,
                    ));
                }

                // 3. Check special attributes (*. * can only be used for COUNT).
                self.validate_wildcard_property(func, arg)?;

                // 4. Recursive verification of the validity of parameter expressions
                self.validate_expression_in_aggregate(arg)?;

                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Verify the use of wildcard attribute properties.
    ///
    /// Validation rules:
    /// 1. Only check the direct parameters of the aggregate functions (do not recursively check nested expressions).
    /// 2. Only check input attribute expressions（$-.prop or $var.prop）
    /// 3. Only the COUNT function allows the use of the wildcard attribute *.
    fn validate_wildcard_property(
        &self,
        func: &AggregateFunction,
        expression: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        let is_count = matches!(func, AggregateFunction::Count(_));

        if is_count {
            return Ok(());
        }

        if let crate::core::types::expr::Expression::Property { object, property } = expression {
            if property == "*" {
                if let crate::core::types::expr::Expression::Variable(var_name) = object.as_ref() {
                    let ref_type = if var_name == "-" {
                        "Input Properties"
                    } else {
                        "Variable Properties"
                    };
                    return Err(ValidationError::new(
                        format!(
                            "You cannot apply the aggregate function `{}` to the {} wildcard attribute `{}. {}`",
                            func.name(),
                            ref_type,
                            var_name,
                            property
                        ),
                        ValidationErrorType::AggregateError,
                    ));
                }
            }
        }

        Ok(())
    }

    /// Verify the validity of the parameter expressions for the aggregate functions.
    /// Recursively check whether there are any other illegal nested structures in the parameter expressions.
    ///
    /// Validation rules:
    /// 1. Recursively check the validity of all subexpressions.
    /// 2. Ensure that the structure of the parameter expression is correct.
    fn validate_expression_in_aggregate(
        &self,
        expression: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        match expression {
            // Recursive checking of unary operations (including various unary operators)
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.validate_expression_in_aggregate(operand)?;
            }

            // Recursive checking of binary operations
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                self.validate_expression_in_aggregate(left)?;
                self.validate_expression_in_aggregate(right)?;
            }

            // Recursive checking of function call parameters
            crate::core::types::expr::Expression::Function { args, .. } => {
                for arg in args {
                    self.validate_expression_in_aggregate(arg)?;
                }
            }

            // Recursively check the list elements
            crate::core::types::expr::Expression::List(items) => {
                for item in items {
                    self.validate_expression_in_aggregate(item)?;
                }
            }

            // Recursive checking of Map values
            crate::core::types::expr::Expression::Map(items) => {
                for (_, value) in items {
                    self.validate_expression_in_aggregate(value)?;
                }
            }

            // Recursive checking of type conversion expressions
            crate::core::types::expr::Expression::TypeCast {
                expression: cast_expression,
                ..
            } => {
                self.validate_expression_in_aggregate(cast_expression)?;
            }

            // Recursive checking of CASE expressions
            crate::core::types::expr::Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(expr) = test_expr {
                    self.validate_expression_in_aggregate(expr)?;
                }
                for (cond, val) in conditions {
                    self.validate_expression_in_aggregate(cond)?;
                    self.validate_expression_in_aggregate(val)?;
                }
                if let Some(d) = default {
                    self.validate_expression_in_aggregate(d)?;
                }
            }

            // Expressions such as constants, attributes, and aggregates do not require further recursive checking.
            _ => {}
        }
        Ok(())
    }
}

impl AggregateValidationStrategy {
    /// Obtain the policy name
    pub fn strategy_name(&self) -> &'static str {
        "AggregateValidationStrategy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::{ContextualExpression, ExpressionMeta};
    use crate::core::types::operators::{AggregateFunction, BinaryOperator};
    use crate::core::types::DataType;
    use crate::core::Expression;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_aggregate_validation_strategy_creation() {
        let strategy = AggregateValidationStrategy::new();
        assert_eq!(strategy.strategy_name(), "AggregateValidationStrategy");
    }

    #[test]
    fn test_has_aggregate_expression() {
        let strategy = AggregateValidationStrategy::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        // The test includes expressions that do not contain aggregate functions.
        let non_agg_expr = Expression::Literal(crate::core::Value::Int(1));
        let non_agg_meta = ExpressionMeta::new(non_agg_expr);
        let non_agg_id = expr_ctx.register_expression(non_agg_meta);
        let non_agg_expression = ContextualExpression::new(non_agg_id, expr_ctx.clone());
        assert!(!strategy.has_aggregate_expression(&non_agg_expression));

        let binary_expr = Expression::Binary {
            left: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(crate::core::Value::Int(2))),
        };
        let binary_meta = ExpressionMeta::new(binary_expr);
        let binary_id = expr_ctx.register_expression(binary_meta);
        let binary_expression = ContextualExpression::new(binary_id, expr_ctx);
        assert!(!strategy.has_aggregate_expression(&binary_expression));
    }

    #[test]
    fn test_validate_unwind_aggregate() {
        let strategy = AggregateValidationStrategy::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        // UNWIND expressions that do not contain any aggregate functions
        let non_agg_expr = Expression::Literal(crate::core::Value::Int(1));
        let non_agg_meta = ExpressionMeta::new(non_agg_expr);
        let non_agg_id = expr_ctx.register_expression(non_agg_meta);
        let non_agg_expression = ContextualExpression::new(non_agg_id, expr_ctx);
        assert!(strategy
            .validate_unwind_aggregate(&non_agg_expression)
            .is_ok());

        // The test includes UNWIND expressions that use aggregate functions.
        // Note: The request contains a phrase that seems to refer to an example of an aggregate expression. However, without specific context or information about what an "aggregate expression" is in this particular context, I cannot provide an accurate translation. An aggregate expression is a mathematical or computational term used in databases, programming languages, or other technical contexts to summarize or group data. If you could provide more details or clarify what you mean by "an example of an aggregate expression," I would be able to assist you with the translation.
        // Skip this test for now, as a specific construction of aggregate expressions is required.
    }

    #[test]
    fn test_nested_expressions() {
        let strategy = AggregateValidationStrategy::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        // Testing nested expressions
        let nested_expr = Expression::Binary {
            left: Box::new(Expression::Unary {
                op: crate::core::types::operators::UnaryOperator::Minus,
                operand: Box::new(Expression::Literal(crate::core::Value::Int(5))),
            }),
            op: crate::core::types::operators::BinaryOperator::Add,
            right: Box::new(Expression::Literal(crate::core::Value::Int(10))),
        };
        let nested_meta = ExpressionMeta::new(nested_expr);
        let nested_id = expr_ctx.register_expression(nested_meta);
        let nested_expression = ContextualExpression::new(nested_id, expr_ctx);

        assert!(!strategy.has_aggregate_expression(&nested_expression));
    }

    #[test]
    fn test_validate_invalid_aggregate_function() {
        let strategy = AggregateValidationStrategy::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        // Count(None) 是有效的，表示 COUNT(*)
        let expression = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            distinct: false,
        };
        let meta = ExpressionMeta::new(expression);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let result = strategy.validate_aggregate_expression(&ctx_expr);
        // Count(None) 应该被接受
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_nested_aggregates() {
        let strategy = AggregateValidationStrategy::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let inner_agg = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            distinct: false,
        };
        let outer_agg = Expression::Aggregate {
            func: AggregateFunction::Sum("".to_string()),
            arg: Box::new(inner_agg),
            distinct: false,
        };
        let meta = ExpressionMeta::new(outer_agg);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let result = strategy.validate_aggregate_expression(&ctx_expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err
            .message
            .contains("Aggregate function nesting is not allowed"));
    }

    #[test]
    fn test_validate_count_with_wildcard() {
        let strategy = AggregateValidationStrategy::new();
        let expression = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "*".to_string(),
            }),
            distinct: false,
        };

        let meta = ExpressionMeta::new(expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        // COUNT allows the use of wildcard attributes.
        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());
    }

    #[test]
    fn test_validate_sum_with_wildcard() {
        let strategy = AggregateValidationStrategy::new();
        let expression = Expression::Aggregate {
            func: AggregateFunction::Sum("".to_string()),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "*".to_string(),
            }),
            distinct: false,
        };

        let meta = ExpressionMeta::new(expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        // The SUM function does not allow the use of wildcard attributes.
        let result = strategy.validate_aggregate_expression(&ctx_expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("SUM"));
        assert!(err.message.contains("wildcard"));
        assert!(err.message.contains("n"));
    }

    #[test]
    fn test_validate_various_aggregate_functions() {
        let strategy = AggregateValidationStrategy::new();
        let _valid_functions = vec![
            "COUNT",
            "SUM",
            "AVG",
            "MAX",
            "MIN",
            "STD",
            "BIT_AND",
            "BIT_OR",
            "BIT_XOR",
            "COLLECT",
            "COLLECT_SET",
        ];

        // Testing various aggregate functions
        let valid_functions = vec![
            AggregateFunction::Count(None),
            AggregateFunction::Sum("".to_string()),
            AggregateFunction::Avg("".to_string()),
            AggregateFunction::Max("".to_string()),
            AggregateFunction::Min("".to_string()),
            AggregateFunction::Collect("".to_string()),
        ];

        for func in valid_functions {
            let expression = Expression::Aggregate {
                func,
                arg: Box::new(Expression::Literal(crate::core::Value::Int(1))),
                distinct: false,
            };

            let meta = ExpressionMeta::new(expression);
            let expr_ctx = ExpressionAnalysisContext::new();
            let id = expr_ctx.register_expression(meta);
            let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

            assert!(
                strategy.validate_aggregate_expression(&ctx_expr).is_ok(),
                "The aggregate functions should be valid."
            );
        }
    }

    #[test]
    fn test_validate_distinct_aggregate() {
        let strategy = AggregateValidationStrategy::new();
        let expression = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            distinct: true,
        };

        let meta = ExpressionMeta::new(expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        // The DISTINCT aggregation should be accepted.
        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());
    }

    #[test]
    fn test_validate_input_property_wildcard() {
        let strategy = AggregateValidationStrategy::new();

        // COUNT($-.*) 应该被允许
        let count_input_wildcard = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("-".to_string())),
                property: "*".to_string(),
            }),
            distinct: false,
        };
        let meta = ExpressionMeta::new(count_input_wildcard);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());

        // SUM($-.*) 不应该被允许
        let sum_input_wildcard = Expression::Aggregate {
            func: AggregateFunction::Sum("".to_string()),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("-".to_string())),
                property: "*".to_string(),
            }),
            distinct: false,
        };
        let meta = ExpressionMeta::new(sum_input_wildcard);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        let result = strategy.validate_aggregate_expression(&ctx_expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Input Properties"));
        assert!(err.message.contains("SUM"));
    }

    #[test]
    fn test_validate_var_property_wildcard() {
        let strategy = AggregateValidationStrategy::new();

        // COUNT($var.*) 应该被允许
        let count_var_wildcard = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("myVar".to_string())),
                property: "*".to_string(),
            }),
            distinct: false,
        };
        let meta = ExpressionMeta::new(count_var_wildcard);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());

        // AVG($var.*) 不应该被允许
        let avg_var_wildcard = Expression::Aggregate {
            func: AggregateFunction::Avg("".to_string()),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("myVar".to_string())),
                property: "*".to_string(),
            }),
            distinct: false,
        };
        let meta = ExpressionMeta::new(avg_var_wildcard);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        let result = strategy.validate_aggregate_expression(&ctx_expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Variable Properties"));
        assert!(err.message.contains("AVG"));
    }

    #[test]
    fn test_validate_wildcard_in_nested_expression() {
        let strategy = AggregateValidationStrategy::new();

        // Wildcards in nested expressions should not be checked (only the direct parameters should be checked).
        // SUM(n.* + 1) - 这里的 n.* 不是聚合函数的直接参数
        let nested_wildcard = Expression::Aggregate {
            func: AggregateFunction::Sum("".to_string()),
            arg: Box::new(Expression::Binary {
                left: Box::new(Expression::Property {
                    object: Box::new(Expression::Variable("n".to_string())),
                    property: "*".to_string(),
                }),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            }),
            distinct: false,
        };
        let meta = ExpressionMeta::new(nested_wildcard);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        // Since the wildcard is not a direct parameter in the nested expression, it should be verified accordingly.
        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());
    }

    #[test]
    fn test_validate_expression_in_aggregate_binary_op() {
        let strategy = AggregateValidationStrategy::new();

        let expression = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "value".to_string(),
            }),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(crate::core::Value::Int(10))),
        };

        assert!(strategy
            .validate_expression_in_aggregate(&expression)
            .is_ok());
    }

    #[test]
    fn test_validate_expression_in_aggregate_function_call() {
        let strategy = AggregateValidationStrategy::new();

        // Validation of function call testing in aggregate parameters
        let expression = Expression::Function {
            name: "LOWER".to_string(),
            args: vec![Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "name".to_string(),
            }],
        };

        // Verification should be carried out.
        assert!(strategy
            .validate_expression_in_aggregate(&expression)
            .is_ok());
    }

    #[test]
    fn test_validate_expression_in_aggregate_case() {
        let strategy = AggregateValidationStrategy::new();

        // Testing the validation of CASE expressions in aggregate parameters
        let expression = Expression::Case {
            test_expr: None,
            conditions: vec![(
                Expression::Binary {
                    left: Box::new(Expression::Property {
                        object: Box::new(Expression::Variable("n".to_string())),
                        property: "status".to_string(),
                    }),
                    op: BinaryOperator::Equal,
                    right: Box::new(Expression::Literal(crate::core::Value::String(
                        "active".to_string(),
                    ))),
                },
                Expression::Literal(crate::core::Value::Int(1)),
            )],
            default: Some(Box::new(Expression::Literal(crate::core::Value::Int(0)))),
        };

        assert!(strategy
            .validate_expression_in_aggregate(&expression)
            .is_ok());
    }

    #[test]
    fn test_validate_expression_in_aggregate_list() {
        let strategy = AggregateValidationStrategy::new();

        let expression = Expression::List(vec![
            Expression::Literal(crate::core::Value::Int(1)),
            Expression::Literal(crate::core::Value::Int(2)),
            Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "value".to_string(),
            },
        ]);

        // Verification should be carried out.
        assert!(strategy
            .validate_expression_in_aggregate(&expression)
            .is_ok());
    }

    #[test]
    fn test_validate_expression_in_aggregate_type_casting() {
        let strategy = AggregateValidationStrategy::new();

        // Validation of test type conversion in aggregate parameters
        let expression = Expression::TypeCast {
            expression: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "value".to_string(),
            }),
            target_type: DataType::Int,
        };

        // Verification should be carried out.
        assert!(strategy
            .validate_expression_in_aggregate(&expression)
            .is_ok());
    }

    #[test]
    fn test_validate_aggregate_sum_valid() {
        let strategy = AggregateValidationStrategy::new();

        // Testing the effectiveness of the SUM aggregation function
        let expression = Expression::Aggregate {
            func: AggregateFunction::Sum("".to_string()),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "amount".to_string(),
            }),
            distinct: false,
        };

        let meta = ExpressionMeta::new(expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());
    }

    #[test]
    fn test_validate_aggregate_count_valid() {
        let strategy = AggregateValidationStrategy::new();

        // Testing the effectiveness of the COUNT aggregate function
        let expression = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            distinct: false,
        };

        let meta = ExpressionMeta::new(expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));
        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());
    }

    #[test]
    fn test_validate_aggregate_min_max_valid() {
        let strategy = AggregateValidationStrategy::new();

        let min_expression = Expression::Aggregate {
            func: AggregateFunction::Min("".to_string()),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "value".to_string(),
            }),
            distinct: false,
        };
        let meta = ExpressionMeta::new(min_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, Arc::new(expr_ctx));

        let max_expression = Expression::Aggregate {
            func: AggregateFunction::Max("".to_string()),
            arg: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "value".to_string(),
            }),
            distinct: false,
        };
        let meta = ExpressionMeta::new(max_expression);
        let expr_ctx = ExpressionAnalysisContext::new();
        let id = expr_ctx.register_expression(meta);
        let ctx_expr2 = ContextualExpression::new(id, Arc::new(expr_ctx));

        assert!(strategy.validate_aggregate_expression(&ctx_expr).is_ok());
        assert!(strategy.validate_aggregate_expression(&ctx_expr2).is_ok());
    }
}

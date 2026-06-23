//! Expression tool functions
//!
//! Provide a specialized tool function for rewriting expressions
//!
//! # Design Notes
//!
//! The responsibility of the Rewrite layer is to rewrite expressions, which requires:
//! 1. Analyze the structure of the existing expression.
//! 2. Create a new expression.
//! 3. Register the new expression in the ExpressionContext.
//!
//! Therefore, the Rewrite layer needs to have access to the internal structure of the Expression.
//! This is a necessary trade-off in terms of design, because:
//! “ContextualExpression” is a lightweight reference that does not contain the structure of the expression itself.
//! The rewrite operation requires the creation of a new Expression.
//! The new Expression must be registered with the ExpressionContext in order to be used.
//!
//! # Note
//!
//! The general expression tools and functions (such as extract_property_refs, is_constant) have been moved to another location.
//! `core::types::expression::common_utils`，本模块仅保留重写专用的函数。

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::ExpressionMeta;
use crate::core::types::expr::PropertyContainsChecker;
use crate::core::types::operators::BinaryOperator;
use crate::core::Expression;
use crate::query::validator::context::ExpressionAnalysisContext;
use std::sync::Arc;

/// Check whether the expression contains the specified attribute name.
///
/// # Parameters
/// `property_names`: List of property names
/// `expr`: The expression to be checked.
///
/// # Return
/// If the expression contains any of the attributes from the list of attribute names, return true.
pub fn check_col_name(property_names: &[String], expr: &Expression) -> bool {
    PropertyContainsChecker::check(expr, property_names)
}

/// Rewrite the contextual expression
///
/// Rewrite the `ContextualExpression` according to the `rewrite_map`, and register the result in the `ExpressionContext`.
///
/// # Parameters
/// `expr`: The `ContextualExpression` that needs to be rewritten.
/// `rewrite_map`: A map for rewriting expressions; the keys represent variable names, and the values represent the `ContextualExpression` objects that need to be replaced.
/// `expr_context`: The context in which the expression is used, used for registering new expressions.
///
/// # Back
/// Revised ContextualExpression
pub fn rewrite_contextual_expression(
    expr: &ContextualExpression,
    rewrite_map: &std::collections::HashMap<String, ContextualExpression>,
    expr_context: Arc<ExpressionAnalysisContext>,
) -> ContextualExpression {
    let expr_meta = match expr.expression() {
        Some(e) => e,
        None => return expr.clone(),
    };
    let inner_expr = expr_meta.inner();

    let rewritten_expr = rewrite_expression_with_map(inner_expr, rewrite_map, expr_context.clone());
    let meta = ExpressionMeta::new(rewritten_expr);
    let id = expr_context.register_expression(meta);
    ContextualExpression::new(id, expr_context)
}

/// Rewrite the expression using the ContextualExpression mapping table.
///
/// # Parameters
/// `expr`: The Expression that needs to be rewritten.
/// - `rewrite_map`: rewrite mapping table, key is variable name, value is ContextualExpression to be replaced
/// - `expr_context`: expression context, used to register new expressions
///
/// # Back
/// The rewritten expression
fn rewrite_expression_with_map(
    expr: &Expression,
    rewrite_map: &std::collections::HashMap<String, ContextualExpression>,
    expr_context: Arc<ExpressionAnalysisContext>,
) -> Expression {
    match expr {
        Expression::Variable(name) => {
            if let Some(new_ctx_expr) = rewrite_map.get(name) {
                let new_expr_meta = match new_ctx_expr.expression() {
                    Some(e) => e,
                    None => return expr.clone(),
                };
                new_expr_meta.inner().clone()
            } else {
                expr.clone()
            }
        }
        Expression::Property { object, property } => {
            if let Expression::Variable(obj_name) = object.as_ref() {
                let full_name = format!("{}.{}", obj_name, property);
                if let Some(new_ctx_expr) = rewrite_map.get(&full_name) {
                    let new_expr_meta = match new_ctx_expr.expression() {
                        Some(e) => e,
                        None => return expr.clone(),
                    };
                    return new_expr_meta.inner().clone();
                }
                if let Some(new_ctx_expr) = rewrite_map.get(property) {
                    let new_expr_meta = match new_ctx_expr.expression() {
                        Some(e) => e,
                        None => return expr.clone(),
                    };
                    return Expression::Property {
                        object: Box::new(new_expr_meta.inner().clone()),
                        property: property.clone(),
                    };
                }
            }
            Expression::Property {
                object: Box::new(rewrite_expression_with_map(
                    object,
                    rewrite_map,
                    expr_context,
                )),
                property: property.clone(),
            }
        }
        Expression::Binary { left, op, right } => Expression::Binary {
            left: Box::new(rewrite_expression_with_map(
                left,
                rewrite_map,
                expr_context.clone(),
            )),
            op: *op,
            right: Box::new(rewrite_expression_with_map(
                right,
                rewrite_map,
                expr_context,
            )),
        },
        Expression::Unary { op, operand } => Expression::Unary {
            op: *op,
            operand: Box::new(rewrite_expression_with_map(
                operand,
                rewrite_map,
                expr_context,
            )),
        },
        Expression::Function { name, args } => Expression::Function {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| rewrite_expression_with_map(arg, rewrite_map, expr_context.clone()))
                .collect(),
        },
        Expression::Aggregate {
            func,
            arg,
            distinct,
        } => Expression::Aggregate {
            func: func.clone(),
            arg: Box::new(rewrite_expression_with_map(arg, rewrite_map, expr_context)),
            distinct: *distinct,
        },
        Expression::List(list) => Expression::List(
            list.iter()
                .map(|item| rewrite_expression_with_map(item, rewrite_map, expr_context.clone()))
                .collect(),
        ),
        Expression::Map(map) => Expression::Map(
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        rewrite_expression_with_map(v, rewrite_map, expr_context.clone()),
                    )
                })
                .collect(),
        ),
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => Expression::Case {
            test_expr: test_expr.as_ref().map(|e| {
                Box::new(rewrite_expression_with_map(
                    e,
                    rewrite_map,
                    expr_context.clone(),
                ))
            }),
            conditions: conditions
                .iter()
                .map(|(w, t)| {
                    (
                        rewrite_expression_with_map(w, rewrite_map, expr_context.clone()),
                        rewrite_expression_with_map(t, rewrite_map, expr_context.clone()),
                    )
                })
                .collect(),
            default: default
                .as_ref()
                .map(|e| Box::new(rewrite_expression_with_map(e, rewrite_map, expr_context))),
        },
        Expression::TypeCast {
            expression,
            target_type,
        } => Expression::TypeCast {
            expression: Box::new(rewrite_expression_with_map(
                expression,
                rewrite_map,
                expr_context,
            )),
            target_type: target_type.clone(),
        },
        Expression::Subscript { collection, index } => Expression::Subscript {
            collection: Box::new(rewrite_expression_with_map(
                collection,
                rewrite_map,
                expr_context.clone(),
            )),
            index: Box::new(rewrite_expression_with_map(
                index,
                rewrite_map,
                expr_context,
            )),
        },
        _ => expr.clone(),
    }
}

/// Split filter criteria
///
/// Split complex filter conditions (such as those connected by the AND operator) into two parts:
/// The part that corresponds to the selector function
/// The remaining part
///
/// # Parameters
/// `ctx_expr`: The context expression for the filtering criteria
/// `picker`: A selector function that returns `true` if the corresponding element should be selected.
///
/// # Back
/// (The selected part; the remaining part)
pub fn split_filter<F>(
    ctx_expr: &ContextualExpression,
    picker: F,
) -> (Option<ContextualExpression>, Option<ContextualExpression>)
where
    F: Fn(&Expression) -> bool,
{
    let expr_meta = match ctx_expr.expression() {
        Some(e) => e,
        None => return (None, None),
    };
    let expr = expr_meta.inner();
    let (picked_expr, remained_expr) = split_filter_impl(expr, &picker);

    let expr_context = ctx_expr.context().clone();
    let picked = picked_expr.map(|e| {
        let meta = ExpressionMeta::new(e);
        let id = expr_context.register_expression(meta);
        ContextualExpression::new(id, expr_context.clone())
    });

    let remained = remained_expr.map(|e| {
        let meta = ExpressionMeta::new(e);
        let id = expr_context.register_expression(meta);
        ContextualExpression::new(id, expr_context.clone())
    });

    (picked, remained)
}

fn split_filter_impl<F>(
    condition: &Expression,
    picker: &F,
) -> (Option<Expression>, Option<Expression>)
where
    F: Fn(&Expression) -> bool,
{
    match condition {
        Expression::Binary {
            op: BinaryOperator::And,
            left,
            right,
        } => {
            // Handle the left and right sides recursively.
            let (left_picked, left_remained) = split_filter_impl(left, picker);
            let (right_picked, right_remained) = split_filter_impl(right, picker);

            // Merge the selected sections.
            let picked = match (left_picked, right_picked) {
                (Some(l), Some(r)) => Some(Expression::Binary {
                    op: BinaryOperator::And,
                    left: Box::new(l),
                    right: Box::new(r),
                }),
                (Some(l), None) => Some(l),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            };

            // Merge the remaining parts.
            let remained = match (left_remained, right_remained) {
                (Some(l), Some(r)) => Some(Expression::Binary {
                    op: BinaryOperator::And,
                    left: Box::new(l),
                    right: Box::new(r),
                }),
                (Some(l), None) => Some(l),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            };

            (picked, remained)
        }
        _ => {
            // Basic situation: Check whether the current expression meets the criteria of the selector.
            if picker(condition) {
                (Some(condition.clone()), None)
            } else {
                (None, Some(condition.clone()))
            }
        }
    }
}

/// To combine two filter conditions, use the AND operator.
///
/// # Parameters
/// “left”: The left-side condition
/// “right”: The condition on the right side
///
/// # Back
/// The merged conditions
pub fn and_condition(
    left: Option<ContextualExpression>,
    right: Option<ContextualExpression>,
) -> Option<ContextualExpression> {
    match (left, right) {
        (Some(l), Some(r)) => {
            let expr_context = l.context().clone();
            let l_expr = match l.expression() {
                Some(e) => e,
                None => return Some(r),
            };
            let r_expr = match r.expression() {
                Some(e) => e,
                None => return Some(l),
            };
            let combined_expr = Expression::Binary {
                op: BinaryOperator::And,
                left: Box::new(l_expr.inner().clone()),
                right: Box::new(r_expr.inner().clone()),
            };
            let meta = ExpressionMeta::new(combined_expr);
            let id = expr_context.register_expression(meta);
            Some(ContextualExpression::new(id, expr_context))
        }
        (Some(l), None) => Some(l),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

/// To combine multiple filter conditions, use the AND operator.
///
/// # Parameters
/// `conditions`: List of conditions
///
/// # Back
/// Post-merger conditions
pub fn and_conditions(
    conditions: Vec<Option<ContextualExpression>>,
) -> Option<ContextualExpression> {
    conditions.into_iter().fold(None, and_condition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;

    #[test]
    fn test_check_col_name() {
        let property_names = vec!["a".to_string(), "b".to_string()];

        let expr = Expression::Property {
            object: Box::new(Expression::Variable("v".to_string())),
            property: "a".to_string(),
        };
        assert!(check_col_name(&property_names, &expr));

        let expr = Expression::Property {
            object: Box::new(Expression::Variable("v".to_string())),
            property: "c".to_string(),
        };
        assert!(!check_col_name(&property_names, &expr));

        let expr = Expression::Binary {
            op: BinaryOperator::Equal,
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("v".to_string())),
                property: "a".to_string(),
            }),
            right: Box::new(Expression::Literal(Value::Int(1))),
        };
        assert!(check_col_name(&property_names, &expr));
    }

    #[test]
    fn test_split_filter() {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let condition = Expression::Binary {
            op: BinaryOperator::And,
            left: Box::new(Expression::Binary {
                op: BinaryOperator::And,
                left: Box::new(Expression::Binary {
                    op: BinaryOperator::Equal,
                    left: Box::new(Expression::Property {
                        object: Box::new(Expression::Variable("v".to_string())),
                        property: "a".to_string(),
                    }),
                    right: Box::new(Expression::Literal(Value::Int(1))),
                }),
                right: Box::new(Expression::Binary {
                    op: BinaryOperator::Equal,
                    left: Box::new(Expression::Property {
                        object: Box::new(Expression::Variable("v".to_string())),
                        property: "b".to_string(),
                    }),
                    right: Box::new(Expression::Literal(Value::Int(2))),
                }),
            }),
            right: Box::new(Expression::Binary {
                op: BinaryOperator::Equal,
                left: Box::new(Expression::Property {
                    object: Box::new(Expression::Variable("v".to_string())),
                    property: "c".to_string(),
                }),
                right: Box::new(Expression::Literal(Value::Int(3))),
            }),
        };

        let meta = ExpressionMeta::new(condition);
        let id = expr_context.register_expression(meta);
        let ctx_condition = ContextualExpression::new(id, expr_context.clone());

        let picker = |expr: &Expression| -> bool {
            let mut collector =
                crate::core::types::expr::visitor_collectors::PropertyCollector::new();
            crate::core::types::expr::ExpressionVisitor::visit(&mut collector, expr);
            collector.properties.contains(&"a".to_string())
                || collector.properties.contains(&"b".to_string())
        };

        let (picked, remained) = split_filter(&ctx_condition, picker);

        assert!(picked.is_some());
        let picked_props = crate::core::types::expr::expression_utils::extract_property_refs(
            picked.as_ref().expect("Failed to get picked expression"),
        );
        assert!(picked_props.contains(&"a".to_string()));
        assert!(picked_props.contains(&"b".to_string()));

        assert!(remained.is_some());
        let remained_props = crate::core::types::expr::expression_utils::extract_property_refs(
            remained
                .as_ref()
                .expect("Failed to get remained expression"),
        );
        assert!(remained_props.contains(&"c".to_string()));
    }
}

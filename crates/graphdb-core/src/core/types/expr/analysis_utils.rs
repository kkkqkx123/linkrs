//! Expression Analysis Utility Functions
//!
//! Provides functions for analyzing expressions, including:
//! - Constant expression checking
//! - Variable collection
//! - Aggregate function detection
//! - Runtime context requirement checking
//! - Expression searching

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor_checkers::ConstantChecker;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;

/// Check if the context expression is a constant
///
/// Constant expressions do not contain any variable or property references
/// and can be evaluated at compile time.
///
/// # Parameters
/// - `ctx_expr`: Contextual expression
///
/// # Returns
/// Returns true if the expression does not contain any variable or property references.
pub fn is_constant(ctx_expr: &ContextualExpression) -> bool {
    let expr_meta = match ctx_expr.expression() {
        Some(e) => e,
        None => return true,
    };
    let expr = expr_meta.inner();
    ConstantChecker::check(expr)
}

/// Check if an expression is a constant (based on Expression)
///
/// Constant expressions do not contain any variable or property references
/// and can be evaluated at compile time.
///
/// # Parameters
/// - `expr`: Expression
///
/// # Returns
/// Returns true if the expression does not contain any variable or property references.
pub fn is_constant_expression(expr: &Expression) -> bool {
    ConstantChecker::check(expr)
}

/// Check whether an expression can be evaluated at compile time
///
/// Checks whether the expression contains only constants, and no variables
/// or elements that require runtime context (such as property access).
///
/// # Parameters
/// - `expression`: Expression to check
///
/// # Returns
/// Returns true if the expression can be evaluated at compile time.
pub fn is_evaluable(expression: &Expression) -> bool {
    !requires_runtime_context(expression)
}

/// Check whether the expression requires a runtime context to be evaluated
fn requires_runtime_context(expression: &Expression) -> bool {
    match expression {
        Expression::Literal(_) => false,
        Expression::Variable(_) => true,
        Expression::Property { .. } => true,
        Expression::Binary { left, right, .. } => {
            requires_runtime_context(left) || requires_runtime_context(right)
        }
        Expression::Unary { operand, .. } => requires_runtime_context(operand),
        Expression::Function { args, .. } => args.iter().any(requires_runtime_context),
        Expression::Aggregate { arg, .. } => requires_runtime_context(arg),
        Expression::List(items) => items.iter().any(requires_runtime_context),
        Expression::Map(pairs) => pairs.iter().any(|(_, val)| requires_runtime_context(val)),
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            test_expr
                .as_ref()
                .is_some_and(|expr| requires_runtime_context(expr))
                || conditions.iter().any(|(cond, val)| {
                    requires_runtime_context(cond) || requires_runtime_context(val)
                })
                || default
                    .as_ref()
                    .is_some_and(|d| requires_runtime_context(d))
        }
        Expression::TypeCast { expression, .. } => requires_runtime_context(expression),
        Expression::Subscript { collection, index } => {
            requires_runtime_context(collection) || requires_runtime_context(index)
        }
        Expression::Range {
            collection,
            start,
            end,
        } => {
            requires_runtime_context(collection)
                || start.as_ref().is_some_and(|s| requires_runtime_context(s))
                || end.as_ref().is_some_and(|e| requires_runtime_context(e))
        }
        Expression::Path(items) => items.iter().any(requires_runtime_context),
        Expression::Label(_) => false,
        Expression::ListComprehension {
            source,
            filter,
            map,
            ..
        } => {
            requires_runtime_context(source)
                || filter.as_ref().is_some_and(|f| requires_runtime_context(f))
                || map.as_ref().is_some_and(|e| requires_runtime_context(e))
        }
        Expression::LabelTagProperty { tag, .. } => requires_runtime_context(tag),
        Expression::TagProperty { .. } => false,
        Expression::EdgeProperty { .. } => false,
        Expression::Predicate { args, .. } => args.iter().any(requires_runtime_context),
        Expression::Reduce {
            initial,
            source,
            mapping,
            ..
        } => {
            requires_runtime_context(initial)
                || requires_runtime_context(source)
                || requires_runtime_context(mapping)
        }
        Expression::PathBuild(exprs) => exprs.iter().any(requires_runtime_context),
        Expression::Parameter(_) => true,
        Expression::Vector(_) => false,
    }
}

/// Collect all variables in the expression
///
/// # Parameters
/// - `expression`: Expression to analyze
///
/// # Returns
/// List of all variable names
pub fn collect_variables(expression: &Expression) -> Vec<String> {
    let mut variables = Vec::new();
    collect_variables_recursive(expression, &mut variables);
    variables.sort();
    variables.dedup();
    variables
}

/// Collect all variables from a ContextualExpression
///
/// # Parameters
/// - `expression`: Contextual expression to analyze
///
/// # Returns
/// List of all variable names
pub fn collect_variables_from_contextual(expression: &ContextualExpression) -> Vec<String> {
    match expression.get_expression() {
        Some(expr) => collect_variables(&expr),
        None => Vec::new(),
    }
}

/// Recursive helper for collecting variables
fn collect_variables_recursive(expression: &Expression, variables: &mut Vec<String>) {
    match expression {
        Expression::Variable(name) if !variables.contains(name) => {
            variables.push(name.clone());
        }
        Expression::Property { object, .. } => {
            collect_variables_recursive(object, variables);
        }
        Expression::Binary { left, right, .. } => {
            collect_variables_recursive(left, variables);
            collect_variables_recursive(right, variables);
        }
        Expression::Unary { operand, .. } => {
            collect_variables_recursive(operand, variables);
        }
        Expression::Function { args, .. } => {
            for arg in args {
                collect_variables_recursive(arg, variables);
            }
        }
        Expression::Aggregate { arg, .. } => {
            collect_variables_recursive(arg, variables);
        }
        Expression::List(items) => {
            for item in items {
                collect_variables_recursive(item, variables);
            }
        }
        Expression::Map(pairs) => {
            for (_, expr) in pairs {
                collect_variables_recursive(expr, variables);
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(test) = test_expr {
                collect_variables_recursive(test, variables);
            }
            for (cond, expr) in conditions {
                collect_variables_recursive(cond, variables);
                collect_variables_recursive(expr, variables);
            }
            if let Some(def) = default {
                collect_variables_recursive(def, variables);
            }
        }
        Expression::TypeCast { expression, .. } => {
            collect_variables_recursive(expression, variables);
        }
        Expression::Subscript { collection, index } => {
            collect_variables_recursive(collection, variables);
            collect_variables_recursive(index, variables);
        }
        Expression::Range {
            collection,
            start,
            end,
        } => {
            collect_variables_recursive(collection, variables);
            if let Some(s) = start {
                collect_variables_recursive(s, variables);
            }
            if let Some(e) = end {
                collect_variables_recursive(e, variables);
            }
        }
        Expression::Path(items) => {
            for item in items {
                collect_variables_recursive(item, variables);
            }
        }
        Expression::ListComprehension {
            variable,
            source,
            filter,
            map,
        } => {
            if !variables.contains(variable) {
                variables.push(variable.clone());
            }
            collect_variables_recursive(source, variables);
            if let Some(f) = filter {
                collect_variables_recursive(f, variables);
            }
            if let Some(m) = map {
                collect_variables_recursive(m, variables);
            }
        }
        _ => {}
    }
}

/// Check whether the expression contains any aggregate functions
///
/// # Parameters
/// - `expression`: Expression to check
///
/// # Returns
/// Returns true if the expression contains an aggregate function.
pub fn has_aggregate_function(expression: &Expression) -> bool {
    match expression {
        Expression::Aggregate { .. } => true,
        Expression::Binary { left, right, .. } => {
            has_aggregate_function(left) || has_aggregate_function(right)
        }
        Expression::Unary { operand, .. } => has_aggregate_function(operand),
        Expression::Function { args, .. } => args.iter().any(has_aggregate_function),
        Expression::List(items) => items.iter().any(has_aggregate_function),
        Expression::Map(pairs) => pairs.iter().any(|(_, expr)| has_aggregate_function(expr)),
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            test_expr
                .as_ref()
                .is_some_and(|e| has_aggregate_function(e))
                || conditions.iter().any(|(cond, expr)| {
                    has_aggregate_function(cond) || has_aggregate_function(expr)
                })
                || default.as_ref().is_some_and(|e| has_aggregate_function(e))
        }
        Expression::TypeCast { expression, .. } => has_aggregate_function(expression),
        Expression::Subscript { collection, index } => {
            has_aggregate_function(collection) || has_aggregate_function(index)
        }
        Expression::Range {
            collection,
            start,
            end,
        } => {
            has_aggregate_function(collection)
                || start.as_ref().is_some_and(|e| has_aggregate_function(e))
                || end.as_ref().is_some_and(|e| has_aggregate_function(e))
        }
        Expression::Path(items) => items.iter().any(has_aggregate_function),
        Expression::ListComprehension {
            source,
            filter,
            map,
            ..
        } => {
            has_aggregate_function(source)
                || filter.as_ref().is_some_and(|e| has_aggregate_function(e))
                || map.as_ref().is_some_and(|e| has_aggregate_function(e))
        }
        Expression::Property { object, .. } => has_aggregate_function(object),
        _ => false,
    }
}

/// Extract all aggregate functions from the expression
///
/// # Parameters
/// - `expression`: Expression to analyze
///
/// # Returns
/// List of all aggregate functions
pub fn extract_aggregate_functions(expression: &Expression) -> Vec<AggregateFunction> {
    let mut functions = Vec::new();
    extract_aggregate_functions_recursive(expression, &mut functions);
    functions
}

/// Recursive helper for extracting aggregate functions
fn extract_aggregate_functions_recursive(
    expression: &Expression,
    functions: &mut Vec<AggregateFunction>,
) {
    match expression {
        Expression::Aggregate { func, .. } => {
            functions.push(func.clone());
        }
        Expression::Binary { left, right, .. } => {
            extract_aggregate_functions_recursive(left, functions);
            extract_aggregate_functions_recursive(right, functions);
        }
        Expression::Unary { operand, .. } => {
            extract_aggregate_functions_recursive(operand, functions);
        }
        Expression::Function { args, .. } => {
            for arg in args {
                extract_aggregate_functions_recursive(arg, functions);
            }
        }
        Expression::List(items) => {
            for item in items {
                extract_aggregate_functions_recursive(item, functions);
            }
        }
        Expression::Map(pairs) => {
            for (_, expr) in pairs {
                extract_aggregate_functions_recursive(expr, functions);
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(test) = test_expr {
                extract_aggregate_functions_recursive(test, functions);
            }
            for (cond, expr) in conditions {
                extract_aggregate_functions_recursive(cond, functions);
                extract_aggregate_functions_recursive(expr, functions);
            }
            if let Some(def) = default {
                extract_aggregate_functions_recursive(def, functions);
            }
        }
        Expression::TypeCast { expression, .. } => {
            extract_aggregate_functions_recursive(expression, functions);
        }
        Expression::Subscript { collection, index } => {
            extract_aggregate_functions_recursive(collection, functions);
            extract_aggregate_functions_recursive(index, functions);
        }
        Expression::Range {
            collection,
            start,
            end,
        } => {
            extract_aggregate_functions_recursive(collection, functions);
            if let Some(s) = start {
                extract_aggregate_functions_recursive(s, functions);
            }
            if let Some(e) = end {
                extract_aggregate_functions_recursive(e, functions);
            }
        }
        Expression::Path(items) => {
            for item in items {
                extract_aggregate_functions_recursive(item, functions);
            }
        }
        Expression::ListComprehension {
            source,
            filter,
            map,
            ..
        } => {
            extract_aggregate_functions_recursive(source, functions);
            if let Some(f) = filter {
                extract_aggregate_functions_recursive(f, functions);
            }
            if let Some(m) = map {
                extract_aggregate_functions_recursive(m, functions);
            }
        }
        Expression::Property { object, .. } => {
            extract_aggregate_functions_recursive(object, functions);
        }
        _ => {}
    }
}

/// Find all expressions in the expression that meet the specified matching conditions
///
/// # Parameters
/// - `expression`: Expression to search
/// - `predicate`: Function that determines the matching criteria
///
/// # Returns
/// List of all matching expressions
pub fn find_all<F>(expression: &Expression, predicate: F) -> Vec<Expression>
where
    F: Fn(&Expression) -> bool,
{
    let mut results = Vec::new();
    find_all_recursive(expression, &predicate, &mut results);
    results
}

/// Recursive helper for searching expressions
fn find_all_recursive<F>(expression: &Expression, predicate: &F, results: &mut Vec<Expression>)
where
    F: Fn(&Expression) -> bool,
{
    if predicate(expression) {
        results.push(expression.clone());
    }
    for child in expression.children() {
        find_all_recursive(child, predicate, results);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_is_constant_expression() {
        let expr = Expression::Literal(crate::core::Value::Int(1));
        assert!(is_constant_expression(&expr));

        let expr = Expression::Variable("v".to_string());
        assert!(!is_constant_expression(&expr));

        let expr = Expression::Property {
            object: Box::new(Expression::Variable("v".to_string())),
            property: "a".to_string(),
        };
        assert!(!is_constant_expression(&expr));
    }

    #[test]
    fn test_is_evaluable() {
        let expr = Expression::Literal(crate::core::Value::Int(1));
        assert!(is_evaluable(&expr));

        let expr = Expression::Variable("v".to_string());
        assert!(!is_evaluable(&expr));
    }

    #[test]
    fn test_collect_variables() {
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(Expression::Variable("a".to_string())),
            right: Box::new(Expression::Variable("b".to_string())),
        };

        let vars = collect_variables(&expr);
        assert_eq!(vars.len(), 2);
        assert!(vars.contains(&"a".to_string()));
        assert!(vars.contains(&"b".to_string()));
    }

    #[test]
    fn test_has_aggregate_function() {
        let expr = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Variable("x".to_string())),
            distinct: false,
        };
        assert!(has_aggregate_function(&expr));

        let expr = Expression::Variable("x".to_string());
        assert!(!has_aggregate_function(&expr));
    }

    #[test]
    fn test_extract_aggregate_functions() {
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(Expression::Aggregate {
                func: AggregateFunction::Count(None),
                arg: Box::new(Expression::Variable("a".to_string())),
                distinct: false,
            }),
            right: Box::new(Expression::Aggregate {
                func: AggregateFunction::Sum("b".to_string()),
                arg: Box::new(Expression::Variable("b".to_string())),
                distinct: false,
            }),
        };

        let funcs = extract_aggregate_functions(&expr);
        assert_eq!(funcs.len(), 2);
    }

    #[test]
    fn test_find_all() {
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(Expression::Variable("a".to_string())),
            right: Box::new(Expression::Variable("b".to_string())),
        };

        let vars = find_all(&expr, |e| matches!(e, Expression::Variable(_)));
        assert_eq!(vars.len(), 2);
    }
}

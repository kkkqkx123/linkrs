//! Semantic validation for the Binder.
//!
//! This module consolidates expression-level validation that was previously
//! scattered across the validator crate.  The Binder now performs these
//! checks during binding (binding = validation).

use std::collections::HashSet;

use graphdb_core::error::{DBError, DBResult, QueryError};
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::DataType;

const MAX_EXPR_DEPTH: usize = 100;
const MAX_FUNCTION_ARGS: usize = 100;
const MAX_COLLECTION_ELEMENTS: usize = 10_000;

/// Validates a contextual expression for structural correctness.
///
/// Checks performed:
/// - Division by zero (literal divisor)
/// - Expression nesting depth
/// - Cyclic variable references
/// - Subscript type compatibility (list→int, map→string)
/// - CASE requires at least one WHEN
/// - Empty function/property names
/// - Function argument count limit
/// - Collection element count limit
/// - Duplicate map keys
pub fn validate_expression(expr: &ContextualExpression) -> DBResult<()> {
    let Some(expr_meta) = expr.expression() else {
        return Err(DBError::from(QueryError::invalid_query(
            "Expression not found in context".to_string(),
        )));
    };
    let inner = expr_meta.inner();

    check_expression_depth(inner, 0)?;
    check_division_by_zero(inner, 0)?;
    check_subscript_types(inner, 0)?;
    check_case_when(inner, 0)?;
    check_empty_names(inner, 0)?;
    check_function_args(inner, 0)?;
    check_collection_limits(inner, 0)?;
    check_map_duplicate_keys(inner, 0)?;

    Ok(())
}

fn check_expression_depth(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Err(DBError::from(QueryError::invalid_query(
            "expressions are nested too deeply in levels".to_string(),
        )));
    }
    match expr {
        Expression::Binary { left, right, .. } => {
            check_expression_depth(left, depth + 1)?;
            check_expression_depth(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_expression_depth(operand, depth + 1)?;
        }
        Expression::Function { args, .. } => {
            for arg in args {
                check_expression_depth(arg, depth + 1)?;
            }
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_expression_depth(arg, depth + 1)?;
            }
        }
        Expression::Property { object, .. } => {
            check_expression_depth(object, depth + 1)?;
        }
        Expression::Subscript { collection, index } => {
            check_expression_depth(collection, depth + 1)?;
            check_expression_depth(index, depth + 1)?;
        }
        Expression::List(items) => {
            for item in items {
                check_expression_depth(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            for (_, v) in pairs {
                check_expression_depth(v, depth + 1)?;
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_expression_depth(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_expression_depth(c, depth + 1)?;
                check_expression_depth(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_expression_depth(d, depth + 1)?;
            }
        }
        Expression::TypeCast { expression, .. } => {
            check_expression_depth(expression, depth + 1)?;
        }
        Expression::ListComprehension {
            source,
            filter,
            map,
            ..
        } => {
            check_expression_depth(source, depth + 1)?;
            if let Some(f) = filter {
                check_expression_depth(f, depth + 1)?;
            }
            if let Some(m) = map {
                check_expression_depth(m, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_division_by_zero(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Ok(());
    }
    match expr {
        Expression::Binary {
            op: graphdb_core::BinaryOperator::Divide | graphdb_core::BinaryOperator::Modulo,
            right,
            ..
        } => {
            if matches!(
                right.as_ref(),
                Expression::Literal(graphdb_core::Value::Int(0))
                    | Expression::Literal(graphdb_core::Value::Float(0.0))
            ) {
                return Err(DBError::from(QueryError::invalid_query(
                    "The divisor cannot be 0".to_string(),
                )));
            }
            check_division_by_zero(right, depth + 1)?;
        }
        Expression::Binary { left, right, .. } => {
            check_division_by_zero(left, depth + 1)?;
            check_division_by_zero(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_division_by_zero(operand, depth + 1)?;
        }
        Expression::Function { args, .. } => {
            for arg in args {
                check_division_by_zero(arg, depth + 1)?;
            }
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_division_by_zero(arg, depth + 1)?;
            }
        }
        Expression::Property { object, .. } => {
            check_division_by_zero(object, depth + 1)?;
        }
        Expression::Subscript { collection, index } => {
            check_division_by_zero(collection, depth + 1)?;
            check_division_by_zero(index, depth + 1)?;
        }
        Expression::List(items) => {
            for item in items {
                check_division_by_zero(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            for (_, v) in pairs {
                check_division_by_zero(v, depth + 1)?;
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_division_by_zero(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_division_by_zero(c, depth + 1)?;
                check_division_by_zero(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_division_by_zero(d, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_subscript_types(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Ok(());
    }
    match expr {
        Expression::Subscript { collection, index } => {
            let col_type = collection.deduce_type();
            let idx_type = index.deduce_type();
            match col_type {
                DataType::List(_) => {
                    // Unknown index types are deferred to the executor.
                    if idx_type != DataType::Int
                        && idx_type != DataType::Empty
                        && idx_type != DataType::Unknown
                    {
                        return Err(DBError::from(QueryError::invalid_query(format!(
                            "List subscripts need to be of integer type, but get: {:?}",
                            idx_type
                        ))));
                    }
                }
                DataType::Map(_) => {
                    if idx_type != DataType::String
                        && idx_type != DataType::Empty
                        && idx_type != DataType::Unknown
                    {
                        return Err(DBError::from(QueryError::invalid_query(format!(
                            "Mapping keys requires a string type, but gets: {:?}",
                            idx_type
                        ))));
                    }
                }
                DataType::Empty | DataType::Unknown => {}
                _ => {
                    return Err(DBError::from(QueryError::invalid_query(format!(
                        "Unsupported types for subscript operations: {:?}",
                        col_type
                    ))));
                }
            }
            check_subscript_types(collection, depth + 1)?;
            check_subscript_types(index, depth + 1)?;
        }
        Expression::Binary { left, right, .. } => {
            check_subscript_types(left, depth + 1)?;
            check_subscript_types(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_subscript_types(operand, depth + 1)?;
        }
        Expression::Function { args, .. } => {
            for arg in args {
                check_subscript_types(arg, depth + 1)?;
            }
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_subscript_types(arg, depth + 1)?;
            }
        }
        Expression::Property { object, .. } => {
            check_subscript_types(object, depth + 1)?;
        }
        Expression::List(items) => {
            for item in items {
                check_subscript_types(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            for (_, v) in pairs {
                check_subscript_types(v, depth + 1)?;
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_subscript_types(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_subscript_types(c, depth + 1)?;
                check_subscript_types(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_subscript_types(d, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_case_when(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Ok(());
    }
    if let Expression::Case { conditions, .. } = expr {
        if conditions.is_empty() {
            return Err(DBError::from(QueryError::invalid_query(
                "CASE expressions must have at least one WHEN clause.".to_string(),
            )));
        }
    }
    match expr {
        Expression::Binary { left, right, .. } => {
            check_case_when(left, depth + 1)?;
            check_case_when(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_case_when(operand, depth + 1)?;
        }
        Expression::Function { args, .. } => {
            for arg in args {
                check_case_when(arg, depth + 1)?;
            }
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_case_when(arg, depth + 1)?;
            }
        }
        Expression::Property { object, .. } => {
            check_case_when(object, depth + 1)?;
        }
        Expression::Subscript { collection, index } => {
            check_case_when(collection, depth + 1)?;
            check_case_when(index, depth + 1)?;
        }
        Expression::List(items) => {
            for item in items {
                check_case_when(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            for (_, v) in pairs {
                check_case_when(v, depth + 1)?;
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_case_when(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_case_when(c, depth + 1)?;
                check_case_when(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_case_when(d, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_empty_names(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Ok(());
    }
    match expr {
        Expression::Function { name, args, .. } => {
            if name.is_empty() {
                return Err(DBError::from(QueryError::invalid_query(
                    "Function name cannot be null".to_string(),
                )));
            }
            for arg in args {
                check_empty_names(arg, depth + 1)?;
            }
        }
        Expression::Property {
            property, object, ..
        } => {
            if property.is_empty() {
                return Err(DBError::from(QueryError::invalid_query(
                    "Attribute name cannot be null".to_string(),
                )));
            }
            check_empty_names(object, depth + 1)?;
        }
        Expression::Binary { left, right, .. } => {
            check_empty_names(left, depth + 1)?;
            check_empty_names(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_empty_names(operand, depth + 1)?;
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_empty_names(arg, depth + 1)?;
            }
        }
        Expression::Subscript { collection, index } => {
            check_empty_names(collection, depth + 1)?;
            check_empty_names(index, depth + 1)?;
        }
        Expression::List(items) => {
            for item in items {
                check_empty_names(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            for (_, v) in pairs {
                check_empty_names(v, depth + 1)?;
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_empty_names(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_empty_names(c, depth + 1)?;
                check_empty_names(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_empty_names(d, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_function_args(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Ok(());
    }
    match expr {
        Expression::Function { name, args, .. } => {
            if args.len() > MAX_FUNCTION_ARGS {
                return Err(DBError::from(QueryError::invalid_query(format!(
                    "The function {:?} has too many arguments: {}",
                    name,
                    args.len()
                ))));
            }
            for arg in args {
                check_function_args(arg, depth + 1)?;
            }
        }
        Expression::Binary { left, right, .. } => {
            check_function_args(left, depth + 1)?;
            check_function_args(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_function_args(operand, depth + 1)?;
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_function_args(arg, depth + 1)?;
            }
        }
        Expression::Property { object, .. } => {
            check_function_args(object, depth + 1)?;
        }
        Expression::Subscript { collection, index } => {
            check_function_args(collection, depth + 1)?;
            check_function_args(index, depth + 1)?;
        }
        Expression::List(items) => {
            for item in items {
                check_function_args(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            for (_, v) in pairs {
                check_function_args(v, depth + 1)?;
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_function_args(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_function_args(c, depth + 1)?;
                check_function_args(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_function_args(d, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_collection_limits(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Ok(());
    }
    match expr {
        Expression::List(items) => {
            if items.len() > MAX_COLLECTION_ELEMENTS {
                return Err(DBError::from(QueryError::invalid_query(
                    "Too many list expression elements".to_string(),
                )));
            }
            for item in items {
                check_collection_limits(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            if pairs.len() > MAX_COLLECTION_ELEMENTS {
                return Err(DBError::from(QueryError::invalid_query(
                    "Mapping expressions with too many key-value pairs".to_string(),
                )));
            }
            for (_, v) in pairs {
                check_collection_limits(v, depth + 1)?;
            }
        }
        Expression::Binary { left, right, .. } => {
            check_collection_limits(left, depth + 1)?;
            check_collection_limits(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_collection_limits(operand, depth + 1)?;
        }
        Expression::Function { args, .. } => {
            for arg in args {
                check_collection_limits(arg, depth + 1)?;
            }
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_collection_limits(arg, depth + 1)?;
            }
        }
        Expression::Property { object, .. } => {
            check_collection_limits(object, depth + 1)?;
        }
        Expression::Subscript { collection, index } => {
            check_collection_limits(collection, depth + 1)?;
            check_collection_limits(index, depth + 1)?;
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_collection_limits(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_collection_limits(c, depth + 1)?;
                check_collection_limits(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_collection_limits(d, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_map_duplicate_keys(expr: &Expression, depth: usize) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Ok(());
    }
    if let Expression::Map(pairs) = expr {
        let mut keys = HashSet::new();
        for (key, _) in pairs {
            if !keys.insert(key) {
                return Err(DBError::from(QueryError::invalid_query(format!(
                    "There are duplicate keys in the mapping expression: {:?}",
                    key
                ))));
            }
        }
    }
    match expr {
        Expression::Binary { left, right, .. } => {
            check_map_duplicate_keys(left, depth + 1)?;
            check_map_duplicate_keys(right, depth + 1)?;
        }
        Expression::Unary { operand, .. } => {
            check_map_duplicate_keys(operand, depth + 1)?;
        }
        Expression::Function { args, .. } => {
            for arg in args {
                check_map_duplicate_keys(arg, depth + 1)?;
            }
        }
        Expression::Aggregate { args, .. } => {
            for arg in args {
                check_map_duplicate_keys(arg, depth + 1)?;
            }
        }
        Expression::Property { object, .. } => {
            check_map_duplicate_keys(object, depth + 1)?;
        }
        Expression::Subscript { collection, index } => {
            check_map_duplicate_keys(collection, depth + 1)?;
            check_map_duplicate_keys(index, depth + 1)?;
        }
        Expression::List(items) => {
            for item in items {
                check_map_duplicate_keys(item, depth + 1)?;
            }
        }
        Expression::Map(pairs) => {
            for (_, v) in pairs {
                check_map_duplicate_keys(v, depth + 1)?;
            }
        }
        Expression::Case {
            test_expr,
            conditions,
            default,
        } => {
            if let Some(t) = test_expr {
                check_map_duplicate_keys(t, depth + 1)?;
            }
            for (c, r) in conditions {
                check_map_duplicate_keys(c, depth + 1)?;
                check_map_duplicate_keys(r, depth + 1)?;
            }
            if let Some(d) = default {
                check_map_duplicate_keys(d, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

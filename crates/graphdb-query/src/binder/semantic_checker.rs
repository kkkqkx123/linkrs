//! Semantic validation for the Binder.
//!
//! This module consolidates expression-level validation that was previously
//! scattered across the validator crate.  The Binder now performs these
//! checks during binding (binding = validation).

use std::collections::HashSet;

use graphdb_core::error::{DBError, DBResult, QueryError};
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::{walk_expr, ExprValidator, MAX_EXPR_DEPTH};
use graphdb_core::types::expr::Expression;
use graphdb_core::types::DataType;

const MAX_FUNCTION_ARGS: usize = 100;
const MAX_COLLECTION_ELEMENTS: usize = 10_000;

/// Validates a contextual expression for structural correctness.
///
/// Checks performed:
/// - Division by zero (literal divisor)
/// - Expression nesting depth
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

    let mut validator = CompositeValidator::new();
    walk_expr(inner, 0, &mut validator)?;

    Ok(())
}

/// Composite validator that performs all checks in a single traversal.
struct CompositeValidator;

impl CompositeValidator {
    fn new() -> Self {
        Self
    }
}

impl ExprValidator for CompositeValidator {
    fn validate(&mut self, expr: &Expression, depth: usize) -> DBResult<()> {
        if depth > MAX_EXPR_DEPTH {
            return Err(DBError::from(QueryError::invalid_query(
                "expressions are nested too deeply in levels".to_string(),
            )));
        }
        self.check_division_by_zero(expr)?;
        self.check_subscript_types(expr, depth)?;
        self.check_case_when(expr)?;
        self.check_empty_names(expr)?;
        self.check_function_args(expr)?;
        self.check_collection_limits(expr)?;
        self.check_map_duplicate_keys(expr)?;
        Ok(())
    }
}

impl CompositeValidator {
    fn check_division_by_zero(&mut self, expr: &Expression) -> DBResult<()> {
        if let Expression::Binary {
            op: graphdb_core::BinaryOperator::Divide | graphdb_core::BinaryOperator::Modulo,
            right,
            ..
        } = expr
        {
            if matches!(
                right.as_ref(),
                Expression::Literal(graphdb_core::Value::Int(0))
                    | Expression::Literal(graphdb_core::Value::Float(0.0))
            ) {
                return Err(DBError::from(QueryError::invalid_query(
                    "The divisor cannot be 0".to_string(),
                )));
            }
        }
        Ok(())
    }

    fn check_subscript_types(&mut self, expr: &Expression, _depth: usize) -> DBResult<()> {
        if let Expression::Subscript { collection, index } = expr {
            let col_type = collection.deduce_type();
            let idx_type = index.deduce_type();
            match col_type {
                DataType::List(_) => {
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
        }
        Ok(())
    }

    fn check_case_when(&mut self, expr: &Expression) -> DBResult<()> {
        if let Expression::Case { conditions, .. } = expr {
            if conditions.is_empty() {
                return Err(DBError::from(QueryError::invalid_query(
                    "CASE expressions must have at least one WHEN clause.".to_string(),
                )));
            }
        }
        Ok(())
    }

    fn check_empty_names(&mut self, expr: &Expression) -> DBResult<()> {
        match expr {
            Expression::Function { name, .. } => {
                if name.is_empty() {
                    return Err(DBError::from(QueryError::invalid_query(
                        "Function name cannot be null".to_string(),
                    )));
                }
            }
            Expression::Property { property, .. } if property.is_empty() => {
                return Err(DBError::from(QueryError::invalid_query(
                    "Attribute name cannot be null".to_string(),
                )));
            }
            _ => {}
        }
        Ok(())
    }

    fn check_function_args(&mut self, expr: &Expression) -> DBResult<()> {
        let (name, args) = match expr {
            Expression::Function { name, args } => (name.as_str(), args.as_slice()),
            Expression::WindowFunction { name, args, .. } => (name.as_str(), args.as_slice()),
            _ => return Ok(()),
        };

        if args.len() > MAX_FUNCTION_ARGS {
            return Err(DBError::from(QueryError::invalid_query(format!(
                "The function {:?} has too many arguments: {}",
                name,
                args.len()
            ))));
        }

        let registry = crate::executor::expression::functions::global_registry_ref();
        if let Some(func) = registry.get_builtin(name) {
            let expected = func.arity();
            let variadic = func.is_variadic();
            if variadic {
                if args.len() < expected {
                    return Err(DBError::from(QueryError::invalid_query(format!(
                        "Function '{}' expects at least {} arguments, got {}",
                        name,
                        expected,
                        args.len()
                    ))));
                }
            } else if args.len() != expected {
                return Err(DBError::from(QueryError::invalid_query(format!(
                    "Function '{}' expects {} arguments, got {}",
                    name,
                    expected,
                    args.len()
                ))));
            }
        } else if let Some(func) = registry.get_custom(name) {
            let expected = func.arity;
            let variadic = func.is_variadic;
            if variadic {
                if args.len() < expected {
                    return Err(DBError::from(QueryError::invalid_query(format!(
                        "Function '{}' expects at least {} arguments, got {}",
                        name,
                        expected,
                        args.len()
                    ))));
                }
            } else if args.len() != expected {
                return Err(DBError::from(QueryError::invalid_query(format!(
                    "Function '{}' expects {} arguments, got {}",
                    name,
                    expected,
                    args.len()
                ))));
            }
        }
        Ok(())
    }

    fn check_collection_limits(&mut self, expr: &Expression) -> DBResult<()> {
        match expr {
            Expression::List(items) => {
                if items.len() > MAX_COLLECTION_ELEMENTS {
                    return Err(DBError::from(QueryError::invalid_query(
                        "Too many list expression elements".to_string(),
                    )));
                }
            }
            Expression::Map(pairs) if pairs.len() > MAX_COLLECTION_ELEMENTS => {
                return Err(DBError::from(QueryError::invalid_query(
                    "Mapping expressions with too many key-value pairs".to_string(),
                )));
            }
            _ => {}
        }
        Ok(())
    }

    fn check_map_duplicate_keys(&mut self, expr: &Expression) -> DBResult<()> {
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
        Ok(())
    }
}

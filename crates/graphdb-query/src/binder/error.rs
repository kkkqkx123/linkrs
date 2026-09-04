//! Unified error helpers for the Binder module.
//!
//! This module provides centralized error construction utilities to ensure
//! consistent error messages and proper error handling throughout the binder.

use graphdb_core::error::{DBError, QueryError};
use graphdb_core::DataType;

/// Create an invalid query error.
pub(crate) fn invalid_query(msg: impl Into<String>) -> DBError {
    DBError::from(QueryError::invalid_query(msg))
}

/// Create an undefined variable error.
pub(crate) fn undefined_variable(name: &str) -> DBError {
    invalid_query(format!("Undefined variable: {}", name))
}

/// Create a type mismatch error.
pub(crate) fn type_mismatch(expected: &str, got: &DataType) -> DBError {
    invalid_query(format!("Expected {}, got {:?}", expected, got))
}

/// Create an undefined property error.
pub(crate) fn undefined_property(var_name: &str, property: &str) -> DBError {
    invalid_query(format!(
        "Property '{}' not found on variable '{}'",
        property, var_name
    ))
}

/// Create an undefined struct field error.
pub(crate) fn undefined_struct_field(field: &str, base_type: &DataType) -> DBError {
    invalid_query(format!(
        "Struct field '{}' could not be resolved against its base type {:?}",
        field, base_type
    ))
}

/// Create a function not found error.
pub(crate) fn function_not_found(name: &str) -> DBError {
    invalid_query(format!("Function '{}' not found", name))
}

/// Create an aggregate function not found error.
pub(crate) fn aggregate_function_not_found(name: &str) -> DBError {
    invalid_query(format!("Aggregate function '{}' not found", name))
}

/// Create a division by zero error.
pub(crate) fn division_by_zero() -> DBError {
    invalid_query("Division by zero")
}

/// Create an invalid expression error.
pub(crate) fn invalid_expression(msg: impl Into<String>) -> DBError {
    invalid_query(format!("Invalid expression: {}", msg.into()))
}

/// Create a schema not found error.
pub(crate) fn schema_not_found(schema_name: &str) -> DBError {
    invalid_query(format!("Schema '{}' not found", schema_name))
}

/// Create a tag not found error.
pub(crate) fn tag_not_found(space: &str, tag: &str) -> DBError {
    invalid_query(format!("Tag '{}' not found in space '{}'", tag, space))
}

/// Create a property type resolution error.
pub(crate) fn property_type_not_found(var_name: &str, property: &str) -> DBError {
    invalid_query(format!(
        "Cannot resolve type for property '{}.{}'",
        var_name, property
    ))
}

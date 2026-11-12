//! Unified error helpers for the Binder module.
//!
//! This module provides centralized error construction utilities to ensure
//! consistent error messages and proper error handling throughout the binder.

use linkrs_core::error::{DBError, QueryError};
use linkrs_core::DataType;

/// Create an invalid query error.
pub(crate) fn invalid_query(msg: impl Into<String>) -> DBError {
    DBError::from(QueryError::invalid_query(msg))
}

/// Create an undefined struct field error.
pub(crate) fn undefined_struct_field(field: &str, base_type: &DataType) -> DBError {
    invalid_query(format!(
        "Struct field '{}' could not be resolved against its base type {:?}",
        field, base_type
    ))
}

/// Create an undefined variable error.
///
/// Sole construction point for unbound variable references found by the
/// scope-sensitive existence check. Callers must not format this message
/// inline so client parsing stays stable.
pub(crate) fn undefined_variable(name: &str) -> DBError {
    invalid_query(format!("Undefined variable: {}", name))
}

/// Create a duplicate variable error.
///
/// Sole construction point for pattern and CTE variable redefinition.
/// Callers must not format this message inline.
pub(crate) fn duplicate_variable(name: &str) -> DBError {
    invalid_query(format!("Duplicate variable: {}", name))
}

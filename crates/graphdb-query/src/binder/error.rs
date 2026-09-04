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

/// Create an undefined struct field error.
pub(crate) fn undefined_struct_field(field: &str, base_type: &DataType) -> DBError {
    invalid_query(format!(
        "Struct field '{}' could not be resolved against its base type {:?}",
        field, base_type
    ))
}

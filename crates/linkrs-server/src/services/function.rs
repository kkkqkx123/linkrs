//! Transport-neutral custom function registry mutation helpers.

use parking_lot::RwLock;

use crate::http::error::HttpError;
use crate::query::executor::expression::functions::{FunctionRegistry, UdfError};

/// Transport-neutral function registry mutation failure.
#[derive(Debug)]
pub(crate) enum FunctionOpError {
    NotFound(String),
    Conflict(String),
    Invalid(String),
    Failed(String),
}

pub(crate) fn udf_error_to_op(error: UdfError) -> FunctionOpError {
    let message = error.to_string();
    match error {
        UdfError::InvalidPath(_, _) | UdfError::InvalidName(_) => FunctionOpError::Invalid(message),
        UdfError::AlreadyLoaded(_, _) | UdfError::BuiltinConflict(_) => {
            FunctionOpError::Conflict(message)
        }
        UdfError::NotLoaded(_) => FunctionOpError::NotFound(message),
        _ => FunctionOpError::Failed(message),
    }
}

/// Register the UDF library at `implementation` (a local dynamic-library
/// path) under `name`. Returns the registered (library-provided) name.
///
/// Only library-backed implementations can execute, so registration goes
/// through the dynamic UDF loader: metadata-only registrations would
/// produce functions that can never run.
pub(crate) fn register_udf_from_source(
    registry: &RwLock<FunctionRegistry>,
    name: &str,
    implementation: &str,
) -> Result<String, FunctionOpError> {
    if name.trim().is_empty() {
        return Err(FunctionOpError::Invalid(
            "function name must not be empty".to_string(),
        ));
    }
    if registry.read().contains(name) {
        return Err(FunctionOpError::Conflict(format!(
            "function '{name}' already exists"
        )));
    }
    if implementation.trim().is_empty() {
        return Err(FunctionOpError::Invalid(
            "implementation must be a local UDF library path; remote sources and embedded code are not supported".to_string(),
        ));
    }
    let mut guard = registry.write();
    match guard.install_dynamic_udf(implementation) {
        Ok(loaded_name) => {
            if loaded_name.to_uppercase() != name.to_uppercase() {
                let _ = guard.unload_dynamic_udf(&loaded_name);
                Err(FunctionOpError::Invalid(format!(
                    "library exports '{loaded_name}', which does not match requested '{name}'"
                )))
            } else {
                Ok(loaded_name)
            }
        }
        Err(e) => Err(udf_error_to_op(e)),
    }
}

/// Unload a dynamic UDF or remove a custom function by name.
pub(crate) fn unregister_udf_by_name(
    registry: &RwLock<FunctionRegistry>,
    name: &str,
) -> Result<(), FunctionOpError> {
    if !registry.read().contains(name) {
        return Err(FunctionOpError::NotFound(format!(
            "function '{name}' does not exist"
        )));
    }
    if registry.read().get_builtin(name).is_some() {
        return Err(FunctionOpError::Invalid(format!(
            "built-in function '{name}' cannot be unregistered"
        )));
    }
    let mut guard = registry.write();
    if guard.is_dynamic(name) {
        guard.unload_dynamic_udf(name).map_err(udf_error_to_op)
    } else if guard.unregister_custom(name) {
        Ok(())
    } else {
        Err(FunctionOpError::NotFound(format!(
            "function '{name}' does not exist"
        )))
    }
}

pub(crate) fn op_error_to_http(error: FunctionOpError) -> HttpError {
    match error {
        FunctionOpError::NotFound(message) => HttpError::not_found(message),
        FunctionOpError::Conflict(message) | FunctionOpError::Invalid(message) => {
            HttpError::bad_request(message)
        }
        FunctionOpError::Failed(message) => HttpError::internal(message),
    }
}

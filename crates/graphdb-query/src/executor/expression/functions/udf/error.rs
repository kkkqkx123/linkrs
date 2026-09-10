//! UDF error type.

use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use thiserror::Error;

/// Errors raised while loading, unloading, or executing dynamic UDFs.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum UdfError {
    #[error("invalid library path '{0}': {1}")]
    InvalidPath(String, String),

    #[error("failed to load library '{0}': {1}")]
    LibraryLoad(String, String),

    #[error("symbol '{1}' not found in library '{0}'")]
    SymbolNotFound(String, String),

    #[error("plugin factory in '{0}' returned a null pointer")]
    NullPlugin(String),

    #[error("plugin in '{0}' reports an empty function name")]
    InvalidName(String),

    #[error("function '{0}' is already loaded from '{1}'")]
    AlreadyLoaded(String, String),

    #[error("no dynamic function loaded under name '{0}'")]
    NotLoaded(String),

    #[error("function name '{0}' conflicts with a builtin function")]
    BuiltinConflict(String),

    #[error("plugin '{0}' ABI version mismatch: expected {1}, got {2}")]
    VersionMismatch(String, u32, u32),

    #[error("dynamic function '{0}' panicked and was isolated: {1}")]
    ExecutionPanic(String, String),

    #[error("remote extension sources are not supported yet: '{0}'")]
    RepoDownloadUnsupported(String),

    #[error("io error for '{0}': {1}")]
    Io(String, String),
}

impl UdfError {
    /// Map the error into an expression-level error for query execution.
    pub fn to_expression_error(&self) -> ExpressionError {
        let error_type = match self {
            UdfError::ExecutionPanic(_, _) => ExpressionErrorType::FunctionExecutionError,
            UdfError::NotLoaded(_) => ExpressionErrorType::UndefinedFunction,
            _ => ExpressionErrorType::InvalidOperation,
        };
        ExpressionError::new(error_type, self.to_string())
    }
}

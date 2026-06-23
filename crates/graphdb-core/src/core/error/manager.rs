//! Manager Error Type
//!
//! Errors related to the Manager layer components, including the Schema Manager, Index Manager, and Storage Client.

use thiserror::Error;

use crate::core::error::codes::{ErrorCode, PublicError, ToPublicError};

/// Manager operation result type
pub type ManagerResult<T> = Result<T, ManagerError>;

/// Incorrect classification of the error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// retryable error
    Retryable,
    /// An error that cannot be retried.
    NonRetryable,
}

/// Manager Error Type
#[derive(Error, Debug, Clone, PartialEq)]
pub enum ManagerError {
    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Resource already exists: {0}")]
    AlreadyExists(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Schema error: {0}")]
    SchemaError(String),

    #[error("Transaction error: {0}")]
    TransactionError(String),

    #[error("Timeout error: {0}")]
    TimeoutError(String),

    #[error("Other error: {0}")]
    Other(String),
}

impl ManagerError {
    /// Obtaining the error classification
    pub fn category(&self) -> ErrorCategory {
        match self {
            ManagerError::StorageError(_) | ManagerError::TimeoutError(_) => {
                ErrorCategory::Retryable
            }
            _ => ErrorCategory::NonRetryable,
        }
    }

    /// Checking for retryability
    pub fn is_retryable(&self) -> bool {
        matches!(self.category(), ErrorCategory::Retryable)
    }

    /// The error “Create not found” was not encountered during the process.
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    /// Create Existing Error
    pub fn already_exists(msg: impl Into<String>) -> Self {
        Self::AlreadyExists(msg.into())
    }

    /// An error was generated due to invalid input.
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }

    /// Creating Storage Errors
    pub fn storage_error(msg: impl Into<String>) -> Self {
        Self::StorageError(msg.into())
    }

    /// An error occurred while creating the schema.
    pub fn schema_error(msg: impl Into<String>) -> Self {
        Self::SchemaError(msg.into())
    }

    /// Create Transaction Error
    pub fn transaction_error(msg: impl Into<String>) -> Self {
        Self::TransactionError(msg.into())
    }

    /// An error occurred due to a timeout.
    pub fn timeout_error(msg: impl Into<String>) -> Self {
        Self::TimeoutError(msg.into())
    }
}

impl ToPublicError for ManagerError {
    fn to_public_error(&self) -> PublicError {
        PublicError::new(self.to_error_code(), self.to_public_message())
    }

    fn to_error_code(&self) -> ErrorCode {
        match self {
            ManagerError::NotFound(_) => ErrorCode::ResourceNotFound,
            ManagerError::AlreadyExists(_) => ErrorCode::ResourceAlreadyExists,
            ManagerError::InvalidInput(_) => ErrorCode::InvalidInput,
            ManagerError::TimeoutError(_) => ErrorCode::Timeout,
            _ => ErrorCode::InternalError,
        }
    }

    fn to_public_message(&self) -> String {
        self.to_string()
    }
}

//! API Core Layer Error Types
//!
//! Business logic errors not related to the transport layer

use thiserror::Error;

/// Extended Error Code Types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendedErrorCode {
    None = 0,

    // Parsing Related (1000-1099)
    SyntaxError = 1000,
    SemanticError = 1001,
    UnexpectedToken = 1002,
    UnterminatedLiteral = 1003,

    // Type-related (1100-1199)
    TypeMismatch = 1100,
    DivisionByZero = 1101,
    OutOfRange = 1102,

    // Binding related (1200-1299)
    DuplicateKey = 1200,
    ForeignKeyConstraint = 1201,
    NotNullConstraint = 1202,
    UniqueConstraint = 1203,
    CheckConstraint = 1204,

    // Concurrency-related (1300-1399)
    ConnectionLost = 1300,
    Deadlock = 1301,
    LockTimeout = 1302,

    // Figure correlation (1400-1499)
    InvalidVertex = 1400,
    InvalidEdge = 1401,
    PathNotFound = 1402,
}

impl ExtendedErrorCode {
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }
}

/// Core layer error types
#[derive(Error, Debug, Clone)]
pub enum CoreError {
    #[error("Query execution failed: {0}")]
    QueryExecutionFailed(String),

    #[error("Transaction operation failed: {0}")]
    TransactionFailed(String),

    #[error("Schema operation failed: {0}")]
    SchemaOperationFailed(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Query error: {message}")]
    DetailedQueryError {
        message: String,
        extended_code: ExtendedErrorCode,
        offset: Option<usize>,
    },

    #[error("Sync error: {0}")]
    SyncError(String),

    #[error("Vector error: {0}")]
    VectorError(String),
}

impl CoreError {
    pub fn extended_code(&self) -> ExtendedErrorCode {
        match self {
            CoreError::DetailedQueryError { extended_code, .. } => *extended_code,
            _ => ExtendedErrorCode::None,
        }
    }

    pub fn error_offset(&self) -> Option<usize> {
        match self {
            CoreError::DetailedQueryError { offset, .. } => *offset,
            _ => None,
        }
    }

    pub fn detailed_query_error(
        message: impl Into<String>,
        extended_code: ExtendedErrorCode,
        offset: Option<usize>,
    ) -> Self {
        CoreError::DetailedQueryError {
            message: message.into(),
            extended_code,
            offset,
        }
    }
}

/// Core layer result types
pub type CoreResult<T> = Result<T, CoreError>;

impl From<crate::core::error::QueryError> for CoreError {
    fn from(err: crate::core::error::QueryError) -> Self {
        CoreError::QueryExecutionFailed(err.to_string())
    }
}

impl From<crate::storage::StorageError> for CoreError {
    fn from(err: crate::storage::StorageError) -> Self {
        CoreError::StorageError(err.to_string())
    }
}

impl From<crate::core::error::DBError> for CoreError {
    fn from(err: crate::core::error::DBError) -> Self {
        use crate::core::error::ErrorKind;
        match err.kind() {
            ErrorKind::Query => CoreError::QueryExecutionFailed(err.message().to_string()),
            ErrorKind::Storage => CoreError::StorageError(err.message().to_string()),
            ErrorKind::Transaction => CoreError::TransactionFailed(err.message().to_string()),
            _ => CoreError::Internal(err.to_string()),
        }
    }
}

impl From<crate::transaction::TransactionError> for CoreError {
    fn from(err: crate::transaction::TransactionError) -> Self {
        CoreError::TransactionFailed(err.to_string())
    }
}

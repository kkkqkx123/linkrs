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

    // Inside (1500-1599)
    Internal = 1500,
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
        position: Option<linkrs_core::types::Position>,
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

    pub fn error_position(&self) -> Option<linkrs_core::types::Position> {
        match self {
            CoreError::DetailedQueryError { position, .. } => *position,
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
            position: None,
        }
    }

    pub fn detailed_query_error_with_position(
        message: impl Into<String>,
        extended_code: ExtendedErrorCode,
        offset: Option<usize>,
        position: Option<linkrs_core::types::Position>,
    ) -> Self {
        CoreError::DetailedQueryError {
            message: message.into(),
            extended_code,
            offset,
            position,
        }
    }
}

/// Map the engine's error taxonomy onto the wire-visible extended codes.
///
/// Every engine error carries a kind, so the extended code is always
/// populated instead of collapsing to `None` for non-syntax failures.
///
/// The mapping is intentionally coarse: the wire codes are a small fixed
/// taxonomy, so several kinds share one code. Resist the urge to grow
/// `ExtendedErrorCode` to make individual kinds distinct.
fn extended_code_from_query_kind(kind: linkrs_core::error::query::QueryErrorKind) -> ExtendedErrorCode {
    use linkrs_core::error::query::QueryErrorKind;
    match kind {
        QueryErrorKind::Parse => ExtendedErrorCode::SyntaxError,
        QueryErrorKind::Type => ExtendedErrorCode::TypeMismatch,
        QueryErrorKind::Transaction => ExtendedErrorCode::Deadlock,
        QueryErrorKind::Timeout => ExtendedErrorCode::LockTimeout,
        QueryErrorKind::InvalidQuery
        | QueryErrorKind::Permission
        | QueryErrorKind::Session
        | QueryErrorKind::FeatureDisabled => ExtendedErrorCode::SemanticError,
        QueryErrorKind::Storage
        | QueryErrorKind::Planning
        | QueryErrorKind::Optimization
        | QueryErrorKind::Execution
        | QueryErrorKind::Expression
        | QueryErrorKind::PlanNodeVisit => ExtendedErrorCode::Internal,
    }
}

/// Core layer result types
pub type CoreResult<T> = Result<T, CoreError>;

impl From<linkrs_core::error::QueryError> for CoreError {
    fn from(err: linkrs_core::error::QueryError) -> Self {
        CoreError::detailed_query_error_with_position(
            err.to_string(),
            extended_code_from_query_kind(err.kind()),
            err.offset(),
            err.parse_error_position(),
        )
    }
}

impl From<linkrs_storage::StorageError> for CoreError {
    fn from(err: linkrs_storage::StorageError) -> Self {
        CoreError::StorageError(err.to_string())
    }
}

impl From<linkrs_core::error::DBError> for CoreError {
    fn from(err: linkrs_core::error::DBError) -> Self {
        use linkrs_core::error::ErrorKind;
        match err.kind() {
            ErrorKind::Query => {
                if let Some(source) = err.source() {
                    if let Some(query_err) = source.downcast_ref::<linkrs_core::error::QueryError>()
                    {
                        return CoreError::from(query_err.clone());
                    }
                }
                CoreError::QueryExecutionFailed(err.message().to_string())
            }
            ErrorKind::Storage => CoreError::StorageError(err.message().to_string()),
            ErrorKind::Transaction => CoreError::TransactionFailed(err.message().to_string()),
            _ => CoreError::Internal(err.to_string()),
        }
    }
}

impl From<linkrs_transaction::TransactionError> for CoreError {
    fn from(err: linkrs_transaction::TransactionError) -> Self {
        CoreError::TransactionFailed(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_error_kinds_map_to_distinct_extended_codes() {
        use linkrs_core::error::query::QueryErrorKind;
        let cases = [
            (QueryErrorKind::Parse, ExtendedErrorCode::SyntaxError),
            (QueryErrorKind::Type, ExtendedErrorCode::TypeMismatch),
            (QueryErrorKind::Transaction, ExtendedErrorCode::Deadlock),
            (QueryErrorKind::Timeout, ExtendedErrorCode::LockTimeout),
            (QueryErrorKind::InvalidQuery, ExtendedErrorCode::SemanticError),
            (QueryErrorKind::Execution, ExtendedErrorCode::Internal),
        ];
        for (kind, expected) in cases {
            let error = CoreError::from(linkrs_core::error::QueryError::new(
                kind,
                "boom",
            ));
            assert_eq!(error.extended_code(), expected, "kind {kind:?}");
            assert!(
                error.to_string().contains("boom"),
                "message survives the mapping: {error}"
            );
        }
    }

    #[test]
    fn converted_query_errors_are_always_detailed() {
        let error = CoreError::from(linkrs_core::error::QueryError::new(
            linkrs_core::error::query::QueryErrorKind::Type,
            "type mismatch",
        ));
        assert_eq!(error.extended_code(), ExtendedErrorCode::TypeMismatch);
        assert!(error.error_offset().is_none());
        assert!(error.error_position().is_none());
    }
}

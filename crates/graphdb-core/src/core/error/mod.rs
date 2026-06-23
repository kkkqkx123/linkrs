//! Unified Error Handling System for GraphDB
//!
//! ## Design concepts ##
//!
//! 1. **Boxed errors**: All sub-errors are boxed to keep DBError small (~24 bytes instead of ~160 bytes)
//! 2. **Error classification**: Each error has a class for unified handling strategies
//! 3. **Error chain**: Full error chain support with source tracking
//! 4. **Public API**: Stable error codes for external API, hiding internal details
//!
//! ## Error Size Comparison ##
//! - Old design: ~160 bytes (embedded sub-errors)
//! - New design: ~24 bytes (boxed sub-errors)

use std::error::Error;
use std::sync::Arc;
use thiserror::Error;

// submodule
pub mod codes;
pub mod manager;
pub mod query;
pub mod storage;

// Re-export the error code
pub use codes::{ErrorCategory as CodeErrorCategory, ErrorCode, PublicError, ToPublicError};

// Re-export all error types
pub use manager::{ErrorCategory, ManagerError, ManagerResult};
pub use query::PlanNodeVisitError;
pub use query::QueryError;
pub use query::QueryResult;
pub use storage::StorageError;
pub use storage::StorageResult;

pub use crate::core::types::DataType;

// ==================== Error Classification ====================

/// Error classification for unified handling strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Transient errors that can be retried
    Retryable,
    /// User input errors (client should fix the request)
    UserError,
    /// System errors (server-side issues)
    SystemError,
    /// Fatal errors (service should stop)
    Fatal,
}

impl ErrorClass {
    pub fn is_retryable(&self) -> bool {
        matches!(self, ErrorClass::Retryable)
    }

    pub fn is_user_error(&self) -> bool {
        matches!(self, ErrorClass::UserError)
    }

    pub fn is_system_error(&self) -> bool {
        matches!(self, ErrorClass::SystemError | ErrorClass::Fatal)
    }
}

// ==================== Boxed Error Type ====================

/// Thread-safe boxed error type
pub type BoxedError = Box<dyn Error + Send + Sync>;

/// Reference-counted error for cloning
pub type SharedError = Arc<dyn Error + Send + Sync>;

// ==================== Error Kind ====================

/// Error kind enumeration for categorization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Storage,
    Query,
    Expression,
    Plan,
    Manager,
    Validation,
    Io,
    TypeDeduction,
    Serialization,
    Index,
    Transaction,
    GraphService,
    Internal,
    Session,
    Auth,
    Permission,
    MemoryLimitExceeded,
    Fulltext,
    Coordinator,
    Vector,
    VectorCoordinator,
    Search,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::Storage => "storage",
            ErrorKind::Query => "query",
            ErrorKind::Expression => "expression",
            ErrorKind::Plan => "plan",
            ErrorKind::Manager => "manager",
            ErrorKind::Validation => "validation",
            ErrorKind::Io => "io",
            ErrorKind::TypeDeduction => "type_deduction",
            ErrorKind::Serialization => "serialization",
            ErrorKind::Index => "index",
            ErrorKind::Transaction => "transaction",
            ErrorKind::GraphService => "graph_service",
            ErrorKind::Internal => "internal",
            ErrorKind::Session => "session",
            ErrorKind::Auth => "auth",
            ErrorKind::Permission => "permission",
            ErrorKind::MemoryLimitExceeded => "memory_limit_exceeded",
            ErrorKind::Fulltext => "fulltext",
            ErrorKind::Coordinator => "coordinator",
            ErrorKind::Vector => "vector",
            ErrorKind::VectorCoordinator => "vector_coordinator",
            ErrorKind::Search => "search",
        }
    }
}

// ==================== Core DBError ====================

/// Unified database error type
///
/// Design principles:
/// 1. Small size: Uses boxed errors to keep enum size minimal (~24 bytes)
/// 2. Full context: Preserves error chain and classification
/// 3. Clone support: Can be cloned for logging/propagation
#[derive(Error, Debug)]
pub struct DBError {
    kind: ErrorKind,
    message: String,
    #[source]
    source: Option<BoxedError>,
    class: ErrorClass,
}

impl DBError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let class = kind.default_class();
        Self {
            kind,
            message: message.into(),
            source: None,
            class,
        }
    }

    pub fn with_source(mut self, source: BoxedError) -> Self {
        self.source = Some(source);
        self
    }

    pub fn with_class(mut self, class: ErrorClass) -> Self {
        self.class = class;
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn class(&self) -> ErrorClass {
        self.class
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_retryable(&self) -> bool {
        self.class.is_retryable()
    }

    pub fn is_user_error(&self) -> bool {
        self.class.is_user_error()
    }

    pub fn is_system_error(&self) -> bool {
        self.class.is_system_error()
    }

    pub fn source(&self) -> &Option<BoxedError> {
        &self.source
    }
}

impl std::fmt::Display for DBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if let Some(ref source) = self.source {
            write!(f, "\n  Caused by: {}", source)?;
        }
        Ok(())
    }
}

impl Clone for DBError {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            message: self.message.clone(),
            source: None,
            class: self.class,
        }
    }
}

impl ErrorKind {
    fn default_class(&self) -> ErrorClass {
        match self {
            ErrorKind::Storage => ErrorClass::SystemError,
            ErrorKind::Query => ErrorClass::UserError,
            ErrorKind::Expression => ErrorClass::UserError,
            ErrorKind::Plan => ErrorClass::UserError,
            ErrorKind::Manager => ErrorClass::SystemError,
            ErrorKind::Validation => ErrorClass::UserError,
            ErrorKind::Io => ErrorClass::SystemError,
            ErrorKind::TypeDeduction => ErrorClass::UserError,
            ErrorKind::Serialization => ErrorClass::SystemError,
            ErrorKind::Index => ErrorClass::SystemError,
            ErrorKind::Transaction => ErrorClass::SystemError,
            ErrorKind::GraphService => ErrorClass::SystemError,
            ErrorKind::Internal => ErrorClass::SystemError,
            ErrorKind::Session => ErrorClass::UserError,
            ErrorKind::Auth => ErrorClass::UserError,
            ErrorKind::Permission => ErrorClass::UserError,
            ErrorKind::MemoryLimitExceeded => ErrorClass::SystemError,
            ErrorKind::Fulltext => ErrorClass::SystemError,
            ErrorKind::Coordinator => ErrorClass::SystemError,
            ErrorKind::Vector => ErrorClass::SystemError,
            ErrorKind::VectorCoordinator => ErrorClass::SystemError,
            ErrorKind::Search => ErrorClass::SystemError,
        }
    }
}

// ==================== Convenience Constructors ====================

impl DBError {
    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Storage, message)
    }

    pub fn query(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Query, message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Validation, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Io, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn transaction(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Transaction, message)
    }

    pub fn search(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Search, message)
    }

    pub fn expression(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Expression, message)
    }

    pub fn plan(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Plan, message)
    }

    pub fn manager(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Manager, message)
    }

    pub fn session(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Session, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Auth, message)
    }

    pub fn permission(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Permission, message)
    }

    pub fn fulltext(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Fulltext, message)
    }

    pub fn vector(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Vector, message)
    }

    pub fn index(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Index, message)
    }
}

// ==================== From Implementations ====================

impl From<serde_json::Error> for DBError {
    fn from(err: serde_json::Error) -> Self {
        DBError::new(ErrorKind::Serialization, err.to_string())
    }
}

impl From<std::io::Error> for DBError {
    fn from(err: std::io::Error) -> Self {
        DBError::io(err.to_string())
    }
}

impl From<query::QueryError> for DBError {
    fn from(err: query::QueryError) -> Self {
        DBError::query(err.to_string()).with_source(Box::new(err))
    }
}

impl From<storage::StorageError> for DBError {
    fn from(err: storage::StorageError) -> Self {
        let class = if err.is_retryable() {
            ErrorClass::Retryable
        } else {
            ErrorClass::SystemError
        };
        DBError::storage(err.to_string())
            .with_source(Box::new(err))
            .with_class(class)
    }
}

// ==================== ToPublicError Implementation ====================

impl ToPublicError for DBError {
    fn to_public_error(&self) -> PublicError {
        PublicError::new(self.to_error_code(), self.to_public_message())
    }

    fn to_error_code(&self) -> ErrorCode {
        match self.kind {
            ErrorKind::Storage => {
                if let Some(ref source) = self.source {
                    if let Some(se) = source.downcast_ref::<StorageError>() {
                        return se.to_error_code();
                    }
                }
                ErrorCode::InternalError
            }
            ErrorKind::Query => {
                if let Some(ref source) = self.source {
                    if let Some(qe) = source.downcast_ref::<QueryError>() {
                        return qe.to_error_code();
                    }
                }
                ErrorCode::ExecutionError
            }
            ErrorKind::Expression | ErrorKind::Plan => ErrorCode::ExecutionError,
            ErrorKind::Manager => {
                if let Some(ref source) = self.source {
                    if let Some(me) = source.downcast_ref::<ManagerError>() {
                        return me.to_error_code();
                    }
                }
                ErrorCode::InternalError
            }
            ErrorKind::Validation | ErrorKind::TypeDeduction => ErrorCode::ValidationError,
            ErrorKind::Io | ErrorKind::Serialization | ErrorKind::Index => ErrorCode::InternalError,
            ErrorKind::Transaction | ErrorKind::GraphService => ErrorCode::ExecutionError,
            ErrorKind::Internal => ErrorCode::InternalError,
            ErrorKind::Session => ErrorCode::Unauthorized,
            ErrorKind::Auth => ErrorCode::Unauthorized,
            ErrorKind::Permission => ErrorCode::PermissionDenied,
            ErrorKind::MemoryLimitExceeded => ErrorCode::ResourceExhausted,
            ErrorKind::Fulltext | ErrorKind::Coordinator => ErrorCode::ExecutionError,
            ErrorKind::Vector | ErrorKind::VectorCoordinator => ErrorCode::ExecutionError,
            ErrorKind::Search => ErrorCode::ExecutionError,
        }
    }

    fn to_public_message(&self) -> String {
        match self.kind {
            ErrorKind::Internal => "Internal server error".to_string(),
            ErrorKind::Io => "IO operation failed".to_string(),
            ErrorKind::Serialization => "Data serialization failed".to_string(),
            ErrorKind::Index => "Index operation failed".to_string(),
            ErrorKind::GraphService => "Graph service error".to_string(),
            _ => self.message.clone(),
        }
    }
}

// ==================== Result Type Aliases ====================

/// Harmonized result types
pub type DBResult<T> = Result<T, DBError>;

/// Type aliases for backward compatibility
pub type GraphDBResult<T> = DBResult<T>;

// ==================== Tests ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dberror_size() {
        assert!(
            std::mem::size_of::<DBError>() <= 64,
            "DBError should be small"
        );
    }

    #[test]
    fn test_dberror_creation() {
        let storage_err =
            StorageError::node_not_found(crate::core::types::VertexId::from_int64(42));
        let db_err: DBError = storage_err.into();
        assert_eq!(db_err.kind(), ErrorKind::Storage);
        assert!(!db_err.is_retryable());
    }

    #[test]
    fn test_error_conversion() {
        let query_err = QueryError::parse_error("test error");
        let db_err: DBError = query_err.into();
        assert_eq!(db_err.kind(), ErrorKind::Query);
        assert!(db_err.is_user_error());
    }

    #[test]
    fn test_error_class() {
        let err = DBError::validation("test");
        assert!(err.is_user_error());
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_retryable_error() {
        let storage_err = StorageError::lock_timeout("test".to_string());
        let db_err: DBError = storage_err.into();
        assert!(db_err.is_retryable());
    }

    #[test]
    fn test_error_display() {
        let err = DBError::query("test query error");
        let display = format!("{}", err);
        assert!(display.contains("query"));
        assert!(display.contains("test query error"));
    }
}

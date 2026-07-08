//! Storage layer error type
//!
//! Errors related to the underlying storage operations of the database.
//!
//! ## Design
//!
//! `StorageError` is a struct with boxed source error to keep size small (~24 bytes).
//! This follows the same pattern as `DBError` and `QueryError` for consistency.

use std::error::Error;

use super::BoxedError;
use crate::core::error::codes::{ErrorCode, PublicError, ToPublicError};

/// Storage layer result type
pub type StorageResult<T> = Result<T, StorageError>;

/// Storage error kind enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageErrorKind {
    DbError,
    StorageError,
    SerializeError,
    DeserializeError,
    NodeNotFound,
    EdgeNotFound,
    NotSupported,
    Conflict,
    LockTimeout,
    Deadlock,
    IOError,
    NotFound,
    AlreadyExists,
    InvalidInput,
    ParseError,
    VertexNotFound,
    VertexAlreadyExists,
    EdgeAlreadyExists,
    LabelNotFound,
    LabelAlreadyExists,
    PropertyNotFound,
    ColumnNotFound,
    ColumnAlreadyExists,
    StorageNotOpen,
    CapacityExceeded,
    NullValueNotAllowed,
    TypeMismatch,
    InvalidOffset,
    InvalidOperation,
    WalError,
    CompressError,
    DecompressError,
    DataCorruption,
}

impl StorageErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            StorageErrorKind::DbError => "db_error",
            StorageErrorKind::StorageError => "storage_error",
            StorageErrorKind::SerializeError => "serialize_error",
            StorageErrorKind::DeserializeError => "deserialize_error",
            StorageErrorKind::NodeNotFound => "node_not_found",
            StorageErrorKind::EdgeNotFound => "edge_not_found",
            StorageErrorKind::NotSupported => "not_supported",
            StorageErrorKind::Conflict => "conflict",
            StorageErrorKind::LockTimeout => "lock_timeout",
            StorageErrorKind::Deadlock => "deadlock",
            StorageErrorKind::IOError => "io_error",
            StorageErrorKind::NotFound => "not_found",
            StorageErrorKind::AlreadyExists => "already_exists",
            StorageErrorKind::InvalidInput => "invalid_input",
            StorageErrorKind::ParseError => "parse_error",
            StorageErrorKind::VertexNotFound => "vertex_not_found",
            StorageErrorKind::VertexAlreadyExists => "vertex_already_exists",
            StorageErrorKind::EdgeAlreadyExists => "edge_already_exists",
            StorageErrorKind::LabelNotFound => "label_not_found",
            StorageErrorKind::LabelAlreadyExists => "label_already_exists",
            StorageErrorKind::PropertyNotFound => "property_not_found",
            StorageErrorKind::ColumnNotFound => "column_not_found",
            StorageErrorKind::ColumnAlreadyExists => "column_already_exists",
            StorageErrorKind::StorageNotOpen => "storage_not_open",
            StorageErrorKind::CapacityExceeded => "capacity_exceeded",
            StorageErrorKind::NullValueNotAllowed => "null_value_not_allowed",
            StorageErrorKind::TypeMismatch => "type_mismatch",
            StorageErrorKind::InvalidOffset => "invalid_offset",
            StorageErrorKind::InvalidOperation => "invalid_operation",
            StorageErrorKind::WalError => "wal_error",
            StorageErrorKind::CompressError => "compress_error",
            StorageErrorKind::DecompressError => "decompress_error",
            StorageErrorKind::DataCorruption => "data_corruption",
        }
    }
}

/// Storage layer error type
///
/// Design principles:
/// 1. Small size: Uses boxed errors to keep struct size minimal (~24 bytes)
/// 2. Full context: Preserves error chain
/// 3. Clone support: Can be cloned for logging/propagation
#[derive(Debug)]
pub struct StorageError {
    kind: StorageErrorKind,
    message: String,
    source: Option<BoxedError>,
}

impl StorageError {
    pub fn new(kind: StorageErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, source: BoxedError) -> Self {
        self.source = Some(source);
        self
    }

    pub fn kind(&self) -> StorageErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn source(&self) -> &Option<BoxedError> {
        &self.source
    }

    fn from_boxed<E: Error + Send + Sync + 'static>(kind: StorageErrorKind, error: E) -> Self {
        Self {
            kind,
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            StorageErrorKind::LockTimeout | StorageErrorKind::Deadlock
        )
    }

    // Convenience constructors
    pub fn db_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::DbError, message)
    }

    #[allow(clippy::self_named_constructors)]
    pub fn storage_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::StorageError, message)
    }

    pub fn serialize_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::SerializeError, message)
    }

    pub fn deserialize_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::DeserializeError, message)
    }

    pub fn node_not_found(value: crate::core::types::VertexId) -> Self {
        Self::new(StorageErrorKind::NodeNotFound, format!("{}", value))
    }

    pub fn edge_not_found(value: crate::core::types::VertexId) -> Self {
        Self::new(StorageErrorKind::EdgeNotFound, format!("{}", value))
    }

    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::NotSupported, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::Conflict, message)
    }

    pub fn lock_timeout(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::LockTimeout, message)
    }

    pub fn deadlock() -> Self {
        Self::new(StorageErrorKind::Deadlock, "Deadlock detected")
    }

    pub fn io_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::IOError, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::NotFound, message)
    }

    pub fn already_exists(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::AlreadyExists, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::InvalidInput, message)
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::ParseError, message)
    }

    pub fn vertex_not_found() -> Self {
        Self::new(StorageErrorKind::VertexNotFound, "Vertex not found")
    }

    pub fn vertex_already_exists(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::VertexAlreadyExists, message)
    }

    pub fn edge_already_exists(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::EdgeAlreadyExists, message)
    }

    pub fn label_not_found(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::LabelNotFound, message)
    }

    pub fn label_already_exists(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::LabelAlreadyExists, message)
    }

    pub fn property_not_found(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::PropertyNotFound, message)
    }

    pub fn column_not_found(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::ColumnNotFound, message)
    }

    pub fn column_already_exists(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::ColumnAlreadyExists, message)
    }

    pub fn storage_not_open() -> Self {
        Self::new(StorageErrorKind::StorageNotOpen, "Storage not open")
    }

    pub fn capacity_exceeded() -> Self {
        Self::new(StorageErrorKind::CapacityExceeded, "Capacity exceeded")
    }

    pub fn null_value_not_allowed(column: impl Into<String>) -> Self {
        Self::new(
            StorageErrorKind::NullValueNotAllowed,
            format!("Null value not allowed for column: {}", column.into()),
        )
    }

    pub fn type_mismatch(expected: crate::core::DataType, actual: crate::core::DataType) -> Self {
        Self::new(
            StorageErrorKind::TypeMismatch,
            format!("Type mismatch: expected {:?}, got {:?}", expected, actual),
        )
    }

    pub fn invalid_offset(offset: u32) -> Self {
        Self::new(
            StorageErrorKind::InvalidOffset,
            format!("Invalid offset: {}", offset),
        )
    }

    pub fn invalid_operation(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::InvalidOperation, message)
    }

    pub fn wal_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::WalError, message)
    }

    pub fn compress_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::CompressError, message)
    }

    pub fn decompress_error(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::DecompressError, message)
    }

    pub fn data_corruption(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::DataCorruption, message)
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if let Some(ref source) = self.source {
            write!(f, "\n  Caused by: {}", source)?;
        }
        Ok(())
    }
}

impl Clone for StorageError {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            message: self.message.clone(),
            source: None,
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        Self::from_boxed(StorageErrorKind::IOError, e)
    }
}

impl From<String> for StorageError {
    fn from(s: String) -> Self {
        Self::db_error(s)
    }
}

impl From<&str> for StorageError {
    fn from(s: &str) -> Self {
        Self::db_error(s)
    }
}

impl<T> From<std::sync::PoisonError<T>> for StorageError {
    fn from(e: std::sync::PoisonError<T>) -> Self {
        Self::db_error(e.to_string())
    }
}

impl From<postcard::Error> for StorageError {
    fn from(e: postcard::Error) -> Self {
        Self::from_boxed(StorageErrorKind::SerializeError, e)
    }
}

impl From<crate::core::types::UndoLogError> for StorageError {
    fn from(e: crate::core::types::UndoLogError) -> Self {
        Self::db_error(e.to_string())
    }
}

impl ToPublicError for StorageError {
    fn to_public_error(&self) -> PublicError {
        PublicError::new(self.to_error_code(), self.to_public_message())
    }

    fn to_error_code(&self) -> ErrorCode {
        match self.kind {
            StorageErrorKind::NodeNotFound
            | StorageErrorKind::EdgeNotFound
            | StorageErrorKind::NotFound => ErrorCode::ResourceNotFound,
            StorageErrorKind::AlreadyExists => ErrorCode::ResourceAlreadyExists,
            StorageErrorKind::InvalidInput => ErrorCode::InvalidInput,
            StorageErrorKind::LockTimeout => ErrorCode::Timeout,
            StorageErrorKind::Deadlock => ErrorCode::Deadlock,
            StorageErrorKind::Conflict => ErrorCode::Conflict,
            StorageErrorKind::NotSupported => ErrorCode::InvalidStatement,
            _ => ErrorCode::InternalError,
        }
    }

    fn to_public_message(&self) -> String {
        match self.kind {
            StorageErrorKind::NodeNotFound => "Node does not exist".to_string(),
            StorageErrorKind::EdgeNotFound => "Edge does not exist".to_string(),
            StorageErrorKind::NotFound => format!("Resource not found: {}", self.message),
            StorageErrorKind::AlreadyExists => format!("Resource already exists: {}", self.message),
            _ => "Storage operation failed".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_error_size() {
        assert!(
            std::mem::size_of::<StorageError>() <= 64,
            "StorageError should be small, got {} bytes",
            std::mem::size_of::<StorageError>()
        );
    }

    #[test]
    fn test_storage_error_creation() {
        let err = StorageError::not_found("test resource");
        assert_eq!(err.kind(), StorageErrorKind::NotFound);
        assert!(err.message().contains("test resource"));
    }

    #[test]
    fn test_retryable_error() {
        let err = StorageError::lock_timeout("test");
        assert!(err.is_retryable());

        let err = StorageError::deadlock();
        assert!(err.is_retryable());

        let err = StorageError::not_found("test");
        assert!(!err.is_retryable());
    }
}

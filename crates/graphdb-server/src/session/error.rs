//! Session Error Type
//!
//! Covering session management related errors.
//!
//! ## Design
//!
//! `SessionError` is a struct with boxed source error to keep size small (~24 bytes).
//! This follows the same pattern as `DBError`, `QueryError`, and `StorageError` for consistency.

use std::error::Error;

use crate::core::error::codes::{ErrorCode, PublicError, ToPublicError};

/// Thread-safe boxed error type
type BoxedError = Box<dyn Error + Send + Sync>;

/// Session operation result type alias
pub type SessionResult<T> = Result<T, SessionError>;

/// Session error kind enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionErrorKind {
    SessionNotFound,
    SessionExpired,
    MaxConnectionsExceeded,
    QueryNotFound,
    KillSessionFailed,
    ManagerError,
    InsufficientPermission,
}

impl SessionErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionErrorKind::SessionNotFound => "session_not_found",
            SessionErrorKind::SessionExpired => "session_expired",
            SessionErrorKind::MaxConnectionsExceeded => "max_connections_exceeded",
            SessionErrorKind::QueryNotFound => "query_not_found",
            SessionErrorKind::KillSessionFailed => "kill_session_failed",
            SessionErrorKind::ManagerError => "manager_error",
            SessionErrorKind::InsufficientPermission => "insufficient_permission",
        }
    }
}

/// Session-related errors
///
/// Design principles:
/// 1. Small size: Uses boxed errors to keep struct size minimal (~24 bytes)
/// 2. Full context: Preserves error chain
/// 3. Clone support: Can be cloned for logging/propagation
#[derive(Debug)]
pub struct SessionError {
    kind: SessionErrorKind,
    message: String,
    source: Option<BoxedError>,
}

impl SessionError {
    pub fn new(kind: SessionErrorKind, message: impl Into<String>) -> Self {
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

    pub fn kind(&self) -> SessionErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    // Convenience constructors
    pub fn session_not_found(session_id: i64) -> Self {
        Self::new(
            SessionErrorKind::SessionNotFound,
            format!("Session not found: {}", session_id),
        )
    }

    pub fn session_expired() -> Self {
        Self::new(SessionErrorKind::SessionExpired, "Session expired")
    }

    pub fn max_connections_exceeded() -> Self {
        Self::new(
            SessionErrorKind::MaxConnectionsExceeded,
            "Maximum connections exceeded",
        )
    }

    pub fn query_not_found(query_id: u32) -> Self {
        Self::new(
            SessionErrorKind::QueryNotFound,
            format!("Query not found: {}", query_id),
        )
    }

    pub fn kill_session_failed(message: impl Into<String>) -> Self {
        Self::new(SessionErrorKind::KillSessionFailed, message)
    }

    pub fn manager_error(message: impl Into<String>) -> Self {
        Self::new(SessionErrorKind::ManagerError, message)
    }

    pub fn insufficient_permission() -> Self {
        Self::new(
            SessionErrorKind::InsufficientPermission,
            "Insufficient permission to perform this operation",
        )
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if let Some(ref source) = self.source {
            write!(f, "\n  Caused by: {}", source)?;
        }
        Ok(())
    }
}

impl Clone for SessionError {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            message: self.message.clone(),
            source: None,
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl ToPublicError for SessionError {
    fn to_public_error(&self) -> PublicError {
        PublicError::new(self.to_error_code(), self.to_public_message())
    }

    fn to_error_code(&self) -> ErrorCode {
        match self.kind {
            SessionErrorKind::SessionNotFound => ErrorCode::ResourceNotFound,
            SessionErrorKind::SessionExpired => ErrorCode::Unauthorized,
            SessionErrorKind::MaxConnectionsExceeded => ErrorCode::ResourceExhausted,
            SessionErrorKind::QueryNotFound => ErrorCode::ResourceNotFound,
            SessionErrorKind::KillSessionFailed => ErrorCode::InternalError,
            SessionErrorKind::ManagerError => ErrorCode::InternalError,
            SessionErrorKind::InsufficientPermission => ErrorCode::PermissionDenied,
        }
    }

    fn to_public_message(&self) -> String {
        self.message.clone()
    }
}

impl From<SessionError> for crate::core::error::DBError {
    fn from(e: SessionError) -> Self {
        let msg = e.to_string();
        crate::core::error::DBError::session(msg).with_source(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_error_size() {
        assert!(
            std::mem::size_of::<SessionError>() <= 64,
            "SessionError should be small, got {} bytes",
            std::mem::size_of::<SessionError>()
        );
    }

    #[test]
    fn test_session_error_creation() {
        let err = SessionError::session_not_found(123);
        assert_eq!(err.kind(), SessionErrorKind::SessionNotFound);
        assert!(err.message().contains("123"));
    }
}

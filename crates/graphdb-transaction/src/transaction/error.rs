//! Transaction Error Type
//!
//! Transaction management related errors.
//!
//! ## Design
//!
//! `TransactionError` is a struct with boxed source error to keep size small (~24 bytes).
//! This follows the same pattern as other error types for consistency.

use std::error::Error;

use super::types::{SavepointId, TransactionId, TransactionState};
use crate::core::error::BoxedError;

/// Transaction operation result type alias
pub type TransactionResult<T> = Result<T, TransactionError>;

/// Transaction error kind enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransactionErrorKind {
    BeginFailed,
    CommitFailed,
    AbortFailed,
    TransactionNotFound,
    SavepointFailed,
    SavepointNotFound,
    SavepointNotActive,
    NoSavepointsInTransaction,
    InvalidStateTransition,
    InvalidStateForCommit,
    InvalidStateForAbort,
    InvalidStateForExecution,
    TransactionTimeout,
    TransactionExpired,
    RollbackFailed,
    TooManyTransactions,
    ReadOnlyTransaction,
    WriteTransactionConflict,
    RecoveryFailed,
    PersistenceFailed,
    SerializationFailed,
    SyncFailed,
    Internal,
}

impl TransactionErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionErrorKind::BeginFailed => "begin_failed",
            TransactionErrorKind::CommitFailed => "commit_failed",
            TransactionErrorKind::AbortFailed => "abort_failed",
            TransactionErrorKind::TransactionNotFound => "transaction_not_found",
            TransactionErrorKind::SavepointFailed => "savepoint_failed",
            TransactionErrorKind::SavepointNotFound => "savepoint_not_found",
            TransactionErrorKind::SavepointNotActive => "savepoint_not_active",
            TransactionErrorKind::NoSavepointsInTransaction => "no_savepoints_in_transaction",
            TransactionErrorKind::InvalidStateTransition => "invalid_state_transition",
            TransactionErrorKind::InvalidStateForCommit => "invalid_state_for_commit",
            TransactionErrorKind::InvalidStateForAbort => "invalid_state_for_abort",
            TransactionErrorKind::InvalidStateForExecution => "invalid_state_for_execution",
            TransactionErrorKind::TransactionTimeout => "transaction_timeout",
            TransactionErrorKind::TransactionExpired => "transaction_expired",
            TransactionErrorKind::RollbackFailed => "rollback_failed",
            TransactionErrorKind::TooManyTransactions => "too_many_transactions",
            TransactionErrorKind::ReadOnlyTransaction => "read_only_transaction",
            TransactionErrorKind::WriteTransactionConflict => "write_transaction_conflict",
            TransactionErrorKind::RecoveryFailed => "recovery_failed",
            TransactionErrorKind::PersistenceFailed => "persistence_failed",
            TransactionErrorKind::SerializationFailed => "serialization_failed",
            TransactionErrorKind::SyncFailed => "sync_failed",
            TransactionErrorKind::Internal => "internal",
        }
    }
}

/// Transaction Error Type
///
/// Design principles:
/// 1. Small size: Uses boxed errors to keep struct size minimal (~24 bytes)
/// 2. Full context: Preserves error chain
/// 3. Clone support: Can be cloned for logging/propagation
#[derive(Debug)]
pub struct TransactionError {
    kind: TransactionErrorKind,
    message: String,
    source: Option<BoxedError>,
}

impl TransactionError {
    pub fn new(kind: TransactionErrorKind, message: impl Into<String>) -> Self {
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

    pub fn kind(&self) -> TransactionErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    fn from_boxed<E: Error + Send + Sync + 'static>(kind: TransactionErrorKind, error: E) -> Self {
        Self {
            kind,
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }

    // Convenience constructors
    pub fn begin_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::BeginFailed, message)
    }

    pub fn commit_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::CommitFailed, message)
    }

    pub fn abort_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::AbortFailed, message)
    }

    pub fn transaction_not_found(id: TransactionId) -> Self {
        Self::new(
            TransactionErrorKind::TransactionNotFound,
            format!("Transaction not found: {}", id),
        )
    }

    pub fn savepoint_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::SavepointFailed, message)
    }

    pub fn savepoint_not_found(id: SavepointId) -> Self {
        Self::new(
            TransactionErrorKind::SavepointNotFound,
            format!("Savepoint not found: {}", id),
        )
    }

    pub fn savepoint_not_active(id: SavepointId) -> Self {
        Self::new(
            TransactionErrorKind::SavepointNotActive,
            format!("Savepoint not active: {}", id),
        )
    }

    pub fn no_savepoints_in_transaction() -> Self {
        Self::new(
            TransactionErrorKind::NoSavepointsInTransaction,
            "No savepoints in transaction",
        )
    }

    pub fn invalid_state_transition(from: TransactionState, to: TransactionState) -> Self {
        Self::new(
            TransactionErrorKind::InvalidStateTransition,
            format!("Invalid state transition: from {} to {}", from, to),
        )
    }

    pub fn invalid_state_for_commit(state: TransactionState) -> Self {
        Self::new(
            TransactionErrorKind::InvalidStateForCommit,
            format!("Invalid state for commit: {}", state),
        )
    }

    pub fn invalid_state_for_abort(state: TransactionState) -> Self {
        Self::new(
            TransactionErrorKind::InvalidStateForAbort,
            format!("Invalid state for abort: {}", state),
        )
    }

    pub fn invalid_state_for_execution(state: TransactionState) -> Self {
        Self::new(
            TransactionErrorKind::InvalidStateForExecution,
            format!("Transaction not active, current state: {}", state),
        )
    }

    pub fn transaction_timeout() -> Self {
        Self::new(
            TransactionErrorKind::TransactionTimeout,
            "Transaction timeout",
        )
    }

    pub fn transaction_expired() -> Self {
        Self::new(
            TransactionErrorKind::TransactionExpired,
            "Transaction expired",
        )
    }

    pub fn rollback_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::RollbackFailed, message)
    }

    pub fn too_many_transactions() -> Self {
        Self::new(
            TransactionErrorKind::TooManyTransactions,
            "Too many concurrent transactions",
        )
    }

    pub fn read_only_transaction() -> Self {
        Self::new(
            TransactionErrorKind::ReadOnlyTransaction,
            "Read-only transaction",
        )
    }

    pub fn write_transaction_conflict() -> Self {
        Self::new(
            TransactionErrorKind::WriteTransactionConflict,
            "Write transaction conflict",
        )
    }

    pub fn recovery_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::RecoveryFailed, message)
    }

    pub fn persistence_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::PersistenceFailed, message)
    }

    pub fn serialization_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::SerializationFailed, message)
    }

    pub fn sync_failed(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::SyncFailed, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(TransactionErrorKind::Internal, message)
    }
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if let Some(ref source) = self.source {
            write!(f, "\n  Caused by: {}", source)?;
        }
        Ok(())
    }
}

impl Clone for TransactionError {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            message: self.message.clone(),
            source: None,
        }
    }
}

impl std::error::Error for TransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<std::io::Error> for TransactionError {
    fn from(e: std::io::Error) -> Self {
        Self::from_boxed(TransactionErrorKind::Internal, e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_error_size() {
        assert!(
            std::mem::size_of::<TransactionError>() <= 64,
            "TransactionError should be small, got {} bytes",
            std::mem::size_of::<TransactionError>()
        );
    }

    #[test]
    fn test_transaction_error_creation() {
        let err = TransactionError::transaction_not_found(TransactionId(123));
        assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
        assert!(err.message().contains("123"));
    }
}

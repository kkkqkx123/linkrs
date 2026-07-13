//! Transaction scope for query execution instances.
//!
//! Per the P2 spec (Section 5.3):
//! - Session-level `SessionTransactionController` holds explicit transaction
//!   handles.
//! - `QueryExecutionInstance::TransactionScope` borrows an explicit transaction
//!   or owns an auto-commit transaction.
//! - Begin/Commit/Rollback commands only trigger state transitions, not
//!   fake text results.
//! - Client disconnect rolls back auto-commit transactions owned by the
//!   instance, but does NOT end the session's explicit transaction.

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionResult;

// ── Transaction ID ──────────────────────────────────────────────────────────

/// Unique identifier for a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransactionId(pub u64);

impl std::fmt::Display for TransactionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "T{}", self.0)
    }
}

// ── TransactionScope ────────────────────────────────────────────────────────

/// The transaction scope for a single query execution.
///
/// - `Explicit`: the query runs within a session-level explicit transaction.
///   The scope borrows the transaction handle; it does not own it.
/// - `AutoCommit`: the instance owns an auto-commit transaction that is
///   committed (or rolled back) when execution completes.
/// - `None`: no transaction is active (read-only queries, DDL, etc.).
#[derive(Debug, Clone)]
pub enum TransactionScope {
    /// Running inside a session-level explicit transaction.
    Explicit {
        /// The explicit transaction ID.
        transaction_id: TransactionId,
        /// Whether this query can write (otherwise read-only).
        read_write: bool,
    },
    /// The query owns an auto-commit transaction.
    AutoCommit {
        /// Whether the transaction has been committed.
        committed: bool,
    },
    /// No transaction scope.
    None,
}

impl TransactionScope {
    /// Create an explicit transaction scope.
    pub fn explicit(transaction_id: TransactionId, read_write: bool) -> Self {
        Self::Explicit {
            transaction_id,
            read_write,
        }
    }

    /// Create an auto-commit scope.
    pub fn auto_commit() -> Self {
        Self::AutoCommit { committed: false }
    }

    /// Check whether a write operation is allowed in this scope.
    pub fn allows_write(&self) -> bool {
        match self {
            Self::Explicit { read_write, .. } => *read_write,
            Self::AutoCommit { .. } => true,
            Self::None => false,
        }
    }

    /// Check whether this is an explicit transaction.
    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::Explicit { .. })
    }

    /// Check whether this is an auto-commit transaction.
    pub fn is_auto_commit(&self) -> bool {
        matches!(self, Self::AutoCommit { .. })
    }

    /// Return the explicit transaction ID, if any.
    pub fn transaction_id(&self) -> Option<TransactionId> {
        match self {
            Self::Explicit { transaction_id, .. } => Some(*transaction_id),
            _ => None,
        }
    }
}

impl Default for TransactionScope {
    fn default() -> Self {
        Self::None
    }
}

// ── SessionTransactionController ────────────────────────────────────────────

/// Session-level controller for explicit transactions.
///
/// Tracks the currently active explicit transaction for a session, if any.
/// Query execution instances borrow the transaction handle from here.
///
/// Thread-safe: uses internal synchronization so it can be shared via `Arc`.
#[derive(Debug, Default)]
pub struct SessionTransactionController {
    state: RwLock<SessionTransactionState>,
}

/// Internal state for a session's transaction tracking.
#[derive(Debug, Clone)]
struct SessionTransactionState {
    /// Currently active explicit transaction, if any.
    active_transaction: Option<TransactionId>,
    /// Whether the active transaction is read-write.
    read_write: bool,
    /// Whether auto-commit is enabled outside explicit transactions.
    auto_commit: bool,
}

impl Default for SessionTransactionState {
    fn default() -> Self {
        Self {
            active_transaction: None,
            read_write: true,
            auto_commit: true,
        }
    }
}

impl SessionTransactionController {
    /// Create a new controller.
    pub fn new() -> Self {
        Self {
            state: RwLock::new(SessionTransactionState::default()),
        }
    }

    /// Begin an explicit transaction.
    pub fn begin(&self, transaction_id: TransactionId, read_write: bool) -> Result<(), QueryError> {
        let mut state = self.state.write();
        if state.active_transaction.is_some() {
            return Err(QueryError::execution(
                "A transaction is already active for this session".to_string(),
            ));
        }
        state.active_transaction = Some(transaction_id);
        state.read_write = read_write;
        Ok(())
    }

    /// Commit the active explicit transaction.
    pub fn commit(&self) -> Result<TransactionId, QueryError> {
        let mut state = self.state.write();
        state
            .active_transaction
            .take()
            .ok_or_else(|| QueryError::execution("No active transaction to commit".to_string()))
    }

    /// Roll back the active explicit transaction.
    pub fn rollback(&self) -> Result<TransactionId, QueryError> {
        let mut state = self.state.write();
        state
            .active_transaction
            .take()
            .ok_or_else(|| QueryError::execution("No active transaction to roll back".to_string()))
    }

    /// Get the current explicit transaction scope, if any.
    pub fn current_scope(&self) -> TransactionScope {
        let state = self.state.read();
        match state.active_transaction {
            Some(txn_id) => TransactionScope::Explicit {
                transaction_id: txn_id,
                read_write: state.read_write,
            },
            None => TransactionScope::None,
        }
    }

    /// Check whether an explicit transaction is active.
    pub fn is_active(&self) -> bool {
        self.state.read().active_transaction.is_some()
    }

    /// Enable or disable auto-commit.
    pub fn set_auto_commit(&self, enabled: bool) {
        self.state.write().auto_commit = enabled;
    }

    /// Whether auto-commit is enabled.
    pub fn auto_commit(&self) -> bool {
        self.state.read().auto_commit
    }
}

// ── Transaction command result ──────────────────────────────────────────────

/// Result of a transaction command (BEGIN, COMMIT, ROLLBACK).
#[derive(Debug, Clone)]
pub struct TransactionCommandResult {
    pub command: &'static str,
    pub transaction_id: Option<TransactionId>,
    pub message: String,
}

impl TransactionCommandResult {
    pub fn begin(transaction_id: TransactionId) -> Self {
        Self {
            command: "BEGIN",
            transaction_id: Some(transaction_id),
            message: "Transaction started".to_string(),
        }
    }

    pub fn commit(transaction_id: TransactionId) -> Self {
        Self {
            command: "COMMIT",
            transaction_id: Some(transaction_id),
            message: "Transaction committed".to_string(),
        }
    }

    pub fn rollback(transaction_id: TransactionId) -> Self {
        Self {
            command: "ROLLBACK",
            transaction_id: Some(transaction_id),
            message: "Transaction rolled back".to_string(),
        }
    }

    /// Convert to an [`ExecutionResult`].
    pub fn into_execution_result(self) -> ExecutionResult {
        let row = vec![
            crate::core::Value::String(self.command.to_string()),
            crate::core::Value::String(self.message),
        ];
        let col_names = vec!["command".to_string(), "result".to_string()];
        let dataset = crate::query::data_set::DataSet::from_rows(vec![row], col_names);
        ExecutionResult::DataSet { data: dataset }
    }
}

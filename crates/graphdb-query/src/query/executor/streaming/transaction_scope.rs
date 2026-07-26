//! Transaction scope for query execution instances.
//!
//! - Session-level `SessionTransactionController` holds explicit transaction
//!   handles and manages state transitions.
//! - `TransactionScope` is the execution view for a single query: it carries
//!   pure metadata (transaction identity, mode, rollback flag).
//! - Begin/Commit/Rollback commands trigger real state transitions through
//!   the controller, not fake text results.
//! - Client disconnect rolls back auto-commit transactions owned by the
//!   instance, but does NOT end the session's explicit transaction.
//!
//! The `SessionTransactionController` is the single state machine that all
//! transaction paths (SQL text, plan-based `TxnOperator`, embedded) flow through.
//! The API layer (`GraphService`) owns the `TransactionManager` reference and
//! performs the actual begin/commit/rollback; the controller tracks state.

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionResult;

pub use crate::core::types::TransactionId;

// ── CancelReason ───────────────────────────────────────────────────────────

/// Typed reason for query cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelReason {
    UserKill,
    Deadline,
    ClientDisconnect,
    MemoryLimit,
    WorkerFailure,
    Shutdown,
    Internal(String),
}

impl std::fmt::Display for CancelReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserKill => write!(f, "killed by user"),
            Self::Deadline => write!(f, "deadline exceeded"),
            Self::ClientDisconnect => write!(f, "client disconnected"),
            Self::MemoryLimit => write!(f, "memory limit exceeded"),
            Self::WorkerFailure => write!(f, "worker failure"),
            Self::Shutdown => write!(f, "system shutdown"),
            Self::Internal(msg) => write!(f, "{}", msg),
        }
    }
}

// ── TransactionScope ──────────────────────────────────────────────────────

/// The transaction scope for a single query execution.
///
/// Pure metadata — no back-reference to the controller. The caller (session /
/// query instance) holds both the scope and the controller separately.
///
/// - `ExplicitBorrowed`: the query runs within a session-level explicit
///   transaction.
/// - `AutoCommitOwned`: the instance owns an auto-commit transaction that is
///   committed (or rolled back) when execution completes.
/// - `ReadOnlySnapshot`: a read-only snapshot that does not need a write
///   transaction handle.
/// - `CommandScope`: transient scope for BEGIN / COMMIT / ROLLBACK state
///   transitions.  No real data access happens in this scope.
/// - `None`: no transaction is required (DDL, admin commands, etc.).
#[derive(Debug, Clone)]
pub enum TransactionScope {
    ExplicitBorrowed {
        transaction_id: TransactionId,
        read_write: bool,
    },
    AutoCommitOwned {
        transaction_id: TransactionId,
        rollback_only: bool,
    },
    ReadOnlySnapshot {
        transaction_id: TransactionId,
    },
    CommandScope,
    None,
}

impl TransactionScope {
    pub fn explicit(transaction_id: TransactionId, read_write: bool) -> Self {
        Self::ExplicitBorrowed {
            transaction_id,
            read_write,
        }
    }

    pub fn auto_commit(transaction_id: TransactionId) -> Self {
        Self::AutoCommitOwned {
            transaction_id,
            rollback_only: false,
        }
    }

    pub fn read_only(transaction_id: TransactionId) -> Self {
        Self::ReadOnlySnapshot { transaction_id }
    }

    /// Whether a write operation is allowed in this scope.
    pub fn allows_write(&self) -> bool {
        match self {
            Self::ExplicitBorrowed { read_write, .. } => *read_write,
            Self::AutoCommitOwned { rollback_only, .. } => !*rollback_only,
            _ => false,
        }
    }

    /// Whether this is an explicit (session-level) transaction.
    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::ExplicitBorrowed { .. })
    }

    /// Whether this is an auto-commit transaction.
    pub fn is_auto_commit(&self) -> bool {
        matches!(self, Self::AutoCommitOwned { .. })
    }

    /// Whether this scope expects at least one result row (command scopes do not).
    pub fn produces_result(&self) -> bool {
        !matches!(self, Self::CommandScope)
    }

    /// Return the transaction ID, if any.
    pub fn transaction_id(&self) -> Option<TransactionId> {
        match self {
            Self::ExplicitBorrowed { transaction_id, .. } => Some(*transaction_id),
            Self::AutoCommitOwned { transaction_id, .. } => Some(*transaction_id),
            Self::ReadOnlySnapshot { transaction_id } => Some(*transaction_id),
            _ => None,
        }
    }

    /// Validate that writes are allowed.  Returns an error if this scope
    /// forbids writes.
    pub fn ensure_can_write(&self) -> Result<(), QueryError> {
        if self.allows_write() {
            Ok(())
        } else {
            Err(QueryError::execution(
                "Current transaction scope does not allow writes".to_string(),
            ))
        }
    }

    /// Mark the transaction as rollback-only (auto-commit variants).
    pub fn mark_rollback_only(&mut self) {
        if let Self::AutoCommitOwned {
            ref mut rollback_only,
            ..
        } = self
        {
            *rollback_only = true;
        }
    }

    /// Whether the auto-commit transaction has been marked rollback-only.
    pub fn is_rollback_only(&self) -> bool {
        matches!(
            self,
            Self::AutoCommitOwned {
                rollback_only: true,
                ..
            }
        )
    }
}

// ── Transaction state machine ─────────────────────────────────────────────

/// Transaction state for session-level explicit transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// No transaction active.
    None,
    /// Transaction is active and accepting operations.
    Active,
    /// Transaction is committing (in-progress).
    Committing,
    /// Transaction has committed successfully.
    Committed,
    /// Transaction is rolling back (in-progress).
    RollingBack,
    /// Transaction has rolled back.
    RolledBack,
    /// Transaction has failed and can only be rolled back.
    RollbackOnly,
}

impl TransactionState {
    pub fn can_begin(&self) -> bool {
        matches!(self, Self::None | Self::Committed | Self::RolledBack)
    }

    pub fn can_execute(&self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn can_commit(&self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn can_rollback(&self) -> bool {
        matches!(self, Self::Active | Self::RollbackOnly)
    }
}

// ── SessionTransactionController ──────────────────────────────────────────

/// Session-level controller for explicit transactions.
///
/// Tracks the currently active explicit transaction for a session with
/// a proper state machine.  The controller validates state transitions but
/// does NOT own a [`TransactionManager`] reference — the caller (typically
/// the API layer) performs the actual begin/commit/rollback operations
/// on the manager and uses this controller for state tracking and scope
/// creation.
///
/// Thread-safe: uses internal synchronization so it can be shared via `Arc`.
#[derive(Debug, Default)]
pub struct SessionTransactionController {
    state: parking_lot::RwLock<SessionTransactionState>,
}

/// Internal state for a session's transaction tracking.
#[derive(Debug, Clone)]
struct SessionTransactionState {
    active_transaction: Option<TransactionId>,
    read_write: bool,
    auto_commit: bool,
    state: TransactionState,
    rollback_only: bool,
}

impl Default for SessionTransactionState {
    fn default() -> Self {
        Self {
            active_transaction: None,
            read_write: true,
            auto_commit: true,
            state: TransactionState::None,
            rollback_only: false,
        }
    }
}

impl SessionTransactionController {
    pub fn new() -> Self {
        Self {
            state: parking_lot::RwLock::new(SessionTransactionState::default()),
        }
    }

    // ── State queries ──

    /// Current state of the session's explicit transaction.
    pub fn state(&self) -> TransactionState {
        self.state.read().state
    }

    /// Whether an explicit transaction is active and can execute queries.
    pub fn is_active(&self) -> bool {
        let s = self.state.read();
        s.active_transaction.is_some() && s.state == TransactionState::Active
    }

    /// Whether auto-commit mode is enabled (outside explicit transactions).
    pub fn auto_commit(&self) -> bool {
        self.state.read().auto_commit
    }

    /// Whether the current transaction has been marked rollback-only.
    pub fn rollback_only(&self) -> bool {
        self.state.read().rollback_only
    }

    /// Return the active explicit transaction ID, if any.
    pub fn current_transaction_id(&self) -> Option<TransactionId> {
        self.state.read().active_transaction
    }

    // ── State transitions (tracking only — actual TM ops are caller's job) ──

    /// Begin tracking a new explicit transaction.
    ///
    /// The caller should have already created the transaction on the
    /// `TransactionManager` before calling this method.
    pub fn begin_tracking(
        &self,
        transaction_id: TransactionId,
        read_write: bool,
    ) -> Result<(), QueryError> {
        let mut state = self.state.write();
        if !state.state.can_begin() {
            return Err(QueryError::execution(format!(
                "Cannot BEGIN in state {:?}",
                state.state
            )));
        }
        state.active_transaction = Some(transaction_id);
        state.read_write = read_write;
        state.state = TransactionState::Active;
        state.rollback_only = false;
        Ok(())
    }

    /// Transition to committing state.  Returns the active transaction ID.
    ///
    /// After calling this, the caller should perform the actual commit
    /// on the `TransactionManager`, then call [`commit_finalize`] on success
    /// or [`fail_commit`] on error.
    pub fn begin_commit(&self) -> Result<TransactionId, QueryError> {
        let mut state = self.state.write();
        if !state.state.can_commit() {
            return Err(QueryError::execution(format!(
                "Cannot COMMIT in state {:?}",
                state.state
            )));
        }
        let txn_id = state
            .active_transaction
            .ok_or_else(|| QueryError::execution("No active transaction to commit".to_string()))?;
        state.state = TransactionState::Committing;
        Ok(txn_id)
    }

    /// Finalize a successful commit.
    pub fn commit_finalize(&self) {
        let mut state = self.state.write();
        state.active_transaction = None;
        state.state = TransactionState::Committed;
        state.rollback_only = false;
    }

    /// Mark commit as failed — transition to RollbackOnly.
    pub fn fail_commit(&self) {
        let mut state = self.state.write();
        state.state = TransactionState::RollbackOnly;
        state.rollback_only = true;
    }

    /// Transition to rolling-back state. Returns the active transaction ID.
    pub fn begin_rollback(&self) -> Result<TransactionId, QueryError> {
        let mut state = self.state.write();
        if !state.state.can_rollback() {
            return Err(QueryError::execution(format!(
                "Cannot ROLLBACK in state {:?}",
                state.state
            )));
        }
        let txn_id = state.active_transaction.ok_or_else(|| {
            QueryError::execution("No active transaction to roll back".to_string())
        })?;
        state.state = TransactionState::RollingBack;
        Ok(txn_id)
    }

    /// Finalize a successful rollback.
    pub fn rollback_finalize(&self) {
        let mut state = self.state.write();
        state.active_transaction = None;
        state.state = TransactionState::RolledBack;
        state.rollback_only = false;
    }

    /// Mark the current transaction as rollback-only.
    pub fn mark_rollback_only(&self) {
        let mut state = self.state.write();
        if state.state == TransactionState::Active {
            state.state = TransactionState::RollbackOnly;
            state.rollback_only = true;
        }
    }

    /// Enable or disable auto-commit mode.
    pub fn set_auto_commit(&self, enabled: bool) {
        self.state.write().auto_commit = enabled;
    }

    // ── Scope creation ──

    /// Get the current explicit `TransactionScope` for query binding.
    pub fn current_scope(&self) -> TransactionScope {
        let state = self.state.read();
        match state.active_transaction {
            Some(txn_id) if state.state == TransactionState::Active => {
                TransactionScope::ExplicitBorrowed {
                    transaction_id: txn_id,
                    read_write: state.read_write,
                }
            }
            _ => TransactionScope::None,
        }
    }

    /// Create a [`TransactionScope::CommandScope`] for transaction control
    /// statements (BEGIN / COMMIT / ROLLBACK).
    pub fn command_scope() -> TransactionScope {
        TransactionScope::CommandScope
    }
}

// ── Transaction command result ─────────────────────────────────────────────

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
            crate::core::Value::string(self.command),
            crate::core::Value::string(self.message),
        ];
        let col_names = vec!["command".to_string(), "result".to_string()];
        let dataset = crate::query::data_set::DataSet::from_rows(vec![row], col_names);
        ExecutionResult::DataSet { data: dataset }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_allows_write() {
        let scope = TransactionScope::explicit(TransactionId(1), true);
        assert!(scope.allows_write());
        assert!(scope.is_explicit());
        assert_eq!(scope.transaction_id(), Some(TransactionId(1)));
    }

    #[test]
    fn test_read_only_scope_does_not_allow_write() {
        let scope = TransactionScope::read_only(TransactionId(2));
        assert!(!scope.allows_write());
        assert!(!scope.is_explicit());
        assert!(!scope.is_auto_commit());
    }

    #[test]
    fn test_auto_commit_scope_allows_write() {
        let scope = TransactionScope::auto_commit(TransactionId(3));
        assert!(scope.allows_write());
        assert!(scope.is_auto_commit());
    }

    #[test]
    fn test_rollback_only_prevents_write() {
        let mut scope = TransactionScope::auto_commit(TransactionId(4));
        assert!(scope.allows_write());
        scope.mark_rollback_only();
        assert!(!scope.allows_write());
        assert!(scope.is_rollback_only());
    }

    #[test]
    fn test_none_scope() {
        let scope = TransactionScope::None;
        assert!(!scope.allows_write());
        assert!(!scope.is_explicit());
        assert!(!scope.is_auto_commit());
        assert!(scope.transaction_id().is_none());
    }

    #[test]
    fn test_command_scope() {
        let scope = TransactionScope::CommandScope;
        assert!(!scope.produces_result());
    }

    #[test]
    fn test_ensure_can_write() {
        let write_scope = TransactionScope::explicit(TransactionId(1), true);
        assert!(write_scope.ensure_can_write().is_ok());

        let read_scope = TransactionScope::read_only(TransactionId(2));
        assert!(read_scope.ensure_can_write().is_err());
    }

    #[test]
    fn test_session_controller_state() {
        let ctrl = SessionTransactionController::new();
        assert_eq!(ctrl.state(), TransactionState::None);
        assert!(!ctrl.is_active());
        assert!(ctrl.auto_commit());
    }

    #[test]
    fn test_session_controller_tracking() {
        let ctrl = SessionTransactionController::new();
        assert!(ctrl.begin_tracking(TransactionId(1), true).is_ok());
        assert_eq!(ctrl.state(), TransactionState::Active);
        assert!(ctrl.is_active());
        assert_eq!(ctrl.current_transaction_id(), Some(TransactionId(1)));

        let txn_id = ctrl.begin_commit().unwrap();
        assert_eq!(txn_id, TransactionId(1));
        assert_eq!(ctrl.state(), TransactionState::Committing);
        ctrl.commit_finalize();
        assert_eq!(ctrl.state(), TransactionState::Committed);
        assert!(!ctrl.is_active());
    }

    #[test]
    fn test_session_controller_rollback() {
        let ctrl = SessionTransactionController::new();
        ctrl.begin_tracking(TransactionId(2), false).unwrap();
        assert!(ctrl.is_active());

        let txn_id = ctrl.begin_rollback().unwrap();
        assert_eq!(txn_id, TransactionId(2));
        ctrl.rollback_finalize();
        assert_eq!(ctrl.state(), TransactionState::RolledBack);
    }

    #[test]
    fn test_session_controller_cannot_double_begin() {
        let ctrl = SessionTransactionController::new();
        ctrl.begin_tracking(TransactionId(1), true).unwrap();
        let result = ctrl.begin_tracking(TransactionId(2), true);
        assert!(result.is_err());
    }

    #[test]
    fn test_session_controller_rollback_only() {
        let ctrl = SessionTransactionController::new();
        ctrl.begin_tracking(TransactionId(1), true).unwrap();
        ctrl.mark_rollback_only();
        assert!(ctrl.rollback_only());
        assert_eq!(ctrl.state(), TransactionState::RollbackOnly);

        // Cannot commit a rollback-only transaction
        assert!(ctrl.begin_commit().is_err());
        // Can only rollback
        assert!(ctrl.begin_rollback().is_ok());
    }

    #[test]
    fn test_current_scope() {
        let ctrl = SessionTransactionController::new();
        assert!(matches!(ctrl.current_scope(), TransactionScope::None));

        ctrl.begin_tracking(TransactionId(42), true).unwrap();
        let scope = ctrl.current_scope();
        assert!(scope.is_explicit());
        assert!(scope.allows_write());
        assert_eq!(scope.transaction_id(), Some(TransactionId(42)));
    }

    #[test]
    fn test_cancel_reason_display() {
        assert_eq!(CancelReason::UserKill.to_string(), "killed by user");
        assert_eq!(CancelReason::Deadline.to_string(), "deadline exceeded");
        assert_eq!(CancelReason::Shutdown.to_string(), "system shutdown");
        assert_eq!(
            CancelReason::Internal("test error".to_string()).to_string(),
            "test error"
        );
    }

    #[test]
    fn test_transaction_state_transitions() {
        assert!(TransactionState::None.can_begin());
        assert!(TransactionState::Committed.can_begin());
        assert!(TransactionState::RolledBack.can_begin());
        assert!(!TransactionState::Active.can_begin());

        assert!(TransactionState::Active.can_execute());
        assert!(!TransactionState::None.can_execute());
        assert!(!TransactionState::Committed.can_execute());

        assert!(TransactionState::Active.can_commit());
        assert!(!TransactionState::None.can_commit());

        assert!(TransactionState::Active.can_rollback());
        assert!(TransactionState::RollbackOnly.can_rollback());
        assert!(!TransactionState::None.can_rollback());
    }
}

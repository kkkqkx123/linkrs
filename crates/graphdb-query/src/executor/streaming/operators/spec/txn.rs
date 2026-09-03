//! Immutable configuration for transaction operators.

/// Immutable config for transaction operators.
///
/// The actual transaction state transitions are performed by the
/// [`SessionTransactionController`] at execution time.
#[derive(Debug, Clone)]
pub enum TxnSpec {
    BeginTransaction,
    Commit,
    Rollback,
    /// Roll back to a savepoint: validates the controller is in `Active`
    /// state but does NOT transition out of it.
    RollbackToSavepoint {
        name: String,
    },
    /// Create a savepoint (validation only — the TransactionManager
    /// operation is performed by the API layer beforehand).
    Savepoint {
        name: String,
    },
    /// Release a savepoint (validation only — the TransactionManager
    /// operation is performed by the API layer beforehand).
    ReleaseSavepoint {
        name: String,
    },
}

//! Transaction Configuration Types
//!
//! Provides shared configuration types for transaction management.

use std::fmt;

/// Durability level for transactions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DurabilityLevel {
    /// No durability - data lost on crash
    None,
    /// Async WAL - may lose recent transactions on crash
    Async,
    /// Sync WAL - guaranteed durability (was Immediate in legacy code)
    #[default]
    Sync,
}

/// Transaction Isolation Level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionIsolationLevel {
    /// Repeatable Read - all statements in the transaction see a snapshot as of the start of the transaction
    ///
    /// Note: this applies to explicit transactions driven through the
    /// `TransactionManager`. Auto-commit DML statements run as single-statement
    /// transactions that bypass the manager and are serialized by the storage
    /// write gate; a failed auto-commit statement is rolled back via before-image
    /// undo. Within a single auto-commit statement the read snapshot is stable,
    /// but property updates are physically overwritten (no MVCC version chain),
    /// so two statements racing on the same entity observe each other's writes.
    #[default]
    RepeatableRead,
    /// Read Committed - each statement sees the latest committed snapshot.
    ReadCommitted,
    /// Serializable - certify read and write dependencies at commit.
    Serializable,
}

impl fmt::Display for TransactionIsolationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionIsolationLevel::RepeatableRead => write!(f, "REPEATABLE READ"),
            TransactionIsolationLevel::ReadCommitted => write!(f, "READ COMMITTED"),
            TransactionIsolationLevel::Serializable => write!(f, "SERIALIZABLE"),
        }
    }
}

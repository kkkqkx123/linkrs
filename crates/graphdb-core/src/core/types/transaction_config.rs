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
    #[default]
    Async,
    /// Sync WAL - guaranteed durability (was Immediate in legacy code)
    Sync,
}

/// Transaction Isolation Level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionIsolationLevel {
    /// Repeatable Read - all statements in the transaction see a snapshot as of the start of the transaction
    #[default]
    RepeatableRead,
    /// Read Committed - each statement sees the latest committed snapshot.
    ReadCommitted,
    /// Serializable - certify read and write dependencies at commit.
    Serializable,
    /// Read Uncommitted - dirty reads allowed, no snapshot isolation.
    ReadUncommitted,
}

impl fmt::Display for TransactionIsolationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionIsolationLevel::RepeatableRead => write!(f, "REPEATABLE READ"),
            TransactionIsolationLevel::ReadCommitted => write!(f, "READ COMMITTED"),
            TransactionIsolationLevel::Serializable => write!(f, "SERIALIZABLE"),
            TransactionIsolationLevel::ReadUncommitted => write!(f, "READ UNCOMMITTED"),
        }
    }
}

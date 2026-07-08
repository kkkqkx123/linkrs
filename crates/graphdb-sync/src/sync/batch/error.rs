use thiserror::Error;

use crate::search::SearchError;
#[cfg(feature = "fulltext-search")]
use crate::sync::coordinator::CoordinatorError;

#[derive(Debug, Error)]
pub enum BatchError {
    #[error("Buffer overflow: {0}")]
    BufferOverflow(String),

    #[error("Queue is full")]
    QueueFull,

    #[error("Queue is closed")]
    QueueClosed,

    #[cfg(feature = "fulltext-search")]
    #[error("Coordinator error: {0}")]
    CoordinatorError(#[from] CoordinatorError),

    #[error("Commit error: {0}")]
    CommitError(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Transaction error: {0}")]
    TransactionError(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Search engine error: {0}")]
    SearchError(#[from] SearchError),
}

pub type BatchResult<T> = Result<T, BatchError>;

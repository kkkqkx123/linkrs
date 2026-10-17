use thiserror::Error;

#[derive(Debug, Error)]
pub enum BatchError {
    #[error("Buffer overflow: {0}")]
    BufferOverflow(String),

    #[error("Queue is full")]
    QueueFull,

    #[error("Queue is closed")]
    QueueClosed,

    #[error("Commit error: {0}")]
    CommitError(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Transaction error: {0}")]
    TransactionError(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    /// Engine-specific error message from the underlying search engine.
    /// The engine type is encapsulated within the processor; only the
    /// human-readable message crosses the BatchError boundary.
    #[error("Engine error: {0}")]
    EngineError(String),
}

pub type BatchResult<T> = Result<T, BatchError>;

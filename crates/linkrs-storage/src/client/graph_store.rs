use super::{StorageCommitOps, StorageOperationContextOps, StorageReader, StorageWriter};
use crate::UndoTarget;

/// Logical graph data access used by query execution.
pub trait GraphStore:
    StorageReader
    + StorageWriter
    + StorageOperationContextOps
    + StorageCommitOps
    + UndoTarget
    + Send
    + Sync
    + std::fmt::Debug
{
}

impl<T> GraphStore for T where
    T: StorageReader
        + StorageWriter
        + StorageOperationContextOps
        + StorageCommitOps
        + UndoTarget
        + Send
        + Sync
        + std::fmt::Debug
{
}

//! Auto-commit batch window operations.
//!
//! Storage engines that support a shared [`AutoCommitBatchWindow`] implement
//! this trait so batch loaders can acquire the auto-commit write gate and
//! register MVCC snapshots once for a run of statements.

use crate::core::StorageResult;
use crate::storage::engine::graph_storage::AutoCommitBatchWindow;
use std::sync::Arc;

/// Operations for running a run of auto-commit DML statements inside a shared
/// batch window.
pub trait AutoCommitBatchOps: Send + Sync {
    fn begin_auto_commit_batch(&self) -> StorageResult<Arc<AutoCommitBatchWindow>>;

    fn bind_auto_commit_statement(
        &self,
        window: &Arc<AutoCommitBatchWindow>,
    ) -> StorageResult<Self>
    where
        Self: Sized;

    fn finalize_auto_commit_batch(&self, window: &AutoCommitBatchWindow) -> StorageResult<()>;
}

/// Group-commit operations on top of an [`AutoCommitBatchWindow`].
/// The window is shared with `AutoCommitBatchOps`; `bind_auto_commit_statement`
/// is mode-aware: bound inside a group window it reuses the shared write
/// timestamp and undo log, so no separate bind method is needed.
pub trait AutoCommitGroupOps: Send + Sync {
    /// Open a group window (gate + shared snapshots + shared write timestamp).
    fn begin_auto_commit_group(&self) -> StorageResult<Arc<AutoCommitBatchWindow>>;
    /// Single group commit point: one fsync, barrier advance, one ts commit.
    fn finalize_auto_commit_group(&self, window: &AutoCommitBatchWindow) -> StorageResult<()>;
}

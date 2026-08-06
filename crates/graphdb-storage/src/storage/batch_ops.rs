//! Auto-commit batch window operations (P4/P6).
//!
//! Storage engines that support a shared [`AutoCommitBatchWindow`] implement
//! this trait so batch loaders (embedded `load_gql_file`, server-side batch
//! endpoints, CLI `LOAD`) can acquire the auto-commit write gate and register
//! MVCC snapshots once for a run of statements instead of once per statement.
//!
//! The window is storage-agnostic: each statement bound through it still
//! allocates its own write timestamp / transaction id / undo log and commits
//! or rolls back independently.

use crate::core::StorageResult;
use crate::storage::engine::graph_storage::AutoCommitBatchWindow;
use std::sync::Arc;

/// Operations for running a run of auto-commit DML statements inside a shared
/// batch window.
pub trait AutoCommitBatchOps: Send + Sync {
    /// Open a batch window on the pristine base storage. Acquires the
    /// auto-commit write gate; MVCC snapshots register lazily on the first
    /// [`bind_auto_commit_statement`](Self::bind_auto_commit_statement).
    fn begin_auto_commit_batch(&self) -> StorageResult<Arc<AutoCommitBatchWindow>>;

    /// Bind one auto-commit statement inside `window`, reusing the window's
    /// write-gate lease and MVCC snapshot registrations. Each bound storage
    /// must be finalized (`finalize_operation`) per statement.
    fn bind_auto_commit_statement(
        &self,
        window: &Arc<AutoCommitBatchWindow>,
    ) -> StorageResult<Self>
    where
        Self: Sized;

    /// Finalize the batch window: unregister its MVCC snapshots and release
    /// the write gate. Idempotent.
    fn finalize_auto_commit_batch(&self, window: &AutoCommitBatchWindow) -> StorageResult<()>;
}

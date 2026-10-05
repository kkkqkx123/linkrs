//! Undo Operation Types
//!
//! Provides the core trait and error types for transaction undo/rollback operations.

use super::storage_ids::{
    ColumnId, EdgeDeletionContext, EdgeId, EdgeIdentifier, EdgeKey, LabelId, Timestamp,
    TransactionId, VertexId, VertexIdentifier,
};
use crate::Value;
use std::sync::Arc;

/// Undo log error
#[derive(Debug, Clone, thiserror::Error)]
pub enum UndoLogError {
    #[error("Undo operation failed: {0}")]
    UndoFailed(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Label not found: {0}")]
    LabelNotFound(LabelId),

    #[error("Vertex not found: {0}")]
    VertexNotFound(VertexId),

    #[error("Edge not found: {0}")]
    EdgeNotFound(EdgeId),

    #[error("Property not found: {0}")]
    PropertyNotFound(String),
}

/// Undo log result type
pub type UndoLogResult<T> = Result<T, UndoLogError>;

/// Boundary of storage-staged (not yet undo-logged) writes captured at
/// savepoint creation.
///
/// Online vertex writes accumulate in a per-transaction staging buffer
/// (plus staged WAL redo) instead of the undo log, so rolling back to a
/// savepoint must rewind this buffer in addition to executing undo logs.
/// `None` at a savepoint means the target held no staged writes and the
/// rollback skips the rewind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedWriteMark {
    /// Staging-buffer journal length at the savepoint.
    pub staging_journal_len: usize,
    /// Staging-buffer index-operation length at the savepoint.
    pub staging_index_len: usize,
    /// Staged WAL redo length at the savepoint.
    pub staged_wal_len: usize,
}

impl StagedWriteMark {
    /// An all-zero mark rewinds nothing.
    pub fn is_empty(self) -> bool {
        self.staging_journal_len == 0 && self.staging_index_len == 0 && self.staged_wal_len == 0
    }
}

/// Target for undo operations
pub trait UndoTarget: Send + Sync {
    fn delete_vertex_type(&self, label: LabelId) -> UndoLogResult<()>;
    fn delete_edge_type(&self, edge_key: EdgeKey) -> UndoLogResult<()>;
    fn delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()>;
    fn delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()>;
    /// Restore an edge that was deleted by a transaction.
    ///
    /// Implementations that cannot restore full edge properties may keep the
    /// default error and reject undo rather than silently losing data.
    fn restore_edge(
        &self,
        _edge: EdgeIdentifier,
        _properties: Vec<(Arc<str>, Value)>,
        _ts: Timestamp,
    ) -> UndoLogResult<()> {
        Err(UndoLogError::UndoFailed(
            "Edge restoration is not supported by this undo target".to_string(),
        ))
    }
    fn undo_update_edge_property(
        &self,
        edge_id: EdgeIdentifier,
        col_id: ColumnId,
        value: Value,
        ts: Timestamp,
    ) -> UndoLogResult<()>;
    fn revert_delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()>;
    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[Arc<str>],
    ) -> UndoLogResult<()>;
    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[Arc<str>],
    ) -> UndoLogResult<()>;
    fn revert_delete_vertex_label(&self, label_name: &str) -> UndoLogResult<()>;
    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> UndoLogResult<()>;
    fn revert_rename_vertex_properties(
        &self,
        label_name: &str,
        current_names: &[Arc<str>],
        original_names: &[Arc<str>],
    ) -> UndoLogResult<()>;
    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[Arc<str>],
        original_names: &[Arc<str>],
    ) -> UndoLogResult<()>;
    /// Capture this target's staged-write boundary for `txn_id`.
    ///
    /// Returns `None` when the target holds no staged writes for the
    /// transaction. Targets that stage uncommitted writes outside the undo
    /// log must override this and [`Self::rollback_staged_writes`]; every
    /// other implementor reports `None`.
    fn staged_write_mark(&self, txn_id: TransactionId) -> Option<StagedWriteMark>;
    /// Rewind staged writes of `txn_id` to a mark previously captured by
    /// [`Self::staged_write_mark`].
    ///
    /// Called only with marks this target produced. An all-zero mark is a
    /// no-op; anything else a non-staging target receives is rejected
    /// loudly instead of leaving staged rows behind.
    fn rollback_staged_writes(
        &self,
        txn_id: TransactionId,
        mark: StagedWriteMark,
    ) -> UndoLogResult<()>;
}

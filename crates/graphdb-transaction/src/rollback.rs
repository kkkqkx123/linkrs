//! Transaction Rollback Module
//!
//! Provides rollback functionality for transactions using UndoLog mechanisms.

use crate::undo_log::{UndoLogEntry, UndoTarget};
use graphdb_core::types::{ColumnId, LabelId, Timestamp, VertexId};
use graphdb_core::StorageError;

pub use crate::undo_log::{
    CreateEdgeTypeUndo, CreateVertexTypeUndo, InsertEdgeUndo, RemoveEdgeUndo, UpdateEdgePropUndo,
};

/// Undo log context trait
///
/// Defines the basic operations required for undo log rollbacks.
/// This is the primary rollback mechanism for NeuG architecture.
pub(crate) trait UndoLogContext {
    fn execute_undo_logs<T: UndoTarget + ?Sized>(&self, target: &T) -> Result<(), StorageError>;
    fn clear_undo_logs(&self) -> Result<(), StorageError>;
}

impl UndoLogContext for crate::context::TransactionContext {
    fn execute_undo_logs<T: UndoTarget + ?Sized>(&self, target: &T) -> Result<(), StorageError> {
        self.execute_undo_logs(target)
            .map_err(|e| StorageError::db_error(e.to_string()))
    }

    fn clear_undo_logs(&self) -> Result<(), StorageError> {
        self.clear_undo_logs()
            .map_err(|error| StorageError::db_error(error.to_string()))
    }
}

/// Undo Log Rollback Processor
///
/// Primary rollback mechanism for NeuG architecture.
/// Uses UndoLog entries to reverse operations during transaction abort.
pub(crate) struct UndoLogRollback<'a, T: UndoLogContext> {
    ctx: &'a T,
}

impl<'a, T: UndoLogContext> UndoLogRollback<'a, T> {
    pub fn new(ctx: &'a T) -> Self {
        Self { ctx }
    }

    pub fn execute_rollback<U: UndoTarget + ?Sized>(
        &self,
        target: &mut U,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        self.ctx.execute_undo_logs(target)
    }

    pub fn clear_logs(&self) -> Result<(), StorageError> {
        self.ctx.clear_undo_logs()
    }
}

/// Rollback helper functions
///
/// Factory for creating undo log entries.
/// Used by transactions to record rollback information.
pub struct RollbackHelper;

/// Parameters for create_update_edge_prop_undo operation
pub struct CreateUpdateEdgePropUndoParams {
    pub src_label: LabelId,
    pub src_vid: u64,
    pub dst_label: LabelId,
    pub dst_vid: u64,
    pub edge_label: LabelId,
    pub rank: i64,
    pub col_id: ColumnId,
    pub old_value: graphdb_core::Value,
}

/// Parameters for create_remove_edge_undo operation
pub struct CreateRemoveEdgeUndoParams {
    pub src_label: LabelId,
    pub src_vid: u64,
    pub dst_label: LabelId,
    pub dst_vid: u64,
    pub edge_label: LabelId,
    pub rank: i64,
}

impl RollbackHelper {
    pub fn create_update_edge_prop_undo(params: CreateUpdateEdgePropUndoParams) -> UndoLogEntry {
        UndoLogEntry::UpdateEdgeProp(UpdateEdgePropUndo {
            src_label: params.src_label,
            src_vid: VertexId::from_u64(params.src_vid),
            dst_label: params.dst_label,
            dst_vid: VertexId::from_u64(params.dst_vid),
            edge_label: params.edge_label,
            rank: params.rank,
            col_id: params.col_id,
            old_value: params.old_value,
        })
    }

    pub fn create_remove_edge_undo(params: CreateRemoveEdgeUndoParams) -> UndoLogEntry {
        UndoLogEntry::RemoveEdge(RemoveEdgeUndo {
            src_label: params.src_label,
            src_vid: VertexId::from_u64(params.src_vid),
            dst_label: params.dst_label,
            dst_vid: VertexId::from_u64(params.dst_vid),
            edge_label: params.edge_label,
            rank: params.rank,
        })
    }

    /// Reserved for future use: DDL create vertex type undo
    pub fn create_create_vertex_type_undo(label: LabelId) -> UndoLogEntry {
        UndoLogEntry::CreateVertexType(CreateVertexTypeUndo { vertex_type: label })
    }

    /// Reserved for future use: DDL create edge type undo
    pub fn create_create_edge_type_undo(
        src_type: LabelId,
        dst_type: LabelId,
        edge_type: LabelId,
    ) -> UndoLogEntry {
        UndoLogEntry::CreateEdgeType(CreateEdgeTypeUndo {
            src_type,
            dst_type,
            edge_type,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UndoLogManager;

    struct MockUndoContext {
        logs: std::cell::RefCell<UndoLogManager>,
    }

    impl MockUndoContext {
        fn new() -> Self {
            Self {
                logs: std::cell::RefCell::new(UndoLogManager::new()),
            }
        }
    }

    impl UndoLogContext for MockUndoContext {
        fn execute_undo_logs<T: UndoTarget + ?Sized>(
            &self,
            _target: &T,
        ) -> Result<(), StorageError> {
            self.logs
                .borrow_mut()
                .clear()
                .map_err(|error| StorageError::db_error(error.to_string()))
        }

        fn clear_undo_logs(&self) -> Result<(), StorageError> {
            self.logs
                .borrow_mut()
                .clear()
                .map_err(|error| StorageError::db_error(error.to_string()))
        }
    }

    #[test]
    fn test_undo_log_rollback() {
        let ctx = MockUndoContext::new();
        let rollback = UndoLogRollback::new(&ctx);

        assert_eq!(ctx.logs.borrow().len(), 0);
        ctx.logs
            .borrow_mut()
            .add(RollbackHelper::create_remove_edge_undo(
                CreateRemoveEdgeUndoParams {
                    src_label: 1,
                    src_vid: 100,
                    dst_label: 2,
                    dst_vid: 200,
                    edge_label: 3,
                    rank: 0,
                },
            ))
            .expect("Failed to append undo log");
        assert_eq!(ctx.logs.borrow().len(), 1);

        rollback.clear_logs().expect("Failed to clear undo log");
        assert_eq!(ctx.logs.borrow().len(), 0);
    }

    #[test]
    fn test_rollback_helper() {
        let undo = RollbackHelper::create_remove_edge_undo(CreateRemoveEdgeUndoParams {
            src_label: 1,
            src_vid: 100,
            dst_label: 2,
            dst_vid: 200,
            edge_label: 3,
            rank: 0,
        });
        assert!(undo.description().contains("RemoveEdgeUndo"));

        let undo = RollbackHelper::create_update_edge_prop_undo(CreateUpdateEdgePropUndoParams {
            src_label: 1,
            src_vid: 100,
            dst_label: 2,
            dst_vid: 200,
            edge_label: 3,
            rank: 0,
            col_id: ColumnId(0),
            old_value: graphdb_core::Value::BigInt(42),
        });
        assert!(undo.description().contains("UpdateEdgePropUndo"));
    }
}

//! Transaction Rollback Module
//!
//! Provides rollback functionality for transactions using both OperationLog and UndoLog mechanisms.
//! The UndoLog-based rollback is the recommended approach for NeuG architecture.

use crate::core::types::{ColumnId, LabelId, Timestamp, VertexId};
use crate::core::StorageError;
use crate::transaction::undo_log::{UndoLogEntry, UndoTarget};

pub use crate::transaction::undo_log::{
    AddEdgePropUndo, AddVertexPropUndo, CreateEdgeTypeUndo, CreateVertexTypeUndo,
    DeleteEdgePropUndo, DeleteEdgeTypeUndo, DeleteVertexPropUndo, DeleteVertexTypeUndo,
    InsertEdgeUndo, InsertVertexUndo, PropertyValue, RelatedEdgeInfo, RemoveEdgeUndo,
    RemoveVertexUndo, RenameEdgePropUndo, RenameVertexPropUndo, UpdateEdgePropUndo,
    UpdateVertexPropUndo,
};

/// Operation logging context trait
///
/// Define the basic operations required for operation log rollbacks.
/// This is used for savepoint rollback functionality.
pub(crate) trait OperationLogContext {
    fn operation_log_len(&self) -> usize;
    fn truncate_operation_log(&self, index: usize);
}

impl OperationLogContext for crate::transaction::context::TransactionContext {
    fn operation_log_len(&self) -> usize {
        self.operation_log_len()
    }

    fn truncate_operation_log(&self, index: usize) {
        self.truncate_operation_log(index);
    }
}

/// Undo log context trait
///
/// Defines the basic operations required for undo log rollbacks.
/// This is the primary rollback mechanism for NeuG architecture.
pub(crate) trait UndoLogContext {
    fn execute_undo_logs<T: UndoTarget + ?Sized>(&self, target: &T) -> Result<(), StorageError>;
    fn execute_undo_logs_from_index<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
        start_index: usize,
    ) -> Result<(), StorageError>;
    fn clear_undo_logs(&self);
}

impl UndoLogContext for crate::transaction::context::TransactionContext {
    fn execute_undo_logs<T: UndoTarget + ?Sized>(&self, target: &T) -> Result<(), StorageError> {
        self.execute_undo_logs(target)
            .map_err(|e| StorageError::db_error(e.to_string()))
    }

    fn execute_undo_logs_from_index<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
        start_index: usize,
    ) -> Result<(), StorageError> {
        self.execute_undo_logs_from_index(target, start_index)
            .map_err(|e| StorageError::db_error(e.to_string()))
    }

    fn clear_undo_logs(&self) {
        self.clear_undo_logs();
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

    pub fn clear_logs(&self) {
        self.ctx.clear_undo_logs();
    }
}

/// Combined Rollback Processor
///
/// Provides both OperationLog and UndoLog rollback capabilities.
/// Used for transactions that need to support both mechanisms.
pub(crate) struct CombinedRollback<'a, T: OperationLogContext + UndoLogContext> {
    ctx: &'a T,
}

impl<'a, T: OperationLogContext + UndoLogContext> CombinedRollback<'a, T> {
    pub fn new(ctx: &'a T) -> Self {
        Self { ctx }
    }

    pub fn execute_undo_rollback_from_index<U: UndoTarget + ?Sized>(
        &self,
        target: &U,
        _ts: Timestamp,
        start_index: usize,
    ) -> Result<(), StorageError> {
        self.ctx.execute_undo_logs_from_index(target, start_index)
    }

    pub fn rollback_operation_log_to_index(&self, index: usize) -> Result<(), StorageError> {
        let current_len = self.ctx.operation_log_len();

        if index > current_len {
            return Err(StorageError::db_error(format!(
                "Invalid rollback index: {}, operation log length: {}",
                index, current_len
            )));
        }

        self.ctx.truncate_operation_log(index);
        Ok(())
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
    pub oe_offset: i32,
    pub ie_offset: i32,
    pub col_id: ColumnId,
    pub old_value: PropertyValue,
}

/// Parameters for create_remove_vertex_undo operation
pub struct CreateRemoveVertexUndoParams {
    pub label: LabelId,
    pub vid: u64,
    pub related_edges: Vec<(LabelId, LabelId, LabelId, Vec<RelatedEdgeInfo>)>,
}

/// Parameters for create_remove_edge_undo operation
pub struct CreateRemoveEdgeUndoParams {
    pub src_label: LabelId,
    pub src_vid: u64,
    pub dst_label: LabelId,
    pub dst_vid: u64,
    pub edge_label: LabelId,
    pub rank: i64,
    pub oe_offset: i32,
    pub ie_offset: i32,
}

impl RollbackHelper {
    /// Reserved for future use: insert vertex undo creation
    pub fn create_insert_vertex_undo(label: LabelId, vid: u64) -> UndoLogEntry {
        UndoLogEntry::InsertVertex(InsertVertexUndo {
            v_label: label,
            vid: VertexId::from_u64(vid),
        })
    }

    pub fn create_update_vertex_prop_undo(
        label: LabelId,
        vid: u64,
        col_id: ColumnId,
        old_value: PropertyValue,
    ) -> UndoLogEntry {
        UndoLogEntry::UpdateVertexProp(UpdateVertexPropUndo {
            v_label: label,
            vid: VertexId::from_u64(vid),
            col_id,
            old_value,
        })
    }

    pub fn create_update_edge_prop_undo(params: CreateUpdateEdgePropUndoParams) -> UndoLogEntry {
        UndoLogEntry::UpdateEdgeProp(UpdateEdgePropUndo {
            src_label: params.src_label,
            src_vid: VertexId::from_u64(params.src_vid),
            dst_label: params.dst_label,
            dst_vid: VertexId::from_u64(params.dst_vid),
            edge_label: params.edge_label,
            rank: params.rank,
            oe_offset: params.oe_offset,
            ie_offset: params.ie_offset,
            col_id: params.col_id,
            old_value: params.old_value,
        })
    }

    pub fn create_remove_vertex_undo(params: CreateRemoveVertexUndoParams) -> UndoLogEntry {
        UndoLogEntry::RemoveVertex(RemoveVertexUndo {
            v_label: params.label,
            vid: VertexId::from_u64(params.vid),
            related_edges: params.related_edges,
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
            oe_offset: params.oe_offset,
            ie_offset: params.ie_offset,
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
    use crate::transaction::UndoLogManager;

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
            self.logs.borrow_mut().clear();
            Ok(())
        }

        fn execute_undo_logs_from_index<T: UndoTarget + ?Sized>(
            &self,
            _target: &T,
            _start_index: usize,
        ) -> Result<(), StorageError> {
            self.logs.borrow_mut().clear();
            Ok(())
        }

        fn clear_undo_logs(&self) {
            self.logs.borrow_mut().clear();
        }
    }

    #[test]
    fn test_undo_log_rollback() {
        let ctx = MockUndoContext::new();
        let rollback = UndoLogRollback::new(&ctx);

        assert_eq!(ctx.logs.borrow().len(), 0);
        ctx.logs
            .borrow_mut()
            .add(RollbackHelper::create_insert_vertex_undo(1, 100));
        assert_eq!(ctx.logs.borrow().len(), 1);

        rollback.clear_logs();
        assert_eq!(ctx.logs.borrow().len(), 0);
    }

    #[test]
    fn test_rollback_helper() {
        let undo = RollbackHelper::create_insert_vertex_undo(1, 100);
        assert!(undo.description().contains("InsertVertexUndo"));

        let undo = RollbackHelper::create_remove_edge_undo(CreateRemoveEdgeUndoParams {
            src_label: 1,
            src_vid: 100,
            dst_label: 2,
            dst_vid: 200,
            edge_label: 3,
            rank: 0,
            oe_offset: 0,
            ie_offset: 0,
        });
        assert!(undo.description().contains("RemoveEdgeUndo"));

        let undo = RollbackHelper::create_update_vertex_prop_undo(
            1,
            100,
            ColumnId(0),
            PropertyValue::Int(42),
        );
        assert!(undo.description().contains("UpdateVertexPropUndo"));
    }
}

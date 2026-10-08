//! Transaction manager savepoint, ownership and undo execution tests.

use super::create_test_manager;
use crate::types::*;
use crate::TransactionErrorKind;
use std::sync::Arc;
#[test]
fn test_savepoint_basic() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let sp_id = manager
        .create_savepoint(txn_id, Some("test_savepoint".to_string()), None)
        .expect("Failed to create savepoint");

    let sp = manager
        .get_savepoint(txn_id, sp_id)
        .expect("Failed to get savepoint");
    assert_eq!(sp.name, Some("test_savepoint".to_string()));

    manager
        .release_savepoint(txn_id, sp_id)
        .expect("Failed to release savepoint");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

#[test]
fn test_owner_is_required_for_kill() {
    let manager = create_test_manager();
    let txn_id = manager
        .begin_transaction_with_owner(TransactionOptions::default(), "session-1")
        .expect("transaction should begin");

    let error = manager
        .kill_transaction(txn_id, Some("session-2"))
        .expect_err("a different owner must not kill the transaction");
    assert_eq!(error.kind(), TransactionErrorKind::TransactionNotOwner);
    manager
        .kill_transaction(txn_id, Some("session-1"))
        .expect("the owner should be able to kill the transaction");
}

#[test]
fn test_abort_without_sink_executes_undo_against_target() {
    use std::sync::atomic::AtomicUsize;

    use crate::undo_log::{InsertEdgeUndo, UndoLogEntry, UndoLogError, UndoLogResult, UndoTarget};
    use linkrs_core::types::{
        ColumnId, EdgeDeletionContext, EdgeIdentifier, EdgeKey, StagedWriteMark, Timestamp,
        VertexId, VertexIdentifier,
    };

    struct CountingTarget {
        deleted_edges: AtomicUsize,
    }

    impl UndoTarget for CountingTarget {
        fn delete_vertex_type(&self, _label: crate::LabelId) -> UndoLogResult<()> {
            Ok(())
        }
        fn delete_edge_type(&self, _edge_key: EdgeKey) -> UndoLogResult<()> {
            Ok(())
        }
        fn delete_vertex(&self, _vertex: VertexIdentifier, _ts: Timestamp) -> UndoLogResult<()> {
            Ok(())
        }
        fn delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
            self.deleted_edges
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn undo_update_edge_property(
            &self,
            _edge_id: EdgeIdentifier,
            _col_id: ColumnId,
            _value: linkrs_core::Value,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_vertex_properties(
            &self,
            _label_name: &str,
            _prop_names: &[Arc<str>],
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _prop_names: &[Arc<str>],
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_vertex_label(&self, _label_name: &str) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_delete_edge_label(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_rename_vertex_properties(
            &self,
            _label_name: &str,
            _current_names: &[Arc<str>],
            _original_names: &[Arc<str>],
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn revert_rename_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _current_names: &[Arc<str>],
            _original_names: &[Arc<str>],
        ) -> UndoLogResult<()> {
            Ok(())
        }
        fn staged_write_mark(&self, _txn_id: TransactionId) -> Option<StagedWriteMark> {
            None
        }
        fn rollback_staged_writes(
            &self,
            _txn_id: TransactionId,
            mark: StagedWriteMark,
        ) -> UndoLogResult<()> {
            if mark.is_empty() {
                Ok(())
            } else {
                Err(UndoLogError::UndoFailed(
                    "mock undo target holds no staged writes".to_string(),
                ))
            }
        }
    }

    // No commit sink: the unified abort path must execute the local undo log
    // against the provided target instead of skipping it.
    let manager = create_test_manager();
    let txn_id = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("transaction should begin");
    let context = manager.get_context(txn_id).expect("context should exist");
    context
        .add_undo_log(UndoLogEntry::InsertEdge(InsertEdgeUndo {
            src_label: 1,
            dst_label: 2,
            edge_label: 3,
            rank: 0,
            src_vid: VertexId::try_from_int64(9).expect("test vertex id"),
            dst_vid: VertexId::try_from_int64(10).expect("test vertex id"),
        }))
        .expect("undo log append should succeed");

    let mut target = CountingTarget {
        deleted_edges: AtomicUsize::new(0),
    };
    manager
        .abort_transaction_with_undo(txn_id, &mut target)
        .expect("abort should succeed");

    assert_eq!(
        target
            .deleted_edges
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the inserted edge must be rolled back exactly once"
    );
    assert_eq!(context.undo_log_len(), 0);
    assert_eq!(context.state(), TransactionState::Aborted);
}

//! TransactionContext Tests
//!
//! Test transaction context functionality, including state management, timeout checking, operation logs, etc.

use std::time::Duration;

use crate::core::types::{
    ColumnId, EdgeDeletionContext, EdgeIdentifier, EdgeKey, LabelId, Timestamp, VertexIdentifier,
};
use crate::transaction::context::TransactionContext;
use crate::transaction::types::{
    DurabilityLevel, OperationLog, TransactionConfig, TransactionId, TransactionState,
};
use crate::transaction::undo_log::{InsertVertexUndo, PropertyValue, UndoLogEntry};
use crate::transaction::undo_log::{UndoLogResult, UndoTarget};
use crate::transaction::TransactionErrorKind;

struct MockUndoTarget;

impl UndoTarget for MockUndoTarget {
    fn delete_vertex_type(&self, _label: LabelId) -> UndoLogResult<()> {
        Ok(())
    }
    fn delete_edge_type(&self, _edge_key: EdgeKey) -> UndoLogResult<()> {
        Ok(())
    }
    fn delete_vertex(&self, _vertex: VertexIdentifier, _ts: Timestamp) -> UndoLogResult<()> {
        Ok(())
    }
    fn delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
        Ok(())
    }
    fn undo_update_vertex_property(
        &self,
        _vertex: VertexIdentifier,
        _col_id: ColumnId,
        _value: PropertyValue,
        _ts: Timestamp,
    ) -> UndoLogResult<()> {
        Ok(())
    }
    fn undo_update_edge_property(
        &self,
        _edge_id: EdgeIdentifier,
        _oe_offset: i32,
        _ie_offset: i32,
        _col_id: ColumnId,
        _value: PropertyValue,
        _ts: Timestamp,
    ) -> UndoLogResult<()> {
        Ok(())
    }
    fn revert_delete_vertex(&self, _vertex: VertexIdentifier, _ts: Timestamp) -> UndoLogResult<()> {
        Ok(())
    }
    fn revert_delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
        Ok(())
    }
    fn revert_delete_vertex_properties(
        &self,
        _label_name: &str,
        _prop_names: &[String],
    ) -> UndoLogResult<()> {
        Ok(())
    }
    fn revert_delete_edge_properties(
        &self,
        _src_label: &str,
        _dst_label: &str,
        _edge_label: &str,
        _prop_names: &[String],
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
        _current_names: &[String],
        _original_names: &[String],
    ) -> UndoLogResult<()> {
        Ok(())
    }
    fn revert_rename_edge_properties(
        &self,
        _src_label: &str,
        _dst_label: &str,
        _edge_label: &str,
        _current_names: &[String],
        _original_names: &[String],
    ) -> UndoLogResult<()> {
        Ok(())
    }
}

fn create_default_config(timeout: Duration) -> TransactionConfig {
    TransactionConfig {
        timeout,
        durability: DurabilityLevel::Sync,
        isolation_level: crate::transaction::types::IsolationLevel::default(),
        query_timeout: None,
        statement_timeout: None,
        idle_timeout: None,
        two_phase_commit: false,
    }
}

#[test]
fn test_transaction_context_creation() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert_eq!(ctx.id, txn_id);
    assert_eq!(ctx.state(), TransactionState::Active);
    assert!(!ctx.read_only);
    assert_eq!(ctx.durability, DurabilityLevel::Sync);
}

#[test]
fn test_transaction_context_readonly_creation() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new_readonly(txn_id, 1, config);

    assert_eq!(ctx.id, txn_id);
    assert_eq!(ctx.state(), TransactionState::Active);
    assert!(ctx.read_only);
}

#[test]
fn test_transaction_context_state_transitions() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert!(ctx.transition_to(TransactionState::Committing).is_ok());
    assert_eq!(ctx.state(), TransactionState::Committing);

    assert!(ctx.transition_to(TransactionState::Committed).is_ok());
    assert_eq!(ctx.state(), TransactionState::Committed);
}

#[test]
fn test_transaction_context_invalid_state_transition() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let result = ctx.transition_to(TransactionState::Committed);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::InvalidStateTransition);

    assert!(ctx.transition_to(TransactionState::Committing).is_ok());
    assert_eq!(ctx.state(), TransactionState::Committing);

    assert!(ctx.transition_to(TransactionState::Committed).is_ok());
    assert_eq!(ctx.state(), TransactionState::Committed);
}

#[test]
fn test_transaction_context_timeout() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_millis(100);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert!(!ctx.is_expired());

    std::thread::sleep(Duration::from_millis(150));

    assert!(ctx.is_expired());
}

#[test]
fn test_transaction_context_remaining_time() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_millis(200);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let remaining = ctx.remaining_time();
    assert!(remaining > Duration::from_millis(150));

    std::thread::sleep(Duration::from_millis(100));

    let remaining = ctx.remaining_time();
    assert!(remaining < Duration::from_millis(150));
    assert!(remaining > Duration::from_millis(50));
}

#[test]
fn test_transaction_context_modified_tables() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    ctx.record_table_modification("vertices");
    ctx.record_table_modification("edges");
    ctx.record_table_modification("vertices");

    let modified = ctx.get_modified_tables();
    assert_eq!(modified.len(), 2);
    assert!(modified.contains(&"vertices".to_string()));
    assert!(modified.contains(&"edges".to_string()));
}

#[test]
fn test_transaction_context_operation_log() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert_eq!(ctx.operation_log_len(), 0);

    ctx.add_operation_log(OperationLog::InsertVertex {
        space: "test".to_string(),
        vertex_id: vec![1, 2, 3],
        previous_state: None,
    });

    assert_eq!(ctx.operation_log_len(), 1);

    ctx.add_operation_log(OperationLog::UpdateVertex {
        space: "test".to_string(),
        vertex_id: vec![1, 2, 3],
        previous_data: vec![4, 5, 6],
    });

    assert_eq!(ctx.operation_log_len(), 2);

    ctx.truncate_operation_log(1);
    assert_eq!(ctx.operation_log_len(), 1);
}

#[test]
fn test_transaction_context_can_execute() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert!(ctx.can_execute().is_ok());

    ctx.transition_to(TransactionState::Committing)
        .expect("State transition failed");

    assert!(ctx.can_execute().is_err());
}

#[test]
fn test_transaction_context_can_execute_expired() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_millis(50);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    std::thread::sleep(Duration::from_millis(100));

    let result = ctx.can_execute();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionExpired);
}

#[test]
fn test_transaction_context_info() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    ctx.record_table_modification("vertices");

    let info = ctx.info();
    assert_eq!(info.id, txn_id);
    assert_eq!(info.state, TransactionState::Active);
    assert!(!info.is_read_only);
    assert_eq!(info.modified_tables.len(), 1);
    assert!(info.modified_tables.contains(&"vertices".to_string()));
}

#[test]
fn test_transaction_context_timestamp() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 42, config);

    assert_eq!(ctx.timestamp(), 42);
}

#[test]
fn test_savepoint_creation() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let savepoint_id = ctx.create_savepoint(Some("sp1".to_string()), 0);
    assert_eq!(savepoint_id, 1);

    let savepoint_info = ctx.get_savepoint(savepoint_id);
    assert!(savepoint_info.is_some());
    let info = savepoint_info.expect("savepoint info should exist");
    assert_eq!(info.name, Some("sp1".to_string()));
    assert_eq!(info.operation_log_index, 0);
}

#[test]
fn test_multiple_savepoints() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let sp1 = ctx.create_savepoint(Some("sp1".to_string()), 0);
    let sp2 = ctx.create_savepoint(Some("sp2".to_string()), 0);
    let sp3 = ctx.create_savepoint(Some("sp3".to_string()), 0);

    assert_eq!(sp1, 1);
    assert_eq!(sp2, 2);
    assert_eq!(sp3, 3);

    assert!(ctx.get_savepoint(sp1).is_some());
    assert!(ctx.get_savepoint(sp2).is_some());
    assert!(ctx.get_savepoint(sp3).is_some());
}

#[test]
fn test_rollback_to_savepoint() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let savepoint_id = ctx.create_savepoint(Some("sp1".to_string()), 0);

    let mock_target = MockUndoTarget;
    let result = ctx.rollback_to_savepoint(savepoint_id, &mock_target);
    assert!(result.is_ok());

    assert_eq!(ctx.operation_log_len(), 0);
}

#[test]
fn test_rollback_to_nonexistent_savepoint() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let mock_target = MockUndoTarget;
    let result = ctx.rollback_to_savepoint(999, &mock_target);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::SavepointNotFound);
}

#[test]
fn test_release_savepoint() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let savepoint_id = ctx.create_savepoint(Some("sp1".to_string()), 0);

    let result = ctx.release_savepoint(savepoint_id);
    assert!(result.is_ok());

    let savepoint_info = ctx.get_savepoint(savepoint_id);
    assert!(savepoint_info.is_none());
}

#[test]
fn test_release_nonexistent_savepoint() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let result = ctx.release_savepoint(999);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::SavepointNotFound);
}

#[test]
fn test_savepoint_with_operations() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let sp1 = ctx.create_savepoint(Some("sp1".to_string()), 0);

    let log1 = OperationLog::InsertVertex {
        space: "test".to_string(),
        vertex_id: vec![1u8, 2u8, 3u8],
        previous_state: None,
    };
    ctx.add_operation_log(log1);

    let log2 = OperationLog::InsertVertex {
        space: "test".to_string(),
        vertex_id: vec![4u8, 5u8, 6u8],
        previous_state: None,
    };
    ctx.add_operation_log(log2);

    let _sp2 = ctx.create_savepoint(Some("sp2".to_string()), 0);

    assert_eq!(ctx.operation_log_len(), 2);

    let mock_target = MockUndoTarget;
    let result = ctx.rollback_to_savepoint(sp1, &mock_target);
    assert!(result.is_ok());

    assert_eq!(ctx.operation_log_len(), 0);
}

#[test]
fn test_savepoint_rollback_preserves_prefix_state() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    ctx.add_undo_log(UndoLogEntry::InsertVertex(InsertVertexUndo {
        v_label: 1,
        vid: crate::transaction::VertexId::from_int64(1),
    }));

    let sp1 = ctx.create_savepoint(Some("sp1".to_string()), 0);

    ctx.add_undo_log(UndoLogEntry::InsertVertex(InsertVertexUndo {
        v_label: 1,
        vid: crate::transaction::VertexId::from_int64(2),
    }));

    let mock_target = MockUndoTarget;
    let result = ctx.rollback_to_savepoint(sp1, &mock_target);
    assert!(result.is_ok());

    assert_eq!(ctx.undo_log_len(), 1);
}

#[test]
fn test_find_savepoint_by_name() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    ctx.create_savepoint(Some("sp1".to_string()), 0);
    ctx.create_savepoint(Some("sp2".to_string()), 0);
    ctx.create_savepoint(None, 0);

    let found = ctx.find_savepoint_by_name("sp1");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, Some("sp1".to_string()));

    let found = ctx.find_savepoint_by_name("sp2");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, Some("sp2".to_string()));

    let not_found = ctx.find_savepoint_by_name("nonexistent");
    assert!(not_found.is_none());
}

#[test]
fn test_get_all_savepoints() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    ctx.create_savepoint(Some("sp1".to_string()), 0);
    ctx.create_savepoint(Some("sp2".to_string()), 0);
    ctx.create_savepoint(None, 0);

    let all_sp = ctx.get_all_savepoints();
    assert_eq!(all_sp.len(), 3);
}

#[test]
fn test_clear() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    ctx.add_operation_log(OperationLog::InsertVertex {
        space: "test".to_string(),
        vertex_id: vec![1, 2, 3],
        previous_state: None,
    });
    ctx.record_table_modification("vertices");
    ctx.create_savepoint(Some("sp1".to_string()), 0);

    ctx.clear();

    assert_eq!(ctx.operation_log_len(), 0);
    assert_eq!(ctx.get_modified_tables().len(), 0);
    assert_eq!(ctx.get_all_savepoints().len(), 0);
}

#[test]
fn test_query_count() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert_eq!(ctx.query_count(), 0);

    ctx.increment_query_count();
    ctx.increment_query_count();
    ctx.increment_query_count();

    assert_eq!(ctx.query_count(), 3);
}

#[test]
fn test_update_activity() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    std::thread::sleep(Duration::from_millis(50));

    ctx.update_activity();

    assert!(!ctx.is_idle_timeout());
}

#[test]
fn test_two_phase_commit_flag() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = TransactionConfig {
        timeout,
        durability: DurabilityLevel::Sync,
        isolation_level: crate::transaction::types::IsolationLevel::default(),
        query_timeout: None,
        statement_timeout: None,
        idle_timeout: None,
        two_phase_commit: true,
    };

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert!(ctx.is_two_phase_enabled());
}

#[test]
fn test_abort_state_transitions() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    assert!(ctx.transition_to(TransactionState::Aborting).is_ok());
    assert_eq!(ctx.state(), TransactionState::Aborting);

    assert!(ctx.transition_to(TransactionState::Aborted).is_ok());
    assert_eq!(ctx.state(), TransactionState::Aborted);
}

#[test]
fn test_add_operation_logs_batch() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    let logs = vec![
        OperationLog::InsertVertex {
            space: "test".to_string(),
            vertex_id: vec![1],
            previous_state: None,
        },
        OperationLog::InsertVertex {
            space: "test".to_string(),
            vertex_id: vec![2],
            previous_state: None,
        },
        OperationLog::InsertVertex {
            space: "test".to_string(),
            vertex_id: vec![3],
            previous_state: None,
        },
    ];

    ctx.add_operation_logs(logs);

    assert_eq!(ctx.operation_log_len(), 3);
}

#[test]
fn test_get_operation_log_range() {
    let txn_id = TransactionId(1);
    let timeout = Duration::from_secs(30);
    let config = create_default_config(timeout);

    let ctx = TransactionContext::new(txn_id, 1, config);

    for i in 0..5 {
        ctx.add_operation_log(OperationLog::InsertVertex {
            space: "test".to_string(),
            vertex_id: vec![i],
            previous_state: None,
        });
    }

    let range = ctx.get_operation_logs_range(1, 4);
    assert_eq!(range.len(), 3);

    let empty_range = ctx.get_operation_logs_range(10, 15);
    assert_eq!(empty_range.len(), 0);
}

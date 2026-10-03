//! Transaction manager statement snapshot and read-committed refresh tests.

use super::create_test_manager;
use crate::types::*;
#[test]
fn test_read_committed_refreshes_statement_snapshot() {
    let manager = create_test_manager();
    let reader = manager
        .begin_read_transaction(
            TransactionOptions::default()
                .read_only()
                .with_isolation_level(crate::IsolationLevel::ReadCommitted),
        )
        .expect("reader should begin");
    let initial = manager
        .get_context(reader)
        .expect("reader context should exist")
        .effective_read_timestamp();

    let writer = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("writer should begin");
    manager
        .commit_transaction(writer)
        .expect("writer should commit");

    let (context, statement_start) = manager
        .begin_statement(reader)
        .expect("reader statement should begin");
    assert!(context.effective_read_timestamp() > initial);
    manager
        .finish_statement(&context, statement_start)
        .expect("reader statement should finish");
    manager
        .commit_transaction(reader)
        .expect("reader should commit");
}

#[test]
fn test_statement_snapshot_pin_lifecycle() {
    let manager = create_test_manager();
    let reader = manager
        .begin_read_transaction(
            TransactionOptions::default()
                .read_only()
                .with_isolation_level(crate::IsolationLevel::ReadCommitted),
        )
        .expect("reader should begin");

    // Advance the committed frontier past the reader start so the
    // refreshed statement snapshot is a distinct timestamp with its
    // own pin (repeatable-read pins coincide with the start slot).
    let writer = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("writer should begin");
    manager
        .commit_transaction(writer)
        .expect("writer should commit");

    let (context, statement_start) = manager
        .begin_statement(reader)
        .expect("statement should begin");
    let pinned = context.effective_read_timestamp();
    assert!(pinned > context.start_timestamp);
    // The running statement snapshot is pinned globally: GC must observe it.
    assert!(manager
        .version_manager()
        .snapshot_tracker()
        .contains_snapshot(pinned));

    manager
        .finish_statement(&context, statement_start)
        .expect("statement should finish");
    // Released on finish: no pin outlives its statement.
    assert!(!manager
        .version_manager()
        .snapshot_tracker()
        .contains_snapshot(pinned));

    manager
        .commit_transaction(reader)
        .expect("reader should commit");
    assert_eq!(
        manager.version_manager().snapshot_tracker().active_count(),
        0
    );
}

#[test]
fn test_statement_snapshot_pin_released_on_abort() {
    let manager = create_test_manager();
    let writer = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("writer should begin");

    let (context, _) = manager
        .begin_statement(writer)
        .expect("statement should begin");
    let pinned = context.effective_read_timestamp();
    assert!(manager
        .version_manager()
        .snapshot_tracker()
        .contains_snapshot(pinned));

    manager
        .abort_transaction(writer)
        .expect("writer should abort");
    assert!(!manager
        .version_manager()
        .snapshot_tracker()
        .contains_snapshot(pinned));
}

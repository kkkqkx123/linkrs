//! Transaction manager not-found and terminal-state error tests.

use super::create_test_manager;
use crate::types::*;
use crate::TransactionErrorKind;
#[test]
fn test_get_transaction_not_found() {
    let manager = create_test_manager();

    let result = manager.get_context(TransactionId(9999));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_commit_transaction_not_found() {
    let manager = create_test_manager();

    let result = manager.commit_transaction(TransactionId(9999));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_abort_transaction_not_found() {
    let manager = create_test_manager();

    let result = manager.abort_transaction(TransactionId(9999));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_commit_already_committed_transaction() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("First commit failed");

    let result = manager.commit_transaction(txn_id);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_abort_already_aborted_transaction() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to start transaction");

    manager
        .abort_transaction(txn_id)
        .expect("First abort failed");

    let result = manager.abort_transaction(txn_id);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_committed_transaction_cannot_be_aborted() {
    let manager = create_test_manager();
    let txn_id = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("transaction should begin");
    manager
        .commit_transaction(txn_id)
        .expect("commit should succeed");

    // Committed transactions leave the active table in the terminal
    // `Committed` state, so a second abort finds nothing to abort.
    let error = manager
        .abort_transaction(txn_id)
        .expect_err("aborting a committed transaction must fail");
    assert_eq!(error.kind(), TransactionErrorKind::TransactionNotFound);
}

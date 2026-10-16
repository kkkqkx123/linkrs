//! Savepoint creation guards: duplicate names and finished transactions.

use super::create_test_manager;
use crate::types::TransactionOptions;
use crate::TransactionErrorKind;

#[test]
fn duplicate_savepoint_name_is_rejected() {
    let manager = create_test_manager();
    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("begin transaction");
    manager
        .create_savepoint(txn_id, Some("sp1".to_string()), None)
        .expect("first savepoint");
    let err = manager
        .create_savepoint(txn_id, Some("sp1".to_string()), None)
        .expect_err("duplicate savepoint name must fail");
    assert_eq!(err.kind(), TransactionErrorKind::SavepointFailed);

    // Releasing frees the name for reuse.
    let info = manager
        .get_context(txn_id)
        .expect("transaction context")
        .find_savepoint_by_name("sp1")
        .expect("savepoint sp1");
    manager
        .release_savepoint(txn_id, info.id)
        .expect("release savepoint");
    manager
        .create_savepoint(txn_id, Some("sp1".to_string()), None)
        .expect("recreate after release");
    manager.abort_transaction(txn_id).expect("abort");
}

#[test]
fn savepoint_on_finished_transaction_is_rejected() {
    let manager = create_test_manager();
    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("begin transaction");
    manager.commit_transaction(txn_id).expect("commit");
    let err = manager
        .create_savepoint(txn_id, Some("sp9".to_string()), None)
        .expect_err("savepoint on finished transaction must fail");
    assert!(
        matches!(
            err.kind(),
            TransactionErrorKind::TransactionNotFound
                | TransactionErrorKind::InvalidStateForExecution
        ),
        "unexpected kind: {:?}",
        err.kind()
    );
}

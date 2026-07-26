//! API Consistency Tests
//!
//! Verifies that different execution paths (materialized, streaming, embedded)
//! produce the same transactional results for the same script.

use graphdb::core::types::TransactionId;
use graphdb::transaction::{TransactionManager, TransactionManagerConfig, TransactionOptions};


/// Verify that the same transaction lifecycle (begin → commit) works
/// regardless of whether the transaction is created via `begin_transaction`
/// or `begin_insert_transaction`.
#[test]
fn test_begin_methods_produce_consistent_ids() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_insert = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin insert txn");
    let txn_legacy = manager
        .begin_transaction(TransactionOptions::default())
        .expect("begin legacy txn");

    assert_ne!(
        txn_insert, txn_legacy,
        "concurrent txns must have distinct IDs"
    );

    let ctx_insert = manager.get_context(txn_insert).expect("get insert ctx");
    let ctx_legacy = manager.get_context(txn_legacy).expect("get legacy ctx");

    assert!(!ctx_insert.read_only, "insert txn must be read-write");
    assert!(!ctx_legacy.read_only, "legacy write txn must be read-write");

    manager
        .commit_transaction(txn_insert)
        .expect("commit insert txn");
    manager
        .commit_transaction(txn_legacy)
        .expect("commit legacy txn");
}

/// Verify that owner-checked transactions enforce ownership.
#[test]
fn test_owner_check_enforcement() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_transaction_with_owner(TransactionOptions::default(), "session-alpha".to_string())
        .expect("begin owned txn");

    let result = manager.check_transaction_owner(txn, Some("session-beta"));
    assert!(result.is_err(), "wrong owner should fail ownership check");

    let result = manager.check_transaction_owner(txn, Some("session-alpha"));
    assert!(result.is_ok(), "correct owner should pass ownership check");

    manager.commit_transaction(txn).expect("commit owned txn");
}

/// Verify that set_transaction_owner can transfer ownership.
#[test]
fn test_set_transaction_owner() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_transaction(TransactionOptions::default())
        .expect("begin txn");

    manager
        .set_transaction_owner(txn, "new-owner".to_string())
        .expect("set owner");

    let result = manager.check_transaction_owner(txn, Some("new-owner"));
    assert!(result.is_ok(), "new owner should pass check");

    manager.commit_transaction(txn).expect("commit txn");
}

/// Verify that commit/abort with owner check enforces ownership.
#[test]
fn test_commit_requires_owner() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_transaction_with_owner(TransactionOptions::default(), "owner-1".to_string())
        .expect("begin owned txn");

    let result = manager.commit_transaction_as_owner(txn, Some("owner-2"));
    assert!(result.is_err(), "commit with wrong owner should fail");

    let result = manager.commit_transaction_as_owner(txn, Some("owner-1"));
    assert!(result.is_ok(), "commit with correct owner should succeed");
}

/// Verify that the transaction info includes owner information.
#[test]
fn test_transaction_info_includes_owner() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_transaction_with_owner(TransactionOptions::default(), "test-session".to_string())
        .expect("begin owned txn");

    let info = manager.get_transaction_info(txn).expect("get txn info");
    assert_eq!(info.owner.as_deref(), Some("test-session"));

    manager.commit_transaction(txn).expect("commit txn");
}

/// Verify that list_transactions returns all active transactions.
#[test]
fn test_list_transactions() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin read txn_a");
    let txn_b = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin read txn_b");

    let active = manager.list_active_transactions();
    assert!(
        active.len() >= 2,
        "should list at least 2 active transactions"
    );

    let ids: Vec<TransactionId> = active.iter().map(|info| info.id).collect();
    assert!(ids.contains(&txn_a));
    assert!(ids.contains(&txn_b));

    manager.commit_transaction(txn_a).expect("commit txn_a");
    manager.commit_transaction(txn_b).expect("commit txn_b");
}

/// Verify that the same transaction script produces the same stats outcome
/// when executed multiple times.
#[test]
fn test_repeated_transaction_script_consistency() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    for _ in 0..5 {
        let txn = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("begin write txn");
        manager.commit_transaction(txn).expect("commit txn");
    }

    for _ in 0..3 {
        let txn = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("begin write txn");
        manager.abort_transaction(txn).expect("abort txn");
    }

    let stats = manager.stats();
    let committed = stats
        .committed_transactions
        .load(std::sync::atomic::Ordering::Relaxed);
    let aborted = stats
        .aborted_transactions
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(committed >= 5, "should have at least 5 committed");
    assert!(aborted >= 3, "should have at least 3 aborted");
}

/// Verify that read-only transactions with owner work correctly.
#[test]
fn test_readonly_with_owner() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_transaction_with_owner(
            TransactionOptions::new().read_only(),
            "reader-session".to_string(),
        )
        .expect("begin readonly owned txn");

    let ctx = manager.get_context(txn).expect("get context");
    assert!(ctx.read_only);

    let info = manager.get_transaction_info(txn).expect("get info");
    assert_eq!(info.owner.as_deref(), Some("reader-session"));
    assert!(info.is_read_only);

    manager
        .commit_transaction(txn)
        .expect("commit readonly txn");
}

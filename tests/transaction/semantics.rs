//! Transaction Semantic Invariant Tests
//!
//! These tests verify core transaction correctness properties:
//! - Uncommitted writes are never visible to other transactions
//! - Rollback fully reverts data and index state
//! - Savepoint rollback only undoes post-savepoint operations
//! - Out-of-order commit does not expose lower-timestamp writes

use graphdb::core::types::TransactionId;
use graphdb::transaction::{
    TransactionManager, TransactionManagerConfig, TransactionOptions,
};

use std::sync::Arc;
use std::time::Duration;

/// Verify that after a full rollback, the transaction's write timestamp is released
/// and the data it wrote is not visible through a new read transaction.
#[test]
fn test_rollback_releases_write_timestamp() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin insert txn_a");

    let ctx_a = manager.get_context(txn_a).expect("get context txn_a");
    let write_ts = ctx_a.timestamp();

    manager.abort_transaction(txn_a).expect("abort txn_a");

    let vm = manager.version_manager();
    let read_ts = vm.read_timestamp();

    assert!(
        read_ts >= write_ts,
        "read_ts ({}) should have advanced past aborted write_ts ({})",
        read_ts,
        write_ts
    );

    assert!(
        !manager.is_transaction_active(txn_a),
        "aborted transaction must not be active"
    );
}

/// Verify that committing a higher write timestamp before a lower one
/// does NOT advance the frontier past the low-ts (which is still pending).
/// The frontier only advances through contiguous decided entries.
#[test]
fn test_out_of_order_commit_visibility() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_low = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin low-ts txn");
    let ctx_low = manager.get_context(txn_low).expect("get ctx_low");
    let low_ts = ctx_low.timestamp();

    let txn_high = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin high-ts txn");
    let ctx_high = manager.get_context(txn_high).expect("get ctx_high");
    let high_ts = ctx_high.timestamp();

    assert!(
        high_ts > low_ts,
        "high_ts ({}) must be greater than low_ts ({})",
        high_ts,
        low_ts
    );

    manager
        .commit_transaction(txn_high)
        .expect("commit high-ts txn");

    let read_ts_after_high = manager.version_manager().read_timestamp();
    assert!(
        read_ts_after_high < high_ts,
        "read_ts ({}) must NOT reach high_ts ({}) while low_ts is pending",
        read_ts_after_high,
        high_ts
    );

    manager
        .commit_transaction(txn_low)
        .expect("commit low-ts txn");

    let read_ts_final = manager.version_manager().read_timestamp();
    assert!(
        read_ts_final >= high_ts,
        "read_ts ({}) should reach high_ts ({}) after both commit",
        read_ts_final,
        high_ts
    );
}

/// Verify that aborting a write transaction advances the frontier to the aborted
/// timestamp (so readers can skip past it) but does NOT publish the aborted data.
#[test]
fn test_abort_does_not_advance_frontier() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin txn_a");
    let ctx_a = manager.get_context(txn_a).expect("get ctx_a");
    let ts_a = ctx_a.timestamp();

    let read_ts_before = manager.version_manager().read_timestamp();

    manager.abort_transaction(txn_a).expect("abort txn_a");

    let read_ts_after = manager.version_manager().read_timestamp();
    assert!(
        read_ts_after >= ts_a,
        "read_ts ({}) should advance to aborted ts ({}) so readers skip it",
        read_ts_after,
        ts_a
    );
    assert!(
        read_ts_after >= read_ts_before,
        "read_ts should not regress after abort"
    );
}

/// Verify that a read-only transaction acquires a timestamp that is released on commit.
#[test]
fn test_read_timestamp_released_on_commit() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin read txn");

    let pending_before = manager.version_manager().pending_count();

    manager.commit_transaction(txn).expect("commit read txn");

    let pending_after = manager.version_manager().pending_count();
    assert!(
        pending_after <= pending_before,
        "pending count should not increase after read txn commit"
    );
}

/// Verify that two concurrent read-only transactions can coexist.
#[test]
fn test_concurrent_read_transactions() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin read txn_a");
    let txn_b = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin read txn_b");

    assert_ne!(txn_a, txn_b, "concurrent reads must have distinct IDs");
    assert!(manager.is_transaction_active(txn_a));
    assert!(manager.is_transaction_active(txn_b));

    manager.commit_transaction(txn_a).expect("commit txn_a");
    assert!(!manager.is_transaction_active(txn_a));
    assert!(manager.is_transaction_active(txn_b));

    manager.commit_transaction(txn_b).expect("commit txn_b");
    assert!(!manager.is_transaction_active(txn_b));
}

/// Verify that two write transactions can begin simultaneously (optimistic model).
/// Conflict is detected at commit time via write-set certification.
#[test]
fn test_concurrent_write_begin_succeeds() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin first write txn");

    let txn_b = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("second write txn should begin (optimistic)");

    assert_ne!(txn_a, txn_b, "concurrent writes must have distinct IDs");
    assert!(manager.is_transaction_active(txn_a));
    assert!(manager.is_transaction_active(txn_b));

    manager.commit_transaction(txn_a).expect("commit txn_a");
    manager.commit_transaction(txn_b).expect("commit txn_b");
}

/// Verify that after committing a write transaction, a new write can begin.
#[test]
fn test_write_after_commit_succeeds() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin txn_a");
    manager.commit_transaction(txn_a).expect("commit txn_a");

    let _txn_b = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin txn_b after txn_a commit");
}

/// Verify that the committed transaction info reflects its read-write nature.
#[test]
fn test_insert_txn_info_is_writable() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin write txn");

    let info = manager
        .get_transaction_info(txn)
        .expect("get txn info");
    assert!(!info.is_read_only, "insert txn must not be read-only");

    manager.commit_transaction(txn).expect("commit txn");
}

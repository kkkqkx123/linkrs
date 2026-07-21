//! Transaction Lifecycle Tests
//!
//! Verifies transaction lifecycle invariants:
//! - Read timestamp guard releases original timestamp, not global read_ts
//! - Historical snapshot registration and retention
//! - Timeout cleanup releases all resources
//! - Snapshot tracker has no leaks after commit/abort

use graphdb::core::types::TransactionId;
use graphdb::transaction::{TransactionManager, TransactionManagerConfig, TransactionOptions};

use std::sync::Arc;
use std::time::Duration;

/// Verify that a read-only transaction's snapshot is properly registered
/// and released after commit.
#[test]
fn test_read_snapshot_registration_and_release() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let vm = manager.version_manager();
    let initial_pending = vm.pending_count();

    let txn = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin read txn");

    let during_pending = vm.pending_count();
    assert!(
        during_pending > initial_pending,
        "pending snapshots should increase after begin"
    );

    manager.commit_transaction(txn).expect("commit read txn");

    let after_pending = vm.pending_count();
    assert_eq!(
        after_pending, initial_pending,
        "pending snapshots should return to initial after commit"
    );
}

/// Verify that aborting a read-only transaction does not leak its snapshot.
#[test]
fn test_read_abort_no_leak() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let vm = manager.version_manager();
    let initial_pending = vm.pending_count();

    let txn = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin read txn");
    manager.abort_transaction(txn).expect("abort read txn");

    let after_pending = vm.pending_count();
    assert_eq!(
        after_pending, initial_pending,
        "pending snapshots should return to initial after abort"
    );
}

/// Verify that timeout cleanup makes the transaction inactive.
#[test]
fn test_timeout_makes_inactive() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_millis(10)))
        .expect("begin txn with short timeout");

    assert!(manager.is_transaction_active(txn));

    std::thread::sleep(Duration::from_millis(50));
    manager.cleanup_expired_transactions();

    assert!(
        !manager.is_transaction_active(txn),
        "timed-out transaction must be inactive after cleanup"
    );
}

/// Verify that timeout cleanup decrements the active transaction counter in stats.
#[test]
fn test_timeout_cleanup_updates_stats() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let _txn = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_millis(10)))
        .expect("begin txn");

    let stats_before = manager.stats();
    let active_before = stats_before
        .active_transactions
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(active_before >= 1);

    std::thread::sleep(Duration::from_millis(50));
    manager.cleanup_expired_transactions();

    let stats_after = manager.stats();
    let active_after = stats_after
        .active_transactions
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        active_after < active_before,
        "active count should decrease after timeout cleanup"
    );
}

/// Verify that begin_snapshot_read validates the timestamp is not too recent.
#[test]
fn test_snapshot_read_rejects_future_timestamp() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let current_write_ts = manager.version_manager().write_timestamp();
    let future_ts = current_write_ts.saturating_add(100);

    let result = manager.begin_snapshot_read(future_ts, TransactionOptions::default());
    assert!(
        result.is_err(),
        "snapshot read should reject timestamp beyond committed frontier"
    );
}

/// Verify that begin_snapshot_read succeeds for a valid past timestamp.
#[test]
fn test_snapshot_read_accepts_past_timestamp() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let current_write_ts = manager.version_manager().write_timestamp();

    let txn = manager
        .begin_snapshot_read(current_write_ts, TransactionOptions::default())
        .expect("snapshot read at current write_ts should succeed");

    let ctx = manager.get_context(txn).expect("get snapshot context");
    assert_eq!(
        ctx.effective_snapshot_timestamp(),
        current_write_ts,
        "snapshot should read at the requested timestamp"
    );

    manager
        .commit_transaction(txn)
        .expect("commit snapshot read");
}

/// Verify that a transaction can be force-killed and becomes inactive.
#[test]
fn test_kill_transaction() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin write txn");
    assert!(manager.is_transaction_active(txn));

    manager.kill_transaction(txn, None).expect("kill txn");

    assert!(
        !manager.is_transaction_active(txn),
        "killed transaction must be inactive"
    );
}

/// Verify that shutdown aborts all active transactions.
#[test]
fn test_shutdown_aborts_all() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig {
        auto_cleanup: false,
        ..Default::default()
    }));

    let txn_a = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin txn_a");
    let txn_b = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("begin txn_b");

    assert!(manager.is_transaction_active(txn_a));
    assert!(manager.is_transaction_active(txn_b));

    manager.clone().shutdown();

    assert!(
        !manager.is_transaction_active(txn_a),
        "manager shutdown should abort active write txn"
    );
    assert!(
        !manager.is_transaction_active(txn_b),
        "manager shutdown should abort active read txn"
    );
}

/// Verify that double-commit is rejected.
#[test]
fn test_double_commit_rejected() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin write txn");
    manager.commit_transaction(txn).expect("first commit");

    let result = manager.commit_transaction(txn);
    assert!(result.is_err(), "double commit should be rejected");
}

/// Verify that commit of a non-existent transaction is rejected.
#[test]
fn test_commit_invalid_transaction() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let result = manager.commit_transaction(TransactionId(99999));
    assert!(result.is_err(), "commit of non-existent txn should fail");
}

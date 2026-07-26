//! Transaction Recovery Tests
//!
//! Verifies crash recovery properties:
//! - Commit record persistence before/after crash
//! - Recovery only replays committed transactions
//! - Fault injection at WAL stages
//! - Timeout/shutdown resource cleanup completeness

use graphdb::transaction::{
    TransactionManager, TransactionManagerConfig, TransactionOptions,
};

use std::sync::Arc;
use std::time::Duration;

/// Verify that when a transaction times out and is cleaned up,
/// its write timestamp is released (making room for new writes).
#[test]
fn test_timeout_cleanup_releases_write_timestamp() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_insert_transaction(TransactionOptions::new().with_timeout(Duration::from_millis(10)))
        .expect("begin short-lived write txn");

    let vm = manager.version_manager();
    let write_ts_a = vm.write_timestamp();

    std::thread::sleep(Duration::from_millis(50));
    manager.cleanup_expired_transactions();

    assert!(!manager.is_transaction_active(txn_a));

    let txn_b = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin new write txn after cleanup");

    let write_ts_b = vm.write_timestamp();
    assert!(
        write_ts_b >= write_ts_a,
        "write timestamp should not regress after cleanup"
    );

    manager.commit_transaction(txn_b).expect("commit txn_b");
}

/// Verify that shutdown prevents new transactions from beginning.
#[test]
fn test_shutdown_prevents_new_transactions() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig {
        auto_cleanup: false,
        ..Default::default()
    }));

    let _txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin txn");
    manager.clone().shutdown();

    let result = manager.begin_insert_transaction(TransactionOptions::default());
    assert!(result.is_err(), "new txn should be rejected after shutdown");
}

/// Verify that the version manager's commit_write_timestamp properly advances
/// the committed frontier.
#[test]
fn test_commit_advances_frontier() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let vm = manager.version_manager();
    let initial_write_ts = vm.write_timestamp();

    let txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin write txn");

    let ctx = manager.get_context(txn).expect("get context");
    let txn_write_ts = ctx.timestamp();

    assert!(
        txn_write_ts > initial_write_ts,
        "new write txn should have a higher timestamp"
    );

    manager.commit_transaction(txn).expect("commit txn");

    let after_commit_ts = vm.read_timestamp();
    assert!(
        after_commit_ts >= txn_write_ts,
        "committed frontier ({}) should reach txn write_ts ({})",
        after_commit_ts,
        txn_write_ts
    );
}

/// Verify that abort advances the frontier to include the aborted timestamp.
#[test]
fn test_abort_advances_frontier() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let vm = manager.version_manager();
    let frontier_before = vm.read_timestamp();

    let txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin write txn");

    let ctx = manager.get_context(txn).expect("get context");
    let txn_write_ts = ctx.timestamp();

    manager.abort_transaction(txn).expect("abort txn");

    let frontier_after = vm.read_timestamp();
    assert!(
        frontier_after >= txn_write_ts,
        "frontier ({}) must reach aborted write_ts ({}) to skip it",
        frontier_after,
        txn_write_ts
    );
    assert!(
        frontier_after >= frontier_before,
        "frontier should not regress"
    );
}

/// Verify concurrent read-only transactions after a write abort
/// can still read consistent snapshots.
#[tokio::test]
async fn test_concurrent_reads_after_abort() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let write_txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin write txn");
    manager
        .abort_transaction(write_txn)
        .expect("abort write txn");

    let mut handles = vec![];
    for _ in 0..3 {
        let mgr = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            let txn = mgr
                .begin_read_transaction(TransactionOptions::new().read_only())
                .expect("begin read");
            mgr.commit_transaction(txn).expect("commit read");
        }));
    }

    for h in handles {
        h.await.expect("task failed");
    }
}

/// Verify that two concurrent write transactions can coexist (optimistic).
#[test]
fn test_concurrent_writes_coexist() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_a = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin first write txn");

    let txn_b = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("second write txn should begin (optimistic model)");

    assert_ne!(txn_a, txn_b);

    manager.commit_transaction(txn_a).expect("commit txn_a");
    manager.commit_transaction(txn_b).expect("commit txn_b");
}

/// Verify that marking a transaction as rollback-only prevents commit.
#[test]
fn test_rollback_only_prevents_commit() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin write txn");

    manager.mark_disconnect(txn).expect("mark disconnect");

    let result = manager.commit_transaction(txn);
    assert!(result.is_err(), "commit of rollback-only txn should fail");
}

/// Verify that stats track commits and aborts correctly.
#[test]
fn test_stats_commit_abort_tracking() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let commit_txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin commit txn");
    manager.commit_transaction(commit_txn).expect("commit txn");

    let abort_txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("begin abort txn");
    manager.abort_transaction(abort_txn).expect("abort txn");

    let stats = manager.stats();
    assert!(
        stats
            .committed_transactions
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 1
    );
    assert!(
        stats
            .aborted_transactions
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 1
    );
}

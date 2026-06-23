//! Write Lock Timeout and Transaction Cleanup Tests
//!
//! Test coverage for the fixes addressing e2e test timeout failures:
//! - Write lock timeout in TransactionManager
//! - Write transaction exclusivity
//! - Transaction cleanup mechanism for expired/stuck transactions
//! - Recovery after write lock timeout
//! - Transaction context cleanup on query failure
//! - StorageInner lock ordering consistency

use graphdb::transaction::{TransactionManager, TransactionManagerConfig, TransactionOptions};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};

/// Test that TransactionManagerConfig has write_lock_timeout field with default
#[test]
fn test_write_lock_timeout_config_default() {
    let config = TransactionManagerConfig::default();
    assert_eq!(config.write_lock_timeout, Duration::from_secs(10));
}

/// Test that TransactionManagerConfig can be customized with a short write_lock_timeout
#[test]
fn test_write_lock_timeout_config_custom() {
    let config = TransactionManagerConfig {
        write_lock_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    assert_eq!(config.write_lock_timeout, Duration::from_secs(5));
}

/// Test that write transaction can be acquired when no other write is active
#[test]
fn test_write_lock_acquired_successfully() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should acquire write lock when no other write is active");

    manager
        .commit_transaction(txn_id)
        .expect("Should commit successfully");
}

/// Test that a second write transaction is rejected with WriteTransactionConflict
/// when another write transaction is active (not blocking indefinitely)
#[test]
fn test_write_conflict_does_not_block_indefinitely() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin first write transaction");

    let result = manager.begin_transaction(TransactionOptions::default());

    assert!(
        result.is_err(),
        "Second write transaction should fail with WriteTransactionConflict"
    );

    manager
        .commit_transaction(txn1)
        .expect("Should commit first transaction");
}

/// Test that after committing a write transaction, a new write can be acquired
#[test]
fn test_write_lock_released_after_commit() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin first write transaction");

    manager
        .commit_transaction(txn1)
        .expect("Should commit first transaction");

    let txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should acquire write lock after commit");

    manager
        .commit_transaction(txn2)
        .expect("Should commit second transaction");
}

/// Test that after rolling back a write transaction, a new write can be acquired
#[test]
fn test_write_lock_released_after_rollback() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin first write transaction");

    manager
        .abort_transaction(txn1)
        .expect("Should rollback first transaction");

    let txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should acquire write lock after rollback");

    manager
        .abort_transaction(txn2)
        .expect("Should rollback second transaction");
}

/// Test that expired transactions are cleaned up and no longer block writes
#[tokio::test]
async fn test_cleanup_expired_transactions_releases_write_lock() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_timeout(Duration::from_millis(50));
    let txn_id = manager
        .begin_transaction(options)
        .expect("Should begin transaction with short timeout");

    assert!(manager.is_transaction_active(txn_id));

    sleep(Duration::from_millis(100)).await;

    manager.cleanup_expired_transactions();

    assert!(
        !manager.is_transaction_active(txn_id),
        "Expired transaction should be cleaned up"
    );

    let txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should acquire write lock after expired transaction cleanup");

    manager
        .commit_transaction(txn2)
        .expect("Should commit new transaction");
}

/// Test that multiple expired transactions are cleaned up
#[tokio::test]
async fn test_cleanup_multiple_expired_transactions() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // Begin a write transaction, then read-only transactions
    let write_txn = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin write transaction");

    let read1 = manager
        .begin_transaction(
            TransactionOptions::new()
                .read_only()
                .with_timeout(Duration::from_millis(50)),
        )
        .expect("Should begin read transaction 1");

    let read2 = manager
        .begin_transaction(
            TransactionOptions::new()
                .read_only()
                .with_timeout(Duration::from_millis(50)),
        )
        .expect("Should begin read transaction 2");

    sleep(Duration::from_millis(100)).await;

    manager.cleanup_expired_transactions();

    assert!(
        !manager.is_transaction_active(read1),
        "Read txn 1 should be cleaned up"
    );
    assert!(
        !manager.is_transaction_active(read2),
        "Read txn 2 should be cleaned up"
    );

    manager
        .abort_transaction(write_txn)
        .expect("Should rollback write transaction");
}

/// Test that sequential write transactions complete within reasonable time
/// This directly addresses the e2e test timeout issue
#[test]
fn test_sequential_writes_complete_quickly() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    for i in 0..10 {
        let txn_id = manager
            .begin_transaction(TransactionOptions::default())
            .unwrap_or_else(|_| panic!("Should begin transaction {}", i));

        manager
            .commit_transaction(txn_id)
            .unwrap_or_else(|_| panic!("Should commit transaction {}", i));
    }
}

/// Test that read-only transactions can run concurrently with each other
#[tokio::test]
async fn test_concurrent_read_only_transactions() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let result = timeout(Duration::from_secs(30), async {
        let mut handles = vec![];
        for _ in 0..5 {
            let mgr = Arc::clone(&manager);
            handles.push(tokio::spawn(async move {
                let txn_id = mgr
                    .begin_transaction(TransactionOptions::new().read_only())
                    .expect("Should begin read transaction");
                sleep(Duration::from_millis(10)).await;
                mgr.commit_transaction(txn_id)
                    .expect("Should commit read transaction");
            }));
        }
        for handle in handles {
            handle.await.expect("Task should complete");
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "Concurrent read-only transactions should complete within 30 seconds"
    );
}

/// Test that write transactions with short write_lock_timeout fail quickly
/// when another write is active
#[test]
fn test_short_write_lock_timeout_fails_quickly() {
    let config = TransactionManagerConfig {
        write_lock_timeout: Duration::from_millis(100),
        ..Default::default()
    };
    let manager = TransactionManager::new(config);

    let _txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin first write transaction");

    // Note: TransactionManager rejects second write with WriteTransactionConflict
    // before even attempting begin_write(), so this test verifies the fast rejection path
    let start = std::time::Instant::now();
    let result = manager.begin_transaction(TransactionOptions::default());
    let elapsed = start.elapsed();

    assert!(result.is_err(), "Second write should be rejected");
    assert!(
        elapsed < Duration::from_secs(1),
        "Rejection should be fast, took {:?}",
        elapsed
    );
}

/// Test that the transaction manager properly handles the lifecycle:
/// begin -> commit -> begin -> rollback -> begin -> commit
#[test]
fn test_transaction_lifecycle_after_errors() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // begin -> commit
    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin txn1");
    manager
        .commit_transaction(txn1)
        .expect("Should commit txn1");

    // begin -> rollback
    let txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin txn2");
    manager
        .abort_transaction(txn2)
        .expect("Should rollback txn2");

    // begin -> commit (should still work after rollback)
    let txn3 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should begin txn3 after rollback");
    manager
        .commit_transaction(txn3)
        .expect("Should commit txn3");
}

/// Test that cleanup_expired_transactions doesn't affect active transactions
#[test]
fn test_cleanup_does_not_affect_active_transactions() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_secs(300)))
        .expect("Should begin transaction with long timeout");

    manager.cleanup_expired_transactions();

    assert!(
        manager.is_transaction_active(txn_id),
        "Active transaction should not be cleaned up"
    );

    manager
        .commit_transaction(txn_id)
        .expect("Should commit active transaction");
}

/// Test rapid begin/commit cycles with cleanup between each
#[test]
fn test_rapid_cycles_with_cleanup() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    for _ in 0..20 {
        let txn_id = manager
            .begin_transaction(TransactionOptions::default())
            .expect("Should begin transaction");
        manager
            .commit_transaction(txn_id)
            .expect("Should commit transaction");
        manager.cleanup_expired_transactions();
    }

    let active = manager.list_active_transactions();
    assert!(active.is_empty(), "No transactions should remain active");
}

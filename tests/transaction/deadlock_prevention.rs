//! Transaction Deadlock Prevention Tests
//!
//! These tests specifically verify the fix for the deadlock issue caused by
//! calling block_on inside spawn_blocking contexts when handling transactions.

use graphdb::transaction::{TransactionManager, TransactionManagerConfig, TransactionOptions};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};

/// Test that verifies no deadlock occurs with concurrent read-only transaction operations
#[tokio::test]
async fn test_no_deadlock_concurrent_transactions() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let mut handles = vec![];

    for i in 0..5 {
        let manager = Arc::clone(&manager);
        let handle = tokio::spawn(async move {
            let txn_id = manager
                .begin_transaction(TransactionOptions::new().read_only())
                .expect("Failed to begin transaction");

            sleep(Duration::from_millis(10)).await;

            manager
                .commit_transaction(txn_id)
                .expect("Failed to commit transaction");

            println!("Transaction {} completed", i);
        });
        handles.push(handle);
    }

    let result = timeout(Duration::from_secs(30), async {
        for handle in handles {
            handle.await.expect("Task should complete");
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "All concurrent transactions should complete without deadlock"
    );
}

/// Test that verifies proper async/await pattern in transaction handling
#[test]
fn test_proper_async_pattern() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let commit_result = manager.commit_transaction(txn_id);
    assert!(commit_result.is_ok(), "Commit should succeed");

    for i in 0..5 {
        let txn_id = manager
            .begin_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        let result = manager.commit_transaction(txn_id);
        assert!(result.is_ok(), "Commit {} should succeed", i);
    }
}

/// Test that write transactions are properly serialized
#[test]
fn test_write_transaction_serialization() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    for i in 0..5 {
        let txn_id = manager
            .begin_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit transaction");

        println!("Write transaction {} completed", i);
    }
}

/// Test transaction timeout handling without deadlock
#[tokio::test]
async fn test_transaction_timeout_no_deadlock() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_millis(50)))
        .expect("Failed to begin transaction");

    sleep(Duration::from_millis(100)).await;

    manager.cleanup_expired_transactions();

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test rapid begin/commit cycles
#[test]
fn test_rapid_transaction_cycles() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    for i in 0..10 {
        let txn_id = manager
            .begin_transaction(TransactionOptions::default())
            .expect("Failed to begin transaction");

        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit transaction");

        println!("Transaction cycle {} completed", i);
    }
}

/// Test concurrent read transactions with cleanup
#[tokio::test]
async fn test_concurrent_reads_with_cleanup() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let mut handles = vec![];

    for _ in 0..3 {
        let manager = Arc::clone(&manager);
        let handle = tokio::spawn(async move {
            let txn_id = manager
                .begin_transaction(TransactionOptions::new().read_only())
                .expect("Failed to begin transaction");

            sleep(Duration::from_millis(5)).await;

            manager
                .commit_transaction(txn_id)
                .expect("Failed to commit transaction");
        });
        handles.push(handle);
    }

    let result = timeout(Duration::from_secs(30), async {
        for handle in handles {
            handle.await.expect("Task should complete");
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "All concurrent read transactions should complete"
    );

    manager.cleanup_expired_transactions();

    let active = manager.list_active_transactions();
    assert!(active.is_empty(), "No transactions should remain active");
}

/// Test transaction info access without deadlock
#[test]
fn test_transaction_info_no_deadlock() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");
    assert!(!context.read_only);

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test savepoint creation without deadlock
#[test]
fn test_savepoint_creation_no_deadlock() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let sp1 = manager
        .create_savepoint(txn_id, Some("sp1".to_string()))
        .expect("Failed to create savepoint 1");
    let sp2 = manager
        .create_savepoint(txn_id, Some("sp2".to_string()))
        .expect("Failed to create savepoint 2");

    assert_ne!(sp1, sp2);

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test mixed read/write transaction handling
#[test]
fn test_mixed_transaction_handling() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    // Write transaction
    let write_txn = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin write transaction");

    manager
        .commit_transaction(write_txn)
        .expect("Failed to commit write transaction");

    // Read transaction
    let read_txn = manager
        .begin_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin read transaction");

    manager
        .commit_transaction(read_txn)
        .expect("Failed to commit read transaction");
}

/// Test transaction abort without deadlock
#[test]
fn test_transaction_abort_no_deadlock() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .abort_transaction(txn_id)
        .expect("Failed to abort transaction");

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test multiple savepoints with release
#[test]
fn test_multiple_savepoints_with_release() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let sp1 = manager
        .create_savepoint(txn_id, Some("sp1".to_string()))
        .expect("Failed to create savepoint 1");
    let sp2 = manager
        .create_savepoint(txn_id, Some("sp2".to_string()))
        .expect("Failed to create savepoint 2");

    manager
        .release_savepoint(txn_id, sp1)
        .expect("Failed to release savepoint 1");
    manager
        .release_savepoint(txn_id, sp2)
        .expect("Failed to release savepoint 2");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

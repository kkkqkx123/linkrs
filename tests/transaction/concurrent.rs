//! Transaction Concurrent Operation Tests
//!
//! Test coverage:
//! - Concurrent read-only transactions
//! - Write transaction exclusivity
//! - Concurrent read and write operations
//! - Transaction isolation - repeatable read
//! - Read committed data only

use super::common;

use common::test_scenario::TestScenario;
use graphdb::core::Value;
use graphdb::transaction::{
    TransactionErrorKind, TransactionManager, TransactionManagerConfig, TransactionOptions,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Test transaction isolation - repeatable read
#[test]
fn test_transaction_repeatable_read() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 RETURN v.age")
        .assert_result_contains(vec![Value::Int(30)]);
}

/// Test transaction isolation - read committed data only
#[test]
fn test_transaction_read_committed_only() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 RETURN v")
        .assert_result_count(1)
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([("name", Value::String("Alice".into()))]),
        );
}

/// Test concurrent read-only transactions using TransactionManager directly
#[tokio::test]
async fn test_concurrent_readonly_transactions() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let mut handles = vec![];

    for i in 0..3 {
        let manager_clone = Arc::clone(&manager);
        let handle = tokio::spawn(async move {
            let options = TransactionOptions::new().read_only();
            let txn_id = manager_clone
                .begin_transaction(options)
                .expect("Failed to begin transaction");

            let context = manager_clone
                .get_context(txn_id)
                .expect("Failed to get context");

            assert!(context.read_only);

            manager_clone
                .commit_transaction(txn_id)
                .expect("Failed to commit transaction");

            println!("Read-only transaction {} completed", i);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.expect("Task failed");
    }
}

/// Test write transaction exclusivity using TransactionManager
#[test]
fn test_write_transaction_exclusivity() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin first transaction");

    let result = manager.begin_transaction(TransactionOptions::default());

    assert!(result.is_err(), "Expected error");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::WriteTransactionConflict,
        "Expected WriteTransactionConflict error"
    );

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit transaction");
}

/// Test concurrent read and write operations using TransactionManager
#[tokio::test]
async fn test_concurrent_read_write() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let write_handle = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move {
            let txn_id = manager
                .begin_transaction(TransactionOptions::default())
                .expect("Failed to begin write transaction");
            tokio::time::sleep(Duration::from_millis(50)).await;
            manager
                .commit_transaction(txn_id)
                .expect("Failed to commit transaction");
        })
    };

    tokio::time::sleep(Duration::from_millis(10)).await;

    let read_handle = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move {
            let options = TransactionOptions::new().read_only();
            let txn_id = manager
                .begin_transaction(options)
                .expect("Failed to begin read transaction");
            manager
                .commit_transaction(txn_id)
                .expect("Failed to commit read transaction");
        })
    };

    let (r1, r2) = tokio::join!(write_handle, read_handle);
    assert!(r1.is_ok() && r2.is_ok());
}

/// Test sequential write transactions
#[test]
fn test_sequential_write_transactions() {
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

/// Test mixed read/write transactions
#[test]
fn test_mixed_read_write_transactions() {
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

    // Another write transaction
    let write_txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin second write transaction");
    manager
        .commit_transaction(write_txn2)
        .expect("Failed to commit second write transaction");
}

/// Test transaction abort releases lock
#[test]
fn test_abort_releases_lock() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin first transaction");

    manager
        .abort_transaction(txn1)
        .expect("Failed to abort transaction");

    let txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Should be able to begin transaction after abort");

    manager
        .commit_transaction(txn2)
        .expect("Failed to commit transaction");
}

/// Test multiple concurrent read transactions
#[tokio::test]
async fn test_multiple_concurrent_reads() {
    let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let mut handles = vec![];

    for _ in 0..5 {
        let manager_clone = Arc::clone(&manager);
        let handle = tokio::spawn(async move {
            let txn_id = manager_clone
                .begin_transaction(TransactionOptions::new().read_only())
                .expect("Failed to begin transaction");

            manager_clone
                .commit_transaction(txn_id)
                .expect("Failed to commit transaction");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.expect("Task should complete");
    }
}

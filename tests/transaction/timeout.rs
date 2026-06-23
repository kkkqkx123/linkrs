//! Transaction Timeout Tests
//!
//! Test coverage:
//! - Transaction timeout handling
//! - Query timeout
//! - Statement timeout
//! - Idle timeout

use graphdb::transaction::{TransactionManager, TransactionManagerConfig, TransactionOptions};
use std::time::Duration;

/// Test transaction with timeout handling
#[tokio::test]
async fn test_transaction_timeout_handling() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_timeout(Duration::from_millis(50));

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = manager.commit_transaction(txn_id);
    assert!(
        result.is_err() || manager.get_context(txn_id).is_err(),
        "Transaction should have timed out or been cleaned up"
    );
}

/// Test transaction with query timeout
#[test]
fn test_transaction_query_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_query_timeout(Duration::from_secs(5));

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    assert!(context.query_timeout.is_some());
    assert_eq!(context.query_timeout.unwrap(), Duration::from_secs(5));

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test transaction with statement timeout
#[test]
fn test_transaction_statement_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_statement_timeout(Duration::from_secs(1));

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    assert!(context.statement_timeout.is_some());
    assert_eq!(context.statement_timeout.unwrap(), Duration::from_secs(1));

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test transaction with idle timeout
#[test]
fn test_transaction_idle_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_idle_timeout(Duration::from_secs(30));

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    assert!(context.idle_timeout.is_some());
    assert_eq!(context.idle_timeout.unwrap(), Duration::from_secs(30));

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test transaction cleanup of expired transactions
#[test]
fn test_cleanup_expired_transactions() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_millis(10)))
        .expect("Failed to begin transaction");

    std::thread::sleep(Duration::from_millis(50));

    manager.cleanup_expired_transactions();

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test transaction with long timeout
#[test]
fn test_transaction_long_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_timeout(Duration::from_secs(300));

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test transaction without timeout
#[test]
fn test_transaction_no_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

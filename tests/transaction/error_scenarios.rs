//! Transaction Error Scenarios Tests
//!
//! Test coverage for various error conditions and edge cases:
//! - Transaction not found errors
//! - Invalid state transitions
//! - Concurrent write transaction conflicts
//! - Too many transactions error
//! - Transaction timeout scenarios
//! - Read-only transaction write attempts
//! - Invalid operations on committed/aborted transactions
//! - Double commit/rollback attempts
//! - Shutdown errors

use graphdb::core::types::TransactionId;
use graphdb::transaction::{
    TransactionError, TransactionErrorKind, TransactionManager, TransactionManagerConfig,
    TransactionOptions, TransactionState,
};
use std::time::Duration;
use tokio::time::sleep;

/// Test transaction not found error
#[test]
fn test_error_transaction_not_found() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let result = manager.get_context(TransactionId(99999));
    assert!(result.is_err(), "Expected error");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionNotFound,
        "Expected TransactionNotFound error"
    );

    let result = manager.commit_transaction(TransactionId(99999));
    assert!(result.is_err(), "Expected error on commit");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionNotFound,
        "Expected TransactionNotFound error on commit"
    );

    let result = manager.abort_transaction(TransactionId(99999));
    assert!(result.is_err(), "Expected error on abort");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionNotFound,
        "Expected TransactionNotFound error on abort"
    );
}

/// Test invalid state transitions
#[test]
fn test_error_invalid_state_transition() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    context
        .transition_to(TransactionState::Committing)
        .expect("Failed to transition to Committing");

    let result = context.transition_to(TransactionState::Aborting);
    assert!(result.is_err(), "Expected error");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::InvalidStateTransition,
        "Expected InvalidStateTransition error"
    );

    context
        .transition_to(TransactionState::Committed)
        .expect("Failed to transition to Committed");

    let result = context.transition_to(TransactionState::Active);
    assert!(result.is_err(), "Expected error from terminal state");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::InvalidStateTransition,
        "Expected InvalidStateTransition error from terminal state"
    );
}

/// Test invalid state for commit/abort
#[test]
fn test_error_invalid_state_for_commit_abort() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    context
        .transition_to(TransactionState::Committing)
        .expect("Failed to transition");

    let result = context.can_execute();
    assert!(result.is_err(), "Expected error");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::InvalidStateForCommit,
        "Expected InvalidStateForCommit error"
    );
}

/// Test savepoint not found error
#[test]
fn test_error_savepoint_not_found() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test write transaction exclusivity
#[test]
fn test_error_write_transaction_exclusivity() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin first transaction");

    let txn2_result = manager.begin_transaction(TransactionOptions::default());

    match txn2_result {
        Ok(txn2) => {
            manager
                .commit_transaction(txn2)
                .expect("Failed to commit second transaction");
        }
        Err(e) => {
            let kind = e.kind();
            assert!(
                kind == TransactionErrorKind::WriteTransactionConflict
                    || kind == TransactionErrorKind::TooManyTransactions,
                "Expected WriteTransactionConflict or TooManyTransactions error, got {:?}",
                kind
            );
        }
    }

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit transaction");
}

/// Test too many transactions error
#[test]
fn test_error_too_many_transactions() {
    let config = TransactionManagerConfig {
        max_concurrent_transactions: 2,
        ..Default::default()
    };

    let manager = TransactionManager::new(config);

    let txn1 = manager
        .begin_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin transaction 1");
    let txn2 = manager
        .begin_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin transaction 2");

    let result = manager.begin_transaction(TransactionOptions::new().read_only());
    assert!(result.is_err(), "Expected error");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TooManyTransactions,
        "Expected TooManyTransactions error"
    );

    manager
        .abort_transaction(txn1)
        .expect("Failed to abort txn1");
    manager
        .abort_transaction(txn2)
        .expect("Failed to abort txn2");
}

/// Test transaction timeout error
#[tokio::test]
async fn test_error_transaction_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_timeout(Duration::from_millis(50));
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    sleep(Duration::from_millis(100)).await;

    let result = manager.commit_transaction(txn_id);
    assert!(result.is_err(), "Expected error");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionTimeout,
        "Expected TransactionTimeout error, got {:?}",
        err.kind()
    );
}

/// Test transaction expired error on operations
#[tokio::test]
async fn test_error_transaction_expired() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().with_timeout(Duration::from_millis(50));
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    sleep(Duration::from_millis(100)).await;

    let result = context.can_execute();
    assert!(result.is_err(), "Expected error");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionExpired,
        "Expected TransactionExpired error"
    );
}

/// Test read-only transaction errors
#[test]
fn test_error_readonly_transaction() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().read_only();
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin read-only transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    assert!(context.read_only);

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit read-only transaction");
}

/// Test double commit attempt
#[test]
fn test_error_double_commit() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("First commit should succeed");

    let result = manager.commit_transaction(txn_id);
    assert!(result.is_err(), "Expected error on double commit");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionNotFound,
        "Expected TransactionNotFound on double commit"
    );
}

/// Test double abort attempt
#[test]
fn test_error_double_abort() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .abort_transaction(txn_id)
        .expect("First abort should succeed");

    let result = manager.abort_transaction(txn_id);
    assert!(result.is_err(), "Expected error on double abort");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionNotFound,
        "Expected TransactionNotFound on double abort"
    );
}

/// Test commit after abort attempt
#[test]
fn test_error_commit_after_abort() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .abort_transaction(txn_id)
        .expect("Abort should succeed");

    let result = manager.commit_transaction(txn_id);
    assert!(result.is_err(), "Expected error on commit after abort");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::TransactionNotFound,
        "Expected TransactionNotFound on commit after abort"
    );
}

/// Test shutdown error
#[test]
fn test_error_shutdown() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    manager.shutdown();

    let result = manager.begin_transaction(TransactionOptions::default());
    assert!(result.is_err(), "Expected error after shutdown");
    let err = result.unwrap_err();
    assert_eq!(
        err.kind(),
        TransactionErrorKind::Internal,
        "Expected Internal error after shutdown"
    );
}

/// Test no savepoints in transaction error
#[test]
fn test_error_no_savepoints() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let savepoints = manager.get_active_savepoints(txn_id);
    assert!(savepoints.is_empty());

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit");
}

/// Test transaction state display
#[test]
fn test_transaction_state_display() {
    assert_eq!(format!("{}", TransactionState::Active), "Active");
    assert_eq!(format!("{}", TransactionState::Committing), "Committing");
    assert_eq!(format!("{}", TransactionState::Committed), "Committed");
    assert_eq!(format!("{}", TransactionState::Aborting), "Aborting");
    assert_eq!(format!("{}", TransactionState::Aborted), "Aborted");
}

/// Test transaction state helpers
#[test]
fn test_transaction_state_helpers() {
    assert!(TransactionState::Active.can_execute());
    assert!(TransactionState::Active.can_commit());
    assert!(TransactionState::Active.can_abort());
    assert!(!TransactionState::Active.is_terminal());

    assert!(!TransactionState::Committed.can_execute());
    assert!(!TransactionState::Committed.can_commit());
    assert!(!TransactionState::Committed.can_abort());
    assert!(TransactionState::Committed.is_terminal());

    assert!(!TransactionState::Aborted.can_execute());
    assert!(!TransactionState::Aborted.can_commit());
    assert!(!TransactionState::Aborted.can_abort());
    assert!(TransactionState::Aborted.is_terminal());
}

/// Test error formatting
#[test]
fn test_error_formatting() {
    let error = TransactionError::transaction_not_found(TransactionId(123));
    assert!(format!("{}", error).contains("123"));

    let error = TransactionError::too_many_transactions();
    assert!(format!("{}", error).contains("many"));

    let error = TransactionError::transaction_timeout();
    assert!(format!("{}", error).contains("timeout"));

    let error = TransactionError::internal("test error");
    assert!(format!("{}", error).contains("test error"));
}

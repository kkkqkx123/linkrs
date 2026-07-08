//! Two-Phase Commit Transaction Tests
//!
//! Test coverage for two-phase commit functionality:
//! - Basic two-phase commit flow
//! - Multiple transactions with two-phase commit
//! - Transaction options with two-phase commit

use graphdb::transaction::{TransactionManager, TransactionManagerConfig, TransactionOptions};
use std::time::Duration;

/// Test basic two-phase commit flow
#[test]
fn test_two_phase_commit_basic() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let mut options = TransactionOptions::new();
    options.two_phase_commit = true;
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    assert!(manager.is_transaction_active(txn_id));

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test two-phase commit with multiple operations
#[test]
fn test_two_phase_commit_multiple_operations() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let mut options = TransactionOptions::new();
    options.two_phase_commit = true;
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager.get_context(txn_id).expect("Failed to get context");

    for i in 0..5 {
        let operation = graphdb::transaction::OperationLog::InsertVertex {
            space: "test_space".to_string(),
            vertex_id: vec![i as u8, 0, 0, 0, 0, 0, 0, 0],
            previous_state: None,
        };
        context.add_operation_log(operation);
    }

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test two-phase commit rollback
#[test]
fn test_two_phase_commit_rollback() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let mut options = TransactionOptions::new();
    options.two_phase_commit = true;
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    assert!(manager.is_transaction_active(txn_id));

    manager
        .abort_transaction(txn_id)
        .expect("Failed to abort transaction");

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test two-phase commit with timeout
#[test]
fn test_two_phase_commit_with_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let mut options = TransactionOptions::new().with_timeout(Duration::from_secs(60));
    options.two_phase_commit = true;

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test two-phase commit with read-only
#[test]
fn test_two_phase_commit_readonly() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let options = TransactionOptions::new().read_only();

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test multiple sequential two-phase commits
#[test]
fn test_multiple_sequential_two_phase_commits() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    for i in 0..5 {
        let mut options = TransactionOptions::new();
        options.two_phase_commit = true;
        let txn_id = manager
            .begin_transaction(options)
            .unwrap_or_else(|_| panic!("Failed to begin transaction {}", i));

        manager
            .commit_transaction(txn_id)
            .unwrap_or_else(|_| panic!("Failed to commit transaction {}", i));
    }

    let active = manager.list_active_transactions();
    assert!(
        active.is_empty(),
        "No transactions should be active after all commits"
    );
}

/// Test two-phase commit with different durability levels
#[test]
fn test_two_phase_commit_durability_levels() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let mut options1 = TransactionOptions::new();
    options1.two_phase_commit = true;
    options1.durability = graphdb::transaction::DurabilityLevel::Sync;
    let txn1 = manager
        .begin_transaction(options1)
        .expect("Failed to begin transaction 1");
    manager
        .commit_transaction(txn1)
        .expect("Failed to commit transaction 1");

    let mut options2 = TransactionOptions::new();
    options2.two_phase_commit = true;
    options2.durability = graphdb::transaction::DurabilityLevel::None;
    let txn2 = manager
        .begin_transaction(options2)
        .expect("Failed to begin transaction 2");
    manager
        .commit_transaction(txn2)
        .expect("Failed to commit transaction 2");
}

/// Test two-phase commit shutdown with pending transaction
#[test]
fn test_two_phase_commit_shutdown_with_pending() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let mut options = TransactionOptions::new();
    options.two_phase_commit = true;
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    assert!(manager.is_transaction_active(txn_id));

    manager.shutdown();

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test transaction state display
#[test]
fn test_transaction_state_display() {
    use graphdb::transaction::TransactionState;

    assert_eq!(format!("{}", TransactionState::Active), "Active");
    assert_eq!(format!("{}", TransactionState::Committing), "Committing");
    assert_eq!(format!("{}", TransactionState::Committed), "Committed");
    assert_eq!(format!("{}", TransactionState::Aborting), "Aborting");
    assert_eq!(format!("{}", TransactionState::Aborted), "Aborted");
}

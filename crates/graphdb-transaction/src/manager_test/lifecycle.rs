//! Transaction manager lifecycle, capacity and observability tests.

use std::sync::Arc;
use std::time::Duration;

use super::create_test_manager;
use crate::manager::TransactionManager;
use crate::mvcc::VersionManager;
use crate::types::*;
use crate::TransactionErrorKind;
#[test]
fn test_transaction_manager_creation() {
    let manager = create_test_manager();

    let config = manager.config();
    assert_eq!(config.max_concurrent_transactions, 1000);
    assert!(!config.auto_cleanup);
}

#[test]
fn test_begin_write_transaction() {
    let manager = create_test_manager();

    let options = TransactionOptions::default();
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    assert!(manager.is_transaction_active(txn_id));

    let context = manager
        .get_context(txn_id)
        .expect("Failed to get transaction context");
    assert_eq!(context.id, txn_id);
    assert_eq!(context.state(), TransactionState::Active);
    assert!(!context.read_only);
}

#[test]
fn test_begin_readonly_transaction() {
    let manager = create_test_manager();

    let options = TransactionOptions::new().read_only();
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin readonly transaction");

    assert!(manager.is_transaction_active(txn_id));

    let context = manager
        .get_context(txn_id)
        .expect("Failed to get transaction context");
    assert_eq!(context.id, txn_id);
    assert!(context.read_only);
}

#[test]
fn test_begin_transaction_with_timeout() {
    let manager = create_test_manager();

    let options = TransactionOptions::new()
        .with_timeout(Duration::from_secs(60))
        .with_durability(DurabilityLevel::None);

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    let context = manager
        .get_context(txn_id)
        .expect("Failed to get transaction context");
    assert!(context.remaining_time() > Duration::from_secs(50));
}

#[test]
fn test_commit_transaction() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    assert!(manager.is_transaction_active(txn_id));

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");

    assert!(!manager.is_transaction_active(txn_id));

    let stats = manager.stats();
    assert_eq!(
        stats
            .committed_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn test_abort_transaction() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to start transaction");

    assert!(manager.is_transaction_active(txn_id));

    manager
        .abort_transaction(txn_id)
        .expect("Failed to abort transaction");

    assert!(!manager.is_transaction_active(txn_id));

    let stats = manager.stats();
    assert_eq!(
        stats
            .aborted_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn test_commit_readonly_transaction() {
    let manager = create_test_manager();

    let options = TransactionOptions::new().read_only();
    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin readonly transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit readonly transaction");

    assert!(!manager.is_transaction_active(txn_id));
}

#[test]
fn test_multiple_readonly_transactions() {
    let manager = create_test_manager();

    let options = TransactionOptions::new().read_only();

    let txn1 = manager
        .begin_transaction(options.clone())
        .expect("Failed to begin first readonly transaction");
    let txn2 = manager
        .begin_transaction(options.clone())
        .expect("Failed to begin second readonly transaction");
    let txn3 = manager
        .begin_transaction(options)
        .expect("Failed to begin third readonly transaction");

    assert!(manager.is_transaction_active(txn1));
    assert!(manager.is_transaction_active(txn2));
    assert!(manager.is_transaction_active(txn3));

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit first readonly transaction");
    manager
        .commit_transaction(txn2)
        .expect("Failed to commit second readonly transaction");
    manager
        .commit_transaction(txn3)
        .expect("Failed to commit third readonly transaction");
}

#[test]
fn test_sequential_transactions() {
    let manager = create_test_manager();

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin first transaction");
    manager
        .commit_transaction(txn1)
        .expect("Failed to commit first transaction");

    let txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin second transaction");
    manager
        .abort_transaction(txn2)
        .expect("Failed to abort second transaction");

    let txn3 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin third transaction");
    manager
        .commit_transaction(txn3)
        .expect("Failed to commit third transaction");

    let stats = manager.stats();
    assert_eq!(
        stats
            .committed_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(
        stats
            .aborted_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn test_transaction_timeout() {
    let manager = create_test_manager();

    let options = TransactionOptions::new().with_timeout(Duration::from_millis(50));

    let txn_id = manager
        .begin_transaction(options)
        .expect("Failed to begin transaction");

    std::thread::sleep(Duration::from_millis(100));

    let result = manager.commit_transaction(txn_id);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionTimeout);

    let stats = manager.stats();
    assert_eq!(
        stats
            .timeout_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn test_list_active_transactions() {
    let manager = create_test_manager();

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin first transaction");
    let txn2 = manager
        .begin_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin second transaction");

    let active_txns = manager.list_active_transactions();
    assert_eq!(active_txns.len(), 2);

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit transaction");

    let active_txns = manager.list_active_transactions();
    assert_eq!(active_txns.len(), 1);

    manager
        .commit_transaction(txn2)
        .expect("Failed to commit transaction");
}

#[test]
fn test_get_transaction_info() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let info = manager
        .get_transaction_info(txn_id)
        .expect("Failed to get transaction info");

    assert_eq!(info.id, txn_id);
    assert_eq!(info.state, TransactionState::Active);
    assert!(!info.is_read_only);

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

#[test]
fn test_max_concurrent_transactions() {
    let config = TransactionManagerConfig {
        max_concurrent_transactions: 2,
        auto_cleanup: false,
        ..Default::default()
    };

    let manager = TransactionManager::new(config);

    let txn1 = manager
        .begin_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin first transaction");

    let txn2 = manager
        .begin_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin second transaction");

    let result = manager.begin_transaction(TransactionOptions::new().read_only());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TooManyTransactions);

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit first transaction");
    manager
        .commit_transaction(txn2)
        .expect("Failed to commit second transaction");
}

#[test]
fn test_transaction_stats() {
    let manager = create_test_manager();

    let stats = manager.stats();

    assert_eq!(
        stats
            .total_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        stats
            .active_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        stats
            .committed_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        stats
            .aborted_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    assert_eq!(
        stats
            .total_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        stats
            .active_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit transaction");

    assert_eq!(
        stats
            .active_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        stats
            .committed_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    let txn2 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .abort_transaction(txn2)
        .expect("Failed to abort transaction");

    assert_eq!(
        stats
            .aborted_transactions
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn test_cleanup_expired_transactions() {
    let manager = create_test_manager();

    let txn1 = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_millis(50)))
        .expect("Failed to begin transaction");

    std::thread::sleep(Duration::from_millis(100));

    manager.cleanup_expired_transactions();

    assert!(!manager.is_transaction_active(txn1));
    assert_eq!(manager.pending_count(), 0);
}

#[test]
fn test_shutdown_manager() {
    let manager = create_test_manager();

    let txn1 = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin first transaction");
    let txn2 = manager
        .begin_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin second transaction");

    manager.shutdown();

    assert!(!manager.is_transaction_active(txn1));
    assert!(!manager.is_transaction_active(txn2));

    let result = manager.begin_transaction(TransactionOptions::default());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::Internal);
}

#[test]
fn test_read_insert_transaction_types() {
    let manager = create_test_manager();

    let read_txn = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin read transaction");

    let insert_txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin insert transaction");

    assert!(manager.is_transaction_active(read_txn));
    assert!(manager.is_transaction_active(insert_txn));

    let read_ctx = manager
        .get_context(read_txn)
        .expect("Failed to get read context");
    assert!(read_ctx.read_only);

    let insert_ctx = manager
        .get_context(insert_txn)
        .expect("Failed to get insert context");
    assert!(!insert_ctx.read_only);

    manager
        .commit_transaction(read_txn)
        .expect("Failed to commit read transaction");
    manager
        .commit_transaction(insert_txn)
        .expect("Failed to commit insert transaction");
}

#[test]
fn test_version_manager_integration() {
    let manager = create_test_manager();

    let read_txn = manager
        .begin_read_transaction(TransactionOptions::new().read_only())
        .expect("Failed to begin read transaction");

    let insert_txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin insert transaction");

    assert!(manager.pending_count() >= 0);

    manager
        .commit_transaction(read_txn)
        .expect("Failed to commit read transaction");
    manager
        .commit_transaction(insert_txn)
        .expect("Failed to commit insert transaction");
}

#[test]
fn test_shared_version_manager_is_preserved() {
    let version_manager = Arc::new(VersionManager::new());
    let manager = TransactionManager::with_shared_version_manager(
        TransactionManagerConfig::default(),
        Arc::new(graphdb_metrics::StatsManager::new()),
        Arc::clone(&version_manager),
    );

    assert!(Arc::ptr_eq(manager.version_manager(), &version_manager));
}

#[test]
fn test_successful_commit_reaches_committed_terminal_state() {
    let manager = create_test_manager();
    let txn_id = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("transaction should begin");
    let context = manager.get_context(txn_id).expect("context should exist");

    manager
        .commit_transaction(txn_id)
        .expect("commit should succeed");

    // Terminal state is observable through a retained handle even after the
    // transaction leaves the active table.
    assert_eq!(context.state(), TransactionState::Committed);
    assert!(context.state().is_terminal());
    assert!(manager.get_context(txn_id).is_err());
    assert_ne!(context.commit_timestamp(), 0);
}

#[test]
fn test_out_of_order_commits_advance_frontier_by_commit_time() {
    let manager = create_test_manager();
    let first = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("first should begin");
    let second = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("second should begin");

    // Commit in reverse start order. Visibility is ordered by commit time, so
    // the later-started transaction committing first must not stall the
    // frontier behind the earlier-started one.
    manager
        .commit_transaction(second)
        .expect("second should commit");
    let frontier_after_second = manager.read_timestamp();
    manager
        .commit_transaction(first)
        .expect("first should commit");
    let frontier_after_first = manager.read_timestamp();

    assert!(frontier_after_first > frontier_after_second);
    let ctx_second = manager.get_context(second);
    assert!(
        ctx_second.is_err(),
        "committed transactions leave the table"
    );
}

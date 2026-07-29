//! TransactionManager Tests
//!
//! Test transaction manager functionality, including transaction lifecycle management, concurrency control, timeout handling, etc.

use std::sync::Arc;
use std::time::Duration;

use crate::transaction::manager::TransactionManager;
use crate::transaction::mvcc::VersionManager;
use crate::transaction::types::{
    DurabilityLevel, TransactionId, TransactionManagerConfig, TransactionOptions, TransactionState,
};
use crate::transaction::TransactionErrorKind;

fn create_test_manager() -> TransactionManager {
    let config = TransactionManagerConfig {
        auto_cleanup: false,
        ..Default::default()
    };
    TransactionManager::new(config)
}

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
fn test_get_transaction_not_found() {
    let manager = create_test_manager();

    let result = manager.get_context(TransactionId(9999));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_commit_transaction_not_found() {
    let manager = create_test_manager();

    let result = manager.commit_transaction(TransactionId(9999));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_abort_transaction_not_found() {
    let manager = create_test_manager();

    let result = manager.abort_transaction(TransactionId(9999));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_commit_already_committed_transaction() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("First commit failed");

    let result = manager.commit_transaction(txn_id);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
}

#[test]
fn test_abort_already_aborted_transaction() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to start transaction");

    manager
        .abort_transaction(txn_id)
        .expect("First abort failed");

    let result = manager.abort_transaction(txn_id);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), TransactionErrorKind::TransactionNotFound);
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
fn test_check_write_set_conflict_no_conflict() {
    use crate::core::types::VertexId;

    let manager = create_test_manager();

    // Create two write transactions with different vertex writes
    let txn1 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn1");

    let txn2 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn2");

    // Record different vertex writes
    let ctx1 = manager.get_context(txn1).expect("Failed to get context 1");
    let ctx2 = manager.get_context(txn2).expect("Failed to get context 2");

    let vid1 = VertexId::from_int64(1);
    let vid2 = VertexId::from_int64(2);

    ctx1.record_vertex_write(vid1);
    ctx2.record_vertex_write(vid2);

    // Check conflict - should be Ok because writes are on different vertices
    let conflict_check = manager.check_write_set_conflict(txn1);
    assert!(
        conflict_check.is_ok(),
        "Should be no conflict for different vertices"
    );

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit txn1");
    manager
        .commit_transaction(txn2)
        .expect("Failed to commit txn2");
}

#[test]
fn test_check_write_set_conflict_with_conflict() {
    use crate::core::types::VertexId;

    let manager = create_test_manager();

    // Create two write transactions with same vertex writes
    let txn1 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn1");

    let txn2 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn2");

    // Record same vertex write
    let ctx1 = manager.get_context(txn1).expect("Failed to get context 1");
    let ctx2 = manager.get_context(txn2).expect("Failed to get context 2");

    let vid = VertexId::from_int64(1);

    ctx1.record_vertex_write(vid);
    ctx2.record_vertex_write(vid);

    // txn1 checks first and passes (first-writer-wins)
    let conflict_check1 = manager.check_write_set_conflict(txn1);
    assert!(
        conflict_check1.is_ok(),
        "First transaction should pass conflict check"
    );

    // txn2 checks second and fails (conflicts with validated txn1)
    let conflict_check2 = manager.check_write_set_conflict(txn2);
    assert!(
        conflict_check2.is_err(),
        "Second transaction should detect conflict with first"
    );

    manager
        .commit_transaction(txn1)
        .expect("Failed to commit txn1");
    manager
        .commit_transaction(txn2)
        .expect("Failed to commit txn2");
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
        Arc::new(crate::core::stats::StatsManager::new()),
        Arc::clone(&version_manager),
    );

    assert!(Arc::ptr_eq(manager.version_manager(), &version_manager));
}

#[test]
fn test_savepoint_basic() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let sp_id = manager
        .create_savepoint(txn_id, Some("test_savepoint".to_string()))
        .expect("Failed to create savepoint");

    let sp = manager
        .get_savepoint(txn_id, sp_id)
        .expect("Failed to get savepoint");
    assert_eq!(sp.name, Some("test_savepoint".to_string()));

    manager
        .release_savepoint(txn_id, sp_id)
        .expect("Failed to release savepoint");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

#[test]
fn test_read_committed_refreshes_statement_snapshot() {
    let manager = create_test_manager();
    let reader = manager
        .begin_read_transaction(
            TransactionOptions::default()
                .read_only()
                .with_isolation_level(crate::transaction::IsolationLevel::ReadCommitted),
        )
        .expect("reader should begin");
    let initial = manager
        .get_context(reader)
        .expect("reader context should exist")
        .effective_snapshot_timestamp();

    let writer = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("writer should begin");
    manager
        .commit_transaction(writer)
        .expect("writer should commit");

    let (context, statement_start) = manager
        .begin_statement(reader)
        .expect("reader statement should begin");
    assert!(context.effective_snapshot_timestamp() > initial);
    manager
        .finish_statement(&context, statement_start)
        .expect("reader statement should finish");
    manager
        .commit_transaction(reader)
        .expect("reader should commit");
}

#[test]
fn test_owner_is_required_for_kill() {
    let manager = create_test_manager();
    let txn_id = manager
        .begin_transaction_with_owner(TransactionOptions::default(), "session-1")
        .expect("transaction should begin");

    let error = manager
        .kill_transaction(txn_id, Some("session-2"))
        .expect_err("a different owner must not kill the transaction");
    assert_eq!(error.kind(), TransactionErrorKind::TransactionNotOwner);
    manager
        .kill_transaction(txn_id, Some("session-1"))
        .expect("the owner should be able to kill the transaction");
}

#[test]
fn test_cross_shard_final_review_no_false_abort() {
    use std::sync::Barrier;
    use std::thread;

    use crate::core::types::{CommitLsn, VertexId};
    use crate::transaction::participant::{
        TransactionAbortDescriptor, TransactionCommitDescriptor, TransactionCommitSink,
    };

    struct PassThroughSink {
        barrier: Arc<Barrier>,
    }

    impl TransactionCommitSink for PassThroughSink {
        fn commit_transaction(&self, _tid: TransactionId) -> Result<CommitLsn, String> {
            Ok(CommitLsn::new(7))
        }
        fn abort_transaction(&self, _tid: TransactionId) -> Result<(), String> {
            Ok(())
        }
        fn commit_transaction_with_descriptor(
            &self,
            _descriptor: &TransactionCommitDescriptor,
        ) -> Result<CommitLsn, String> {
            self.barrier.wait();
            Ok(CommitLsn::new(7))
        }
        fn abort_transaction_with_descriptor(
            &self,
            _descriptor: &TransactionAbortDescriptor,
        ) -> Result<(), String> {
            Ok(())
        }
        fn finalize_commit(
            &self,
            _descriptor: &TransactionCommitDescriptor,
            _commit_lsn: CommitLsn,
        ) -> Result<(), String> {
            Ok(())
        }
        fn recover_unfinalized_commits(&self) -> Result<usize, String> {
            Ok(0)
        }
    }

    // Two non-conflicting transactions in different cert shards commit
    // concurrently. The sink barrier ensures both pass certification
    // before either enters the publication phase (final review).
    // The final review must NOT false-positive on non-conflicting writes.
    for iteration in 0..20 {
        let barrier = Arc::new(Barrier::new(2));
        let manager = Arc::new(
            TransactionManager::new(TransactionManagerConfig::default())
                .with_commit_sink(Arc::new(PassThroughSink {
                    barrier: Arc::clone(&barrier),
                })),
        );

        let vid1 = VertexId::from_int64(iteration as i64 * 2);
        let vid2 = VertexId::from_int64(iteration as i64 * 2 + 1);

        let txn1 = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("txn1");
        let ctx1 = manager.get_context(txn1).expect("ctx1");
        ctx1.record_vertex_write(vid1);
        drop(ctx1);

        let txn2 = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("txn2");
        let ctx2 = manager.get_context(txn2).expect("ctx2");
        ctx2.record_vertex_write(vid2);
        drop(ctx2);

        let mgr1 = Arc::clone(&manager);
        let mgr2 = Arc::clone(&manager);
        let h1 = thread::spawn(move || mgr1.commit_transaction(txn1));
        let h2 = thread::spawn(move || mgr2.commit_transaction(txn2));

        let r1 = h1.join().expect("thread1");
        let r2 = h2.join().expect("thread2");

        assert!(
            r1.is_ok(),
            "non-conflicting txn1 should commit: {:?}",
            r1
        );
        assert!(
            r2.is_ok(),
            "non-conflicting txn2 should commit: {:?}",
            r2
        );
    }
}

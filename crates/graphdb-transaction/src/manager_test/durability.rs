//! Transaction manager finalization failure, gate balance and recovery tests.

use std::sync::Arc;

use super::create_test_manager;
use crate::manager::TransactionManager;
use crate::types::*;
use crate::TransactionErrorKind;
#[test]
fn test_finalize_failure_keeps_reads_invisible_until_recovery() {
    use std::sync::atomic::AtomicUsize;

    use crate::participant::TransactionCommitSink;
    use graphdb_core::types::CommitLsn;

    struct FlakyFinalizeSink {
        finalize_failures: AtomicUsize,
        finalize_calls: AtomicUsize,
    }

    impl TransactionCommitSink for FlakyFinalizeSink {
        fn commit_transaction(&self, _transaction_id: TransactionId) -> Result<CommitLsn, String> {
            Ok(CommitLsn::new(11))
        }
        fn abort_transaction(&self, _transaction_id: TransactionId) -> Result<(), String> {
            Ok(())
        }
        fn finalize_commit(
            &self,
            _descriptor: &crate::participant::TransactionCommitDescriptor,
            _commit_lsn: CommitLsn,
        ) -> Result<(), String> {
            self.finalize_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self
                .finalize_failures
                .load(std::sync::atomic::Ordering::SeqCst)
                > 0
            {
                self.finalize_failures
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                return Err("injected finalize failure".to_string());
            }
            Ok(())
        }
    }

    let sink = Arc::new(FlakyFinalizeSink {
        finalize_failures: AtomicUsize::new(100),
        finalize_calls: AtomicUsize::new(0),
    });
    let config = TransactionManagerConfig {
        auto_cleanup: false,
        commit_retry_attempts: 0,
        ..Default::default()
    };
    let manager = TransactionManager::new(config).with_commit_sink(sink.clone());

    let txn_id = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("transaction should begin");
    let start_ts = manager
        .get_context(txn_id)
        .expect("context should exist")
        .timestamp();

    let error = manager
        .commit_transaction(txn_id)
        .expect_err("finalize failure must surface as an error");
    assert_eq!(error.kind(), TransactionErrorKind::CommitFailed);

    // The reserved commit timestamp was retired without publishing
    // visibility, and the start slot with it: the frontier advances past
    // both instead of pinning behind the failed commit. The transaction
    // stays Committing and recoverable.
    let context = manager
        .get_context(txn_id)
        .expect("durable transaction stays in the active table");
    assert_eq!(context.state(), TransactionState::Committing);
    assert_eq!(context.commit_timestamp(), 0);
    assert_eq!(manager.read_timestamp(), start_ts + 1);
    assert!(!manager.is_transaction_active(txn_id));

    // Aborting a durable commit is refused.
    let abort_error = manager
        .abort_transaction(txn_id)
        .expect_err("durable commit must not be abortable");
    assert_eq!(abort_error.kind(), TransactionErrorKind::CommitFailed);

    // Re-drive finalization: allow the sink to succeed now.
    sink.finalize_failures
        .store(0, std::sync::atomic::Ordering::SeqCst);
    manager
        .recover_pending_finalization(txn_id)
        .expect("recovery should complete finalization");

    let commit_ts = context.commit_timestamp();
    assert!(commit_ts > start_ts);
    assert!(manager.read_timestamp() >= commit_ts);
    assert_eq!(context.state(), TransactionState::Committed);
    assert!(manager.get_context(txn_id).is_err());
    assert!(
        sink.finalize_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            >= 2
    );
}

#[test]
fn test_gate_lease_released_exactly_once_on_commit_failure_path() {
    use crate::participant::TransactionCommitSink;
    use graphdb_core::types::CommitLsn;

    // Fail every finalize call so the commit stays durable-but-unfinalized.
    struct AlwaysFailFinalizeSink;

    impl TransactionCommitSink for AlwaysFailFinalizeSink {
        fn commit_transaction(&self, _transaction_id: TransactionId) -> Result<CommitLsn, String> {
            Ok(CommitLsn::new(31))
        }
        fn abort_transaction(&self, _transaction_id: TransactionId) -> Result<(), String> {
            Ok(())
        }
        fn finalize_commit(
            &self,
            _descriptor: &crate::participant::TransactionCommitDescriptor,
            _commit_lsn: CommitLsn,
        ) -> Result<(), String> {
            Err("injected finalize failure".to_string())
        }
    }

    let config = TransactionManagerConfig {
        auto_cleanup: false,
        commit_retry_attempts: 0,
        ..Default::default()
    };
    let manager =
        TransactionManager::new(config).with_commit_sink(Arc::new(AlwaysFailFinalizeSink));

    let txn_id = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("transaction should begin");
    assert_eq!(manager.checkpoint_gate().active_write_count(), 1);

    manager
        .commit_transaction(txn_id)
        .expect_err("finalize failure must surface as an error");
    // The commit path released the gate lease before finalization.
    assert_eq!(manager.checkpoint_gate().active_write_count(), 0);
    assert_eq!(manager.active_write_count(), 0);

    // The durable commit cannot be aborted; the refused abort must not touch
    // the gate either (a second release would underflow the counter and wedge
    // checkpoint drain until timeout).
    let abort_error = manager
        .abort_transaction(txn_id)
        .expect_err("durable commit must not be abortable");
    assert_eq!(abort_error.kind(), TransactionErrorKind::CommitFailed);
    assert_eq!(manager.checkpoint_gate().active_write_count(), 0);
    assert_eq!(manager.active_write_count(), 0);

    // A failed recovery re-drive re-queues the pending record without
    // touching the gate; the transaction stays recoverable.
    manager
        .recover_pending_finalization(txn_id)
        .expect_err("finalize still fails");
    assert_eq!(manager.checkpoint_gate().active_write_count(), 0);
    let context = manager
        .get_context(txn_id)
        .expect("durable transaction stays in the active table");
    assert_eq!(context.state(), TransactionState::Committing);
}

#[test]
fn test_double_abort_keeps_gate_balanced() {
    let manager = create_test_manager();

    let txn_id = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("transaction should begin");
    assert_eq!(manager.checkpoint_gate().active_write_count(), 1);

    manager
        .abort_transaction(txn_id)
        .expect("first abort should succeed");
    assert_eq!(manager.checkpoint_gate().active_write_count(), 0);
    assert_eq!(manager.active_write_count(), 0);

    assert!(manager.abort_transaction(txn_id).is_err());
    assert_eq!(manager.checkpoint_gate().active_write_count(), 0);
    assert_eq!(manager.active_write_count(), 0);
}

#[test]
fn test_recovery_sidecar_roundtrip_across_restart() {
    use crate::participant::TransactionCommitDescriptor;
    use crate::recovery::RecoveryManager;
    use graphdb_core::types::{CommitLsn, TransactionId};

    let dir = tempfile::tempdir().expect("tempdir should be created");
    let sidecar = dir.path().join("pending.sidecar");

    let recovery = RecoveryManager::new();
    recovery.set_sidecar_path(&sidecar);
    let descriptor = TransactionCommitDescriptor::new(
        TransactionId(9),
        4,
        crate::DurabilityLevel::Sync,
        crate::WriteSet::new(),
    );
    recovery.record(&descriptor, 0, CommitLsn::new(77));
    // Recording the same transaction twice must not duplicate the record.
    recovery.record(&descriptor, 0, CommitLsn::new(77));
    assert!(sidecar.exists(), "sidecar file should be written");

    // A fresh manager (simulating a restart) consumes the sidecar record.
    let restarted = RecoveryManager::new();
    restarted.set_sidecar_path(&sidecar);
    let recovered = restarted.recover(None).expect("recovery should succeed");
    assert_eq!(recovered, 1);

    // Re-running recovery is idempotent: nothing left to recover.
    let recovered_again = restarted.recover(None).expect("recovery should succeed");
    assert_eq!(recovered_again, 0);
    assert!(!sidecar.exists(), "sidecar file should be cleared");
}

#[test]
fn test_startup_recovery_completes_durable_but_unfinalized_commit() {
    use std::sync::atomic::AtomicUsize;

    use crate::participant::TransactionCommitSink;
    use graphdb_core::types::CommitLsn;

    struct FailOnceFinalizeSink {
        finalize_calls: AtomicUsize,
    }

    impl TransactionCommitSink for FailOnceFinalizeSink {
        fn commit_transaction(&self, _transaction_id: TransactionId) -> Result<CommitLsn, String> {
            Ok(CommitLsn::new(21))
        }
        fn abort_transaction(&self, _transaction_id: TransactionId) -> Result<(), String> {
            Ok(())
        }
        fn finalize_commit(
            &self,
            _descriptor: &crate::participant::TransactionCommitDescriptor,
            _commit_lsn: CommitLsn,
        ) -> Result<(), String> {
            let calls = self
                .finalize_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if calls == 0 {
                return Err("injected crash between commit and finalize".to_string());
            }
            Ok(())
        }
        fn recover_unfinalized_commits(&self) -> Result<usize, String> {
            Ok(0)
        }
    }

    let config = TransactionManagerConfig {
        auto_cleanup: false,
        commit_retry_attempts: 0,
        ..Default::default()
    };
    let manager =
        TransactionManager::new(config).with_commit_sink(Arc::new(FailOnceFinalizeSink {
            finalize_calls: AtomicUsize::new(0),
        }));

    // Crash point: WAL durable, storage finalization failed.
    let txn_id = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("transaction should begin");
    let context = manager.get_context(txn_id).expect("context should exist");
    let frontier_before = manager.read_timestamp();
    manager
        .commit_transaction(txn_id)
        .expect_err("commit must fail at the crash point");

    // Restart: startup recovery re-drives finalization and completes the commit.
    let recovered = manager
        .startup_recovery()
        .expect("startup recovery should succeed");
    assert_eq!(recovered, 1);

    assert_eq!(context.state(), TransactionState::Committed);
    assert!(manager.get_context(txn_id).is_err());
    assert!(manager.read_timestamp() > frontier_before);

    // Second recovery run is idempotent.
    let recovered_again = manager
        .startup_recovery()
        .expect("second recovery should succeed");
    assert_eq!(recovered_again, 0);
}

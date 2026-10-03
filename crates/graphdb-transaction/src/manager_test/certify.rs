//! Transaction manager conflict certification delegation tests.

use std::sync::Arc;

use super::create_test_manager;
use crate::manager::TransactionManager;
use crate::types::*;
use crate::TransactionErrorKind;
#[test]
fn test_check_write_set_conflict_no_conflict() {
    use graphdb_core::types::VertexId;

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

    let vid1 = VertexId::try_from_int64(1).expect("test vertex id");
    let vid2 = VertexId::try_from_int64(2).expect("test vertex id");

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
    use graphdb_core::types::VertexId;

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

    let vid = VertexId::try_from_int64(1).expect("test vertex id");

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
    // First-committer-wins is enforced at commit time too: txn1's write
    // set is indexed by commit timestamp, which is newer than txn2's
    // start, so the late committer must lose instead of silently
    // overwriting txn1's write.
    let txn2_error = manager
        .commit_transaction(txn2)
        .expect_err("Late committer must lose the conflict");
    assert_eq!(txn2_error.kind(), TransactionErrorKind::SerializationFailed);
    // The failed commit already ran the canonical abort: txn2 left the
    // active table instead of lingering as a zombie.
    assert!(manager.get_context(txn2).is_err());
}

#[test]
fn test_concurrent_final_review_no_false_abort() {
    use std::sync::Barrier;
    use std::thread;

    use crate::participant::{
        TransactionAbortDescriptor, TransactionCommitDescriptor, TransactionCommitSink,
    };
    use graphdb_core::types::{CommitLsn, VertexId};

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

    // Two non-conflicting transactions commit concurrently under the global
    // certification lock. The sink barrier ensures both pass certification
    // before either enters the publication phase (final review).
    // The final review must NOT false-positive on non-conflicting writes.
    for iteration in 0..20 {
        let barrier = Arc::new(Barrier::new(2));
        let manager = Arc::new(
            TransactionManager::new(TransactionManagerConfig::default()).with_commit_sink(
                Arc::new(PassThroughSink {
                    barrier: Arc::clone(&barrier),
                }),
            ),
        );

        let vid1 = VertexId::try_from_int64(iteration as i64 * 2).expect("test vertex id");
        let vid2 = VertexId::try_from_int64(iteration as i64 * 2 + 1).expect("test vertex id");

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

        assert!(r1.is_ok(), "non-conflicting txn1 should commit: {:?}", r1);
        assert!(r2.is_ok(), "non-conflicting txn2 should commit: {:?}", r2);
    }
}

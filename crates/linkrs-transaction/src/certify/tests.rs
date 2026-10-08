//! Certifier tests: fast-path bypasses, conflict detection and publication.

use dashmap::DashMap;
use std::sync::Arc;

use super::Certifier;
use crate::context::TransactionContext;
use crate::types::{ConcurrencyMode, TransactionConfig, TransactionId, TransactionStats, WriteSet};

fn make_context(txn_id: u64, read_only: bool, mode: ConcurrencyMode) -> Arc<TransactionContext> {
    let config = TransactionConfig {
        concurrency_mode: mode,
        ..Default::default()
    };
    let ctx = if read_only {
        TransactionContext::new_readonly(TransactionId(txn_id), txn_id, config)
    } else {
        TransactionContext::new(TransactionId(txn_id), txn_id, config)
    };
    Arc::new(ctx)
}

#[test]
fn test_certification_fast_path_read_only() {
    let certifier = Certifier::new();
    let active: DashMap<TransactionId, Arc<TransactionContext>> = DashMap::new();
    let stats = TransactionStats::new();
    let ctx = make_context(1, true, ConcurrencyMode::Optimistic);
    active.insert(TransactionId(1), Arc::clone(&ctx));
    assert!(certifier
        .check_write_set_conflict(TransactionId(1), &active, &stats)
        .is_ok());
}

#[test]
fn test_certification_fast_path_single_writer() {
    let certifier = Certifier::new();
    let active: DashMap<TransactionId, Arc<TransactionContext>> = DashMap::new();
    let stats = TransactionStats::new();
    let ctx = make_context(2, false, ConcurrencyMode::SingleWriter);
    active.insert(TransactionId(2), Arc::clone(&ctx));
    assert!(certifier
        .check_write_set_conflict(TransactionId(2), &active, &stats)
        .is_ok());
    assert!(ctx.is_write_validated());
}

#[test]
fn test_certification_fast_path_empty_write_set() {
    let certifier = Certifier::new();
    let active: DashMap<TransactionId, Arc<TransactionContext>> = DashMap::new();
    let stats = TransactionStats::new();
    let ctx = make_context(3, false, ConcurrencyMode::Optimistic);
    active.insert(TransactionId(3), Arc::clone(&ctx));
    assert!(certifier
        .check_write_set_conflict(TransactionId(3), &active, &stats)
        .is_ok());
}

#[test]
fn test_publish_then_check_detects_committed_conflict_concurrent() {
    // Transaction IDs 10 and 2 commit concurrently; the global commit
    // lock must still serialize them so the second committer observes
    // the first. A commits after B started, so B must fail.
    let certifier = Certifier::new();
    let active: DashMap<TransactionId, Arc<TransactionContext>> = DashMap::new();
    let stats = TransactionStats::new();
    let ctx_a = make_context(10, false, ConcurrencyMode::Optimistic);
    let ctx_b = make_context(2, false, ConcurrencyMode::Optimistic);
    let vid = linkrs_core::types::VertexId::try_from_int64(7).expect("test vertex id");
    ctx_a.record_vertex_write(vid);
    ctx_b.record_vertex_write(vid);
    active.insert(TransactionId(10), Arc::clone(&ctx_a));
    active.insert(TransactionId(2), Arc::clone(&ctx_b));

    certifier
        .check_write_set_conflict(TransactionId(10), &active, &stats)
        .expect("first check should pass");
    certifier
        .publish(
            TransactionId(10),
            ctx_a.timestamp(),
            ctx_a.start_timestamp,
            &ctx_a.get_write_set(),
            &active,
            &stats,
        )
        .expect("publish should succeed");
    active.remove(&TransactionId(10));

    let result = certifier.check_write_set_conflict(TransactionId(2), &active, &stats);
    assert!(
        result.is_err(),
        "second committer must see the published write"
    );
}

#[test]
fn test_certification_detects_write_conflict() {
    let certifier = Certifier::new();
    let active: DashMap<TransactionId, Arc<TransactionContext>> = DashMap::new();
    let stats = TransactionStats::new();
    let ctx_a = make_context(10, false, ConcurrencyMode::Optimistic);
    let ctx_b = make_context(11, false, ConcurrencyMode::Optimistic);
    let vid = linkrs_core::types::VertexId::try_from_int64(42).expect("test vertex id");
    ctx_a.record_vertex_write(vid);
    ctx_b.record_vertex_write(vid);
    active.insert(TransactionId(10), Arc::clone(&ctx_a));
    active.insert(TransactionId(11), Arc::clone(&ctx_b));
    ctx_a.mark_write_validated();
    let result = certifier.check_write_set_conflict(TransactionId(11), &active, &stats);
    assert!(result.is_err());
}

#[test]
fn test_publish_indexed_by_commit_timestamp_catches_late_committer() {
    // Long-running writer A starts at 5 and commits at 15; B starts at
    // 10 on the same vertex. Indexing A by commit timestamp makes B's
    // check observe the conflict; indexing by start timestamp would
    // miss it (5 < 10) and lose the update.
    let certifier = Certifier::new();
    let active: DashMap<TransactionId, Arc<TransactionContext>> = DashMap::new();
    let stats = TransactionStats::new();
    // Test contexts start at their id (`make_context` uses the id as
    // the start timestamp): A starts at 5, B at 10.
    let ctx_a = make_context(5, false, ConcurrencyMode::Optimistic);
    let ctx_b = make_context(10, false, ConcurrencyMode::Optimistic);
    let vid = linkrs_core::types::VertexId::try_from_int64(99).expect("test vertex id");
    ctx_a.record_vertex_write(vid);
    ctx_b.record_vertex_write(vid);
    active.insert(TransactionId(5), Arc::clone(&ctx_a));
    active.insert(TransactionId(10), Arc::clone(&ctx_b));

    certifier
        .publish(
            TransactionId(5),
            15,
            ctx_a.start_timestamp,
            &ctx_a.get_write_set(),
            &active,
            &stats,
        )
        .expect("publish should succeed");
    active.remove(&TransactionId(5));

    let result = certifier.check_write_set_conflict(TransactionId(10), &active, &stats);
    assert!(
        result.is_err(),
        "committer indexed at 15 must conflict with start 10"
    );
}

#[test]
fn test_force_publish_is_idempotent() {
    let certifier = Certifier::new();
    let mut write_set = WriteSet::new();
    write_set
        .record_vertex(linkrs_core::types::VertexId::try_from_int64(7).expect("test vertex id"));

    certifier.force_publish(TransactionId(10), 15, &write_set);
    certifier.force_publish(TransactionId(10), 15, &write_set);

    let committed = certifier.committed_write_sets.lock();
    assert_eq!(committed.get(&15).map(|sets| sets.len()), Some(1));
}

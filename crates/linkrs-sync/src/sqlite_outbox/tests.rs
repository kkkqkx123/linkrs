use linkrs_core::types::{
    IdempotencyKey, IndexGeneration, OrderingKey, TargetId, TransactionId, VertexId,
};
use linkrs_core::wal::{
    EntityRef, IndexMutation, IndexOperation, OutboxIntent, WAL_SYNC_WIRE_VERSION,
};
use tempfile::TempDir;

use super::SqliteOutbox;
use linkrs_core::types::CommitLsn;

fn intent(sequence: u32, entity: i64, target: &TargetId) -> OutboxIntent {
    OutboxIntent {
        wire_version: WAL_SYNC_WIRE_VERSION,
        transaction_id: TransactionId::new(1),
        intent_sequence: sequence,
        mutation: IndexMutation {
            wire_version: WAL_SYNC_WIRE_VERSION,
            target: target.clone(),
            index_id: 1,
            index_generation: IndexGeneration::new(1),
            entity_ref: EntityRef::Vertex(
                VertexId::try_from_int64(entity).expect("test vertex id"),
            ),
            operation: IndexOperation::Upsert,
            document_or_vector: vec![entity as u8],
            idempotency_key: IdempotencyKey::new(format!("event-{sequence}"))
                .expect("key should be valid"),
            ordering_key: OrderingKey::new(format!("entity-{entity}"))
                .expect("key should be valid"),
        },
    }
}

#[tokio::test]
async fn materialize_claim_and_ack_are_durable() {
    let directory = TempDir::new().expect("temporary directory should be created");
    let path = directory.path().join("outbox.sqlite");
    let target = TargetId::new("fulltext").expect("target should be valid");
    let outbox = SqliteOutbox::open(&path).await.expect("outbox should open");
    outbox
        .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should be created");
    outbox
        .transition_generation_to_backfilling(&target, 1, 1)
        .await
        .expect("generation should transition to backfilling");
    outbox
        .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should transition to catching up");
    outbox
        .activate_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should activate");
    outbox
        .materialize_commit(
            CommitLsn::new(100),
            &[intent(0, 7, &target)],
            std::slice::from_ref(&target),
        )
        .await
        .expect("commit should materialize");
    let event = outbox
        .claim_next(&target, "worker-1", 10, 100)
        .await
        .expect("claim should succeed")
        .expect("event should be available");
    assert!(outbox
        .acknowledge(&event)
        .await
        .expect("ack should succeed"));
    assert_eq!(
        outbox
            .target_frontier(&target)
            .await
            .expect("frontier should load"),
        CommitLsn::new(100)
    );
    drop(outbox);
    let reopened = SqliteOutbox::open(&path)
        .await
        .expect("outbox should reopen");
    assert_eq!(
        reopened
            .materialized_lsn()
            .await
            .expect("materialized LSN should load"),
        CommitLsn::new(100)
    );
}

#[tokio::test]
async fn materialization_registers_a_missing_first_generation() {
    let directory = TempDir::new().expect("temporary directory should be created");
    let target = TargetId::new("fulltext").expect("target should be valid");
    let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
        .await
        .expect("outbox should open");
    outbox
        .materialize_commit(
            CommitLsn::new(10),
            &[intent(0, 1, &target)],
            std::slice::from_ref(&target),
        )
        .await
        .expect("commit should materialize");

    let state = outbox
        .get_generation_state(&target, 1, 1)
        .await
        .expect("generation state should load")
        .expect("generation should be registered");
    assert_eq!(state.0, "active");
    assert!(outbox
        .claim_next(&target, "worker-1", 0, 100)
        .await
        .expect("event should be claimable")
        .is_some());
}

#[tokio::test]
async fn frontier_does_not_cross_an_unacknowledged_commit() {
    let directory = TempDir::new().expect("temporary directory should be created");
    let target = TargetId::new("vector").expect("target should be valid");
    let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
        .await
        .expect("outbox should open");
    outbox
        .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should be created");
    outbox
        .transition_generation_to_backfilling(&target, 1, 1)
        .await
        .expect("generation should transition to backfilling");
    outbox
        .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should transition to catching up");
    outbox
        .activate_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should activate");
    outbox
        .materialize_commit(
            CommitLsn::new(100),
            &[intent(0, 1, &target)],
            std::slice::from_ref(&target),
        )
        .await
        .expect("first commit should materialize");
    let mut second = intent(0, 2, &target);
    second.transaction_id = TransactionId::new(2);
    second.mutation.idempotency_key = IdempotencyKey::new("second").expect("key should be valid");
    outbox
        .materialize_commit(
            CommitLsn::new(200),
            &[second],
            std::slice::from_ref(&target),
        )
        .await
        .expect("second commit should materialize");

    let first = outbox
        .claim_next(&target, "worker-1", 10, 100)
        .await
        .expect("first claim should succeed")
        .expect("first event should exist");
    let second = outbox
        .claim_next(&target, "worker-2", 10, 100)
        .await
        .expect("second claim should succeed")
        .expect("second event should exist");
    assert_eq!(second.commit_lsn, CommitLsn::new(200));
    assert!(outbox
        .acknowledge(&second)
        .await
        .expect("second ack should succeed"));
    assert_eq!(
        outbox
            .target_frontier(&target)
            .await
            .expect("frontier should load"),
        CommitLsn::ZERO
    );
    assert!(outbox
        .acknowledge(&first)
        .await
        .expect("first ack should succeed"));
    assert_eq!(
        outbox
            .target_frontier(&target)
            .await
            .expect("frontier should load"),
        CommitLsn::new(200)
    );
}

#[tokio::test]
async fn stale_lease_cannot_ack() {
    let directory = TempDir::new().expect("temporary directory should be created");
    let target = TargetId::new("fulltext").expect("target should be valid");
    let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
        .await
        .expect("outbox should open");
    outbox
        .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should be created");
    outbox
        .transition_generation_to_backfilling(&target, 1, 1)
        .await
        .expect("generation should transition to backfilling");
    outbox
        .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should transition to catching up");
    outbox
        .activate_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should activate");
    outbox
        .materialize_commit(
            CommitLsn::new(10),
            &[intent(0, 1, &target)],
            std::slice::from_ref(&target),
        )
        .await
        .expect("commit should materialize");
    let stale = outbox
        .claim_next(&target, "worker-1", 0, 10)
        .await
        .expect("claim should succeed")
        .expect("event should exist");
    let current = outbox
        .claim_next(&target, "worker-2", 11, 10)
        .await
        .expect("reclaim should succeed")
        .expect("event should be reclaimed");
    assert!(!outbox
        .acknowledge(&stale)
        .await
        .expect("stale ack should be rejected cleanly"));
    assert!(outbox
        .acknowledge(&current)
        .await
        .expect("current ack should succeed"));
}

#[tokio::test]
async fn snapshot_is_atomic_and_checksum_verified() {
    let directory = TempDir::new().expect("temporary directory");
    let path = directory.path().join("outbox.sqlite");
    let snapshot_path = directory.path().join("checkpoints/outbox.sqlite");
    let outbox = SqliteOutbox::open(&path).await.expect("outbox should open");
    let snapshot = outbox
        .create_snapshot(&snapshot_path)
        .await
        .expect("snapshot should be created");
    SqliteOutbox::verify_snapshot(&snapshot).expect("snapshot checksum should verify");
    let mut bytes = std::fs::read(&snapshot_path).expect("snapshot should be readable");
    bytes[0] ^= 0xff;
    std::fs::write(&snapshot_path, bytes).expect("snapshot should be corrupted");
    assert!(SqliteOutbox::verify_snapshot(&snapshot).is_err());
}

#[tokio::test]
async fn diagnostics_report_backlog_and_generation_frontiers() {
    let directory = TempDir::new().expect("temporary directory should be created");
    let target = TargetId::new("fulltext").expect("target should be valid");
    let outbox = SqliteOutbox::open(directory.path().join("outbox.sqlite"))
        .await
        .expect("outbox should open");
    outbox
        .create_index_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should be created");
    outbox
        .transition_generation_to_backfilling(&target, 1, 1)
        .await
        .expect("generation should transition to backfilling");
    outbox
        .transition_generation_to_catching_up(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should transition to catching up");
    outbox
        .activate_generation(&target, 1, 1, CommitLsn::ZERO)
        .await
        .expect("generation should activate");
    outbox
        .materialize_commit(
            CommitLsn::new(42),
            &[intent(0, 1, &target)],
            std::slice::from_ref(&target),
        )
        .await
        .expect("commit should materialize");

    let diagnostics = outbox.diagnostics().await.expect("diagnostics should load");
    assert_eq!(diagnostics.materialized_lsn, CommitLsn::new(42));
    assert_eq!(diagnostics.targets.len(), 1);
    assert_eq!(diagnostics.targets[0].target, "fulltext");
    assert_eq!(diagnostics.targets[0].pending, 1);
    assert_eq!(diagnostics.targets[0].frontier_lag, 42);
    assert!(diagnostics.targets[0].oldest_event_age_ms.is_some());
    assert_eq!(diagnostics.indexes.len(), 1);
    assert_eq!(diagnostics.indexes[0].state, "active");
    assert_eq!(diagnostics.indexes[0].frontier_lag, 42);

    let stats = outbox.stats().await.expect("outbox stats should load");
    assert_eq!(stats.pending, 1);
    assert_eq!(stats.leased, 0);
    assert_eq!(stats.dead_lettered, 0);
    assert_eq!(stats.retries, 0);

    let event = outbox
        .claim_next(&target, "stats-worker", 0, 1_000)
        .await
        .expect("event should be claimable")
        .expect("event should exist");
    let leased = outbox.stats().await.expect("leased stats should load");
    assert_eq!(leased.pending, 0);
    assert_eq!(leased.leased, 1);
    outbox
        .retry(&event, 1_001, "temporary failure")
        .await
        .expect("event should be retryable");
    let retrying = outbox.stats().await.expect("retry stats should load");
    assert_eq!(retrying.pending, 1);
    assert_eq!(retrying.leased, 0);
    assert_eq!(retrying.retries, 1);
}

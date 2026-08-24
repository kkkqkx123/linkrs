//! Regression tests for vector intent delivery through the durable outbox.
//!
//! Guards the contract between intent staging (`delivery_target_names`) and
//! outbox consumption (`apply_index_mutation`): whenever a `"vector"` target
//! is staged, the consumer must have a matching delivery branch, regardless
//! of which optional features are compiled in.

#![cfg(feature = "vector")]

use std::sync::Arc;

use graphdb_sync::core::types::{CommitLsn, TransactionId};
use graphdb_sync::core::{Value, VectorValue};
use graphdb_sync::sync::types::ChangeType;
use graphdb_sync::sync::{SyncManager, VectorBackend, VectorSyncCoordinator};
use vector_search::{DistanceMetric, LocalVectorEngine};

fn make_manager(engine: Arc<LocalVectorEngine>) -> SyncManager {
    let backend = VectorBackend::Local(engine);
    let coordinator = Arc::new(VectorSyncCoordinator::new_without_embedding(
        backend,
        tokio::runtime::Handle::current(),
    ));
    SyncManager::new_without_fulltext().with_vector_coordinator(coordinator)
}

fn vector_property(value: Vec<f32>) -> Vec<(String, Value)> {
    vec![(
        "embedding".to_string(),
        Value::Vector(VectorValue::dense(value)),
    )]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staged_vector_intent_is_delivered_to_local_backend() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let engine = Arc::new(
        LocalVectorEngine::open(directory.path().join("vec"))
            .expect("local vector engine should open"),
    );
    let mut manager = make_manager(engine.clone());
    manager
        .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
        .expect("outbox should configure");

    // Register the logical index so staged vertex changes carry a vector payload.
    let coordinator = manager.vector_coordinator().expect("coordinator attached");
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");

    let txn = TransactionId::from(1u64);
    manager
        .on_vertex_change_with_txn(
            txn,
            1,
            "user",
            &Value::string("v1"),
            &vector_property(vec![1.0, 0.0, 0.0, 0.0]),
            ChangeType::Insert,
        )
        .expect("vertex change should stage");

    let intents = manager
        .pending_transaction_intents(txn)
        .expect("pending intents should resolve");
    assert!(
        intents
            .iter()
            .any(|intent| intent.mutation.target.as_str() == "vector"),
        "a vector target intent must be staged"
    );

    manager
        .materialize_committed_transaction(txn, CommitLsn::new(10), &intents)
        .expect("committed transaction should materialize");
    manager.clear_transaction_intents(txn);

    let delivered = manager
        .retry_outbox_sync()
        .expect("outbox delivery should run");
    assert_eq!(delivered, 1, "the staged vector intent must be delivered");

    assert_eq!(
        engine.count("space_1").expect("collection count"),
        1,
        "the vector point must land in the local engine instead of being retried into the dead-letter queue"
    );
    assert!(
        manager.get_dead_letter_entries().is_empty(),
        "no entry may reach the dead-letter queue"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staged_vector_schema_intents_are_consumed_without_dead_letters() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let engine = Arc::new(
        LocalVectorEngine::open(directory.path().join("vec"))
            .expect("local vector engine should open"),
    );
    let mut manager = make_manager(engine.clone());
    manager
        .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
        .expect("outbox should configure");

    let txn = TransactionId::from(2u64);
    manager
        .on_vertex_change_with_txn(
            txn,
            1,
            "user",
            &Value::string("v1"),
            &[],
            ChangeType::Delete,
        )
        .expect("delete change should stage");

    let intents = manager
        .pending_transaction_intents(txn)
        .expect("pending intents should resolve");
    manager
        .materialize_committed_transaction(txn, CommitLsn::new(20), &intents)
        .expect("committed transaction should materialize");
    manager.clear_transaction_intents(txn);

    manager
        .retry_outbox_sync()
        .expect("outbox delivery should run");

    assert!(
        !engine.collection_exists("space_1"),
        "a delete without registered indexes must not create collections"
    );
    assert!(
        manager.get_dead_letter_entries().is_empty(),
        "schema-only deliveries must not dead-letter"
    );
}

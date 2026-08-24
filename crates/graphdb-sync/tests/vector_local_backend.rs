//! Integration tests for the local vector backend delivery path.
//!
//! Exercises `VectorSyncCoordinator` against a `VectorBackend::Local` through
//! the production entry points: logical index creation, batched change
//! delivery (`on_vector_change_batch`), search and crash recovery.

#![cfg(feature = "vector")]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use graphdb_sync::sync::vector_sync::SearchOptions;
use graphdb_sync::sync::{
    VectorBackend, VectorChangeContext, VectorChangeType, VectorPointData, VectorSyncCoordinator,
};

use vector_search::{DistanceMetric, LocalVectorEngine, SearchQuery, TxnOp, VectorPoint};

fn make_engine(root: &Path) -> Arc<LocalVectorEngine> {
    Arc::new(LocalVectorEngine::open(root).unwrap())
}

fn make_coordinator(engine: Arc<LocalVectorEngine>) -> Arc<VectorSyncCoordinator> {
    let backend = VectorBackend::Local(engine);
    Arc::new(VectorSyncCoordinator::new_without_embedding(
        backend,
        tokio::runtime::Handle::current(),
    ))
}

fn change_ctx(
    space_id: u64,
    tag_name: &str,
    field_name: &str,
    change_type: VectorChangeType,
    id: &str,
    vector: Vec<f32>,
) -> VectorChangeContext {
    let data = VectorPointData {
        id: id.to_string(),
        vector,
        payload: HashMap::new(),
    };
    VectorChangeContext::new(space_id, tag_name, field_name, change_type, data)
}

#[tokio::test]
async fn test_local_index_create_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    let collection = coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .unwrap();
    assert_eq!(collection, "space_1");
    assert!(engine.collection_exists("space_1"));
    assert!(coordinator.index_exists(1, "user", "embedding"));
    assert_eq!(
        coordinator.engine_state(),
        graphdb_sync::sync::VectorEngineState::Active
    );

    // Duplicate index creation is tolerated (config conflict check passes).
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_batch_delivery_applies_upserts_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .unwrap();

    coordinator
        .on_vector_change_batch(vec![
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v1_user_embedding",
                vec![1.0, 0.0, 0.0, 0.0],
            ),
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v2_user_embedding",
                vec![0.0, 1.0, 0.0, 0.0],
            ),
        ])
        .await
        .unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 2);
    let got = engine.get("space_1", "v1_user_embedding").unwrap().unwrap();
    assert_eq!(got.vector, vec![1.0, 0.0, 0.0, 0.0]);

    // Search scoped by group_id returns the nearest point first.
    let results = coordinator
        .search_with_options(SearchOptions::new(
            1,
            "user",
            "embedding",
            vec![1.0, 0.0, 0.0, 0.0],
            2,
        ))
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].id.to_string(), "v1_user_embedding");

    // Delete propagation through a second delivery.
    coordinator
        .on_vector_change_batch(vec![change_ctx(
            1,
            "user",
            "embedding",
            VectorChangeType::Delete,
            "v1_user_embedding",
            Vec::new(),
        )])
        .await
        .unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 1);
    assert!(engine
        .get("space_1", "v1_user_embedding")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_engine_replay_is_idempotent_per_txn_id() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());

    engine
        .create_collection(
            "space_1",
            &vector_search::CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .unwrap();

    let txn_ops = vec![TxnOp::Upsert {
        collection: "space_1".to_string(),
        point: VectorPoint::new("v1_user_embedding", vec![0.5, 0.5, 0.0, 0.0]),
    }];
    engine.apply_txn(7u64, txn_ops.clone()).unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 1);

    // Replaying the same transaction id is a no-op for the same point.
    engine.apply_txn(7u64, txn_ops).unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 1);
}

#[tokio::test]
async fn test_batch_delivery_injects_group_id_payload() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(2, "item", "vec", 3, DistanceMetric::Euclid)
        .await
        .unwrap();

    coordinator
        .on_vector_change_batch(vec![change_ctx(
            2,
            "item",
            "vec",
            VectorChangeType::Insert,
            "10_item_vec",
            vec![1.0, 0.0, 0.0],
        )])
        .await
        .unwrap();

    assert_eq!(engine.count("space_2").unwrap(), 1);
    let got = engine.get("space_2", "10_item_vec").unwrap().unwrap();
    assert_eq!(got.vector, vec![1.0, 0.0, 0.0]);
    assert_eq!(
        got.payload
            .as_ref()
            .and_then(|p| p.get("group_id"))
            .and_then(|v| v.as_str()),
        Some("item_vec")
    );

    // A delivered insert is searchable through the group-scoped query path.
    let results = coordinator
        .search_with_options(SearchOptions::new(2, "item", "vec", vec![1.0, 0.0, 0.0], 5))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_crash_recovery_replays_delivered_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vec");

    let (engine, _coordinator) = {
        let engine = make_engine(root.as_path());
        let coordinator = make_coordinator(engine.clone());
        coordinator
            .create_vector_index(6, "item", "vec", 4, DistanceMetric::Cosine)
            .await
            .unwrap();
        let inserts: Vec<VectorChangeContext> = [
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ]
        .iter()
        .enumerate()
        .map(|(i, v)| {
            change_ctx(
                6,
                "item",
                "vec",
                VectorChangeType::Insert,
                &format!("v{}_item_vec", i + 1),
                v.clone(),
            )
        })
        .collect();
        coordinator.on_vector_change_batch(inserts).await.unwrap();
        assert_eq!(engine.count("space_6").unwrap(), 3);
        (engine, coordinator)
    };

    // Simulate a crash: drop the coordinator and engine without deleting the
    // directory, then reopen.
    drop(_coordinator);
    drop(engine);

    let recovered = make_engine(root.as_path());
    assert_eq!(recovered.count("space_6").unwrap(), 3);
    let got = recovered
        .get("space_6", "v2_item_vec")
        .unwrap()
        .expect("recovered point");
    assert_eq!(got.vector, vec![0.0, 1.0, 0.0, 0.0]);
    let results = recovered
        .search("space_6", &SearchQuery::new(vec![1.0, 0.0, 0.0, 0.0], 1))
        .unwrap();
    assert_eq!(results[0].id.to_string(), "v1_item_vec");
}

#[tokio::test]
async fn test_delete_then_compaction_keeps_search_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vec");
    let engine = make_engine(root.as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(8, "user", "embedding", 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    // Insert five points through one delivered batch.
    let vectors: Vec<Vec<f32>> = vec![
        vec![1.0, 0.0],
        vec![0.9, 0.1],
        vec![0.5, 0.5],
        vec![0.1, 0.9],
        vec![0.0, 1.0],
    ];
    let inserts = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| {
            change_ctx(
                8,
                "user",
                "embedding",
                VectorChangeType::Insert,
                &format!("v{}_user_embedding", i + 1),
                v.clone(),
            )
        })
        .collect();
    coordinator.on_vector_change_batch(inserts).await.unwrap();
    assert_eq!(engine.count("space_8").unwrap(), 5);

    // Delete two of five (40% > 20% threshold): auto-compaction physically
    // removes the tombstones after the delivery.
    let deletes = ["v1_user_embedding", "v3_user_embedding"]
        .iter()
        .map(|id| {
            change_ctx(
                8,
                "user",
                "embedding",
                VectorChangeType::Delete,
                id,
                Vec::new(),
            )
        })
        .collect();
    coordinator.on_vector_change_batch(deletes).await.unwrap();

    assert_eq!(engine.count("space_8").unwrap(), 3);
    assert!(engine
        .get("space_8", "v1_user_embedding")
        .unwrap()
        .is_none());
    assert!(engine
        .get("space_8", "v3_user_embedding")
        .unwrap()
        .is_none());

    // Search after compaction only surfaces surviving points.
    let results = coordinator
        .search_with_options(SearchOptions::new(
            8,
            "user",
            "embedding",
            vec![1.0, 0.0],
            5,
        ))
        .await
        .unwrap();
    assert_eq!(results.len(), 3);
    let ids: std::collections::HashSet<String> = results.iter().map(|r| r.id.to_string()).collect();
    assert!(!ids.contains("v1_user_embedding"));
    assert!(!ids.contains("v3_user_embedding"));
    assert_eq!(results[0].id.to_string(), "v2_user_embedding");

    // The compacted state survives a close/reopen cycle.
    drop(coordinator);
    drop(engine);
    let recovered = make_engine(root.as_path());
    assert_eq!(recovered.count("space_8").unwrap(), 3);
    assert!(recovered
        .get("space_8", "v2_user_embedding")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn test_drop_reclaims_group_points_and_physical_collection() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    // Two logical indexes share one space-level physical collection.
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .unwrap();
    coordinator
        .create_vector_index(1, "user", "avatar", 4, DistanceMetric::Cosine)
        .await
        .unwrap();
    assert!(engine.collection_exists("space_1"));

    coordinator
        .on_vector_change_batch(vec![
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v1",
                vec![1.0, 0.0, 0.0, 0.0],
            ),
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v2",
                vec![0.0, 1.0, 0.0, 0.0],
            ),
            change_ctx(
                1,
                "user",
                "avatar",
                VectorChangeType::Insert,
                "a1",
                vec![0.0, 0.0, 1.0, 0.0],
            ),
            change_ctx(
                1,
                "user",
                "avatar",
                VectorChangeType::Insert,
                "a2",
                vec![0.0, 0.0, 0.0, 1.0],
            ),
        ])
        .await
        .unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 4);

    // Dropping the first index purges its group but keeps the collection
    // alive for the sibling index.
    coordinator
        .drop_vector_index(1, "user", "embedding")
        .await
        .unwrap();
    assert!(engine.collection_exists("space_1"));
    assert!(!coordinator.index_exists(1, "user", "embedding"));
    assert!(coordinator.index_exists(1, "user", "avatar"));
    let embedding_hits = coordinator
        .search_by_location(1, "user", "embedding", vec![1.0, 0.0, 0.0, 0.0], 10)
        .await
        .unwrap();
    assert!(embedding_hits.is_empty());
    assert_eq!(
        coordinator
            .search_by_location(1, "user", "avatar", vec![0.0, 0.0, 1.0, 0.0], 10)
            .await
            .unwrap()
            .len(),
        2
    );

    // Dropping the last index reclaims the physical directory.
    coordinator
        .drop_vector_index(1, "user", "avatar")
        .await
        .unwrap();
    assert!(!engine.collection_exists("space_1"));
    assert!(!dir.path().join("vec").join("space_1").exists());

    // A same-named index can be recreated cleanly afterwards.
    coordinator
        .create_vector_index(1, "user", "avatar", 4, DistanceMetric::Cosine)
        .await
        .unwrap();
    assert!(engine.collection_exists("space_1"));
}

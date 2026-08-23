//! Integration tests for the local vector backend commit path.
//!
//! Exercises `VectorSyncCoordinator` against a `VectorBackend::Local`:
//! index creation, buffered change commits (WAL-backed `apply_txn`), search,
//! delete propagation and commit idempotency.

#![cfg(feature = "vector")]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use graphdb_sync::core::types::{TransactionId, VertexId};
use graphdb_sync::core::{Tag, Value, VectorValue, Vertex};
use graphdb_sync::sync::vector_sync::SearchOptions;
use graphdb_sync::sync::{
    VectorBackend, VectorChangeContext, VectorChangeType, VectorPointData, VectorSyncCoordinator,
    VectorTransactionBufferConfig,
};

use vector_search::{
    DistanceMetric, FilterCondition, LocalVectorEngine, SearchQuery, TxnOp, VectorFilter,
    VectorPoint,
};

fn make_engine(root: &Path) -> Arc<LocalVectorEngine> {
    Arc::new(LocalVectorEngine::open(root).unwrap())
}

fn make_coordinator(engine: Arc<LocalVectorEngine>) -> Arc<VectorSyncCoordinator> {
    let backend = VectorBackend::Local(engine);
    Arc::new(VectorSyncCoordinator::with_transaction_buffer(
        backend,
        #[cfg(feature = "vector-qdrant")]
        None,
        VectorTransactionBufferConfig::default(),
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

fn vertex(vid: i64, tag_name: &str, field_name: &str, vector: Vec<f32>) -> Vertex {
    Vertex {
        vid: VertexId::from_int64(vid),
        id: 0,
        tags: vec![Tag {
            name: tag_name.to_string(),
            properties: HashMap::from([(
                field_name.to_string(),
                Value::Vector(VectorValue::dense(vector)),
            )]),
        }],
        properties: HashMap::new(),
    }
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
async fn test_commit_propagates_upsert_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .unwrap();

    let txn: TransactionId = 1u64.into();
    coordinator
        .buffer_vector_change(
            txn,
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v1_user_embedding",
                vec![1.0, 0.0, 0.0, 0.0],
            ),
        )
        .unwrap();
    coordinator
        .buffer_vector_change(
            txn,
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v2_user_embedding",
                vec![0.0, 1.0, 0.0, 0.0],
            ),
        )
        .unwrap();

    coordinator.commit_transaction(txn).await.unwrap();

    // Buffered updates were drained and both points landed in the collection.
    assert!(!coordinator
        .transaction_buffer()
        .unwrap()
        .has_pending_updates(txn));
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

    // Delete propagation through a second commit.
    let txn2: TransactionId = 2u64.into();
    coordinator
        .buffer_vector_change(
            txn2,
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Delete,
                "v1_user_embedding",
                Vec::new(),
            ),
        )
        .unwrap();
    coordinator.commit_transaction(txn2).await.unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 1);
    assert!(engine
        .get("space_1", "v1_user_embedding")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_commit_is_idempotent_per_txn_id() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .unwrap();

    let txn: TransactionId = 7u64.into();
    coordinator
        .buffer_vector_change(
            txn,
            change_ctx(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v1_user_embedding",
                vec![0.5, 0.5, 0.0, 0.0],
            ),
        )
        .unwrap();
    coordinator.commit_transaction(txn).await.unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 1);

    // The engine replay of the same txn id is a no-op for the same point.
    engine
        .apply_txn(
            txn.into(),
            vec![TxnOp::Upsert {
                collection: "space_1".to_string(),
                point: VectorPoint::new("v1_user_embedding", vec![0.5, 0.5, 0.0, 0.0]),
            }],
        )
        .unwrap();
    assert_eq!(engine.count("space_1").unwrap(), 1);
}

#[tokio::test]
async fn test_direct_vertex_upsert_path() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(2, "item", "vec", 3, DistanceMetric::Euclid)
        .await
        .unwrap();

    let vertex = vertex(10, "item", "vec", vec![1.0, 0.0, 0.0]);
    coordinator.on_vertex_inserted(2, &vertex).await.unwrap();
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

    // Filter search by payload field.
    let filter = VectorFilter::new().must(FilterCondition::match_value("group_id", "item_vec"));
    let query = SearchQuery::new(vec![1.0, 0.0, 0.0], 1).with_filter(filter);
    let results = engine.search("space_2", &query).unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_string_vid_point_id_and_vertex_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(4, "item", "vec", 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let vertex = Vertex {
        vid: VertexId::from_string("v10"),
        id: 0,
        tags: vec![Tag {
            name: "item".to_string(),
            properties: HashMap::from([(
                "vec".to_string(),
                Value::Vector(VectorValue::dense(vec![1.0, 0.0])),
            )]),
        }],
        properties: HashMap::new(),
    };
    coordinator.on_vertex_inserted(4, &vertex).await.unwrap();
    assert_eq!(engine.count("space_4").unwrap(), 1);

    // String vertex ids produce unquoted point ids.
    assert!(engine.get("space_4", "v10_item_vec").unwrap().is_some());

    // on_vertex_deleted matches the plain vertex_id payload from
    // on_vertex_inserted.
    coordinator
        .on_vertex_deleted(4, "item", &Value::string("v10"))
        .await
        .unwrap();
    assert_eq!(engine.count("space_4").unwrap(), 0);
}

#[tokio::test]
async fn test_commit_failure_preserves_buffer_and_retry_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(5, "user", "embedding", 2, DistanceMetric::Cosine)
        .await
        .unwrap();

    let txn: TransactionId = 100u64.into();
    // Valid update (sequence 1).
    coordinator
        .buffer_vector_change_with_sequence(
            txn,
            1,
            change_ctx(
                5,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v1_user_embedding",
                vec![1.0, 0.0],
            ),
        )
        .unwrap();
    // Invalid-dimension update (sequence 2) makes the whole txn fail.
    coordinator
        .buffer_vector_change_with_sequence(
            txn,
            2,
            change_ctx(
                5,
                "user",
                "embedding",
                VectorChangeType::Insert,
                "v2_user_embedding",
                vec![1.0, 0.0, 0.0],
            ),
        )
        .unwrap();

    let err = coordinator.commit_transaction(txn).await.unwrap_err();
    assert!(!err.to_string().is_empty());

    // The buffer is preserved and nothing was applied.
    assert!(coordinator
        .transaction_buffer()
        .unwrap()
        .has_pending_updates(txn));
    assert_eq!(engine.count("space_5").unwrap(), 0);

    // Fix the bad update (drop sequence > 1) and retry -> succeeds.
    coordinator.truncate_transaction(txn, 1).unwrap();
    coordinator.commit_transaction(txn).await.unwrap();
    assert_eq!(engine.count("space_5").unwrap(), 1);
    assert!(engine
        .get("space_5", "v1_user_embedding")
        .unwrap()
        .is_some());
    assert!(!coordinator
        .transaction_buffer()
        .unwrap()
        .has_pending_updates(txn));
}

#[tokio::test]
async fn test_crash_recovery_replays_committed_wal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vec");

    let (engine, coordinator) = {
        let engine = make_engine(root.as_path());
        let coordinator = make_coordinator(engine.clone());
        coordinator
            .create_vector_index(6, "item", "vec", 4, DistanceMetric::Cosine)
            .await
            .unwrap();
        let txn: TransactionId = 200u64.into();
        for (i, v) in [
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ]
        .iter()
        .enumerate()
        {
            let point_id = format!("v{}_item_vec", i + 1);
            coordinator
                .buffer_vector_change(
                    txn,
                    change_ctx(
                        6,
                        "item",
                        "vec",
                        VectorChangeType::Insert,
                        &point_id,
                        v.clone(),
                    ),
                )
                .unwrap();
        }
        coordinator.commit_transaction(txn).await.unwrap();
        assert_eq!(engine.count("space_6").unwrap(), 3);
        (engine, coordinator)
    };

    // Simulate a crash: drop the coordinator and engine without deleting the
    // directory, then reopen.
    drop(coordinator);
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

    // Insert five points through committed transactions.
    let txn: TransactionId = 300u64.into();
    for i in 1..=5 {
        let v = match i {
            1 => vec![1.0, 0.0],
            2 => vec![0.9, 0.1],
            3 => vec![0.5, 0.5],
            4 => vec![0.1, 0.9],
            _ => vec![0.0, 1.0],
        };
        coordinator
            .buffer_vector_change(
                txn,
                change_ctx(
                    8,
                    "user",
                    "embedding",
                    VectorChangeType::Insert,
                    &format!("v{}_user_embedding", i),
                    v,
                ),
            )
            .unwrap();
    }
    coordinator.commit_transaction(txn).await.unwrap();
    assert_eq!(engine.count("space_8").unwrap(), 5);

    // Delete two of five (40% > 20% threshold): auto-compaction physically
    // removes the tombstones during commit.
    let txn2: TransactionId = 301u64.into();
    for id in ["v1_user_embedding", "v3_user_embedding"] {
        coordinator
            .buffer_vector_change(
                txn2,
                change_ctx(
                    8,
                    "user",
                    "embedding",
                    VectorChangeType::Delete,
                    id,
                    Vec::new(),
                ),
            )
            .unwrap();
    }
    coordinator.commit_transaction(txn2).await.unwrap();

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
async fn test_delete_by_filter_through_coordinator() {
    let dir = tempfile::tempdir().unwrap();
    let engine = make_engine(dir.path().join("vec").as_path());
    let coordinator = make_coordinator(engine.clone());

    coordinator
        .create_vector_index(3, "user", "vec", 2, DistanceMetric::Dot)
        .await
        .unwrap();

    // Insert points with a plain JSON string `vertex_id` payload so the
    // coordinator's `on_vertex_deleted` filter (`vertex_id == format!(... )`)
    // can match them.
    engine
        .upsert(
            "space_3",
            VectorPoint::new("a_user_vec", vec![1.0, 0.0])
                .with_payload(HashMap::from([("vertex_id".to_string(), "a".into())])),
        )
        .unwrap();
    engine
        .upsert(
            "space_3",
            VectorPoint::new("b_user_vec", vec![0.0, 1.0])
                .with_payload(HashMap::from([("vertex_id".to_string(), "b".into())])),
        )
        .unwrap();
    assert_eq!(engine.count("space_3").unwrap(), 2);

    coordinator
        .on_vertex_deleted(3, "user", &Value::string("a"))
        .await
        .unwrap();
    assert_eq!(engine.count("space_3").unwrap(), 1);
    assert!(engine.get("space_3", "a_user_vec").unwrap().is_none());
    assert!(engine.get("space_3", "b_user_vec").unwrap().is_some());
}

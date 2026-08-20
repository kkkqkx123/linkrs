//! Local vector engine startup end-to-end test.
//!
//! Boots a full GraphService with the built-in local vector engine
//! (configured through `Config.vector`) and exercises the VectorApi
//! round-trip: create index -> insert batch -> search -> get -> count ->
//! drop index.

#![cfg(feature = "vector")]

use std::collections::HashMap;
use std::sync::Arc;

use graphdb::config::{Config, VectorEngineKind};
use graphdb::storage::{GraphStorage, PropertyGraphConfig};
use graphdb::sync::vector_sync::{DistanceMetric, PointId, SearchOptions, VectorPoint};
use graphdb_server::server::GraphService;

fn point(id: i64, vector: Vec<f32>, group_id: &str) -> VectorPoint {
    let mut payload = HashMap::new();
    payload.insert(
        "group_id".to_string(),
        serde_json::Value::String(group_id.to_string()),
    );
    VectorPoint {
        id: PointId::Num(id as u64),
        vector,
        payload: Some(payload),
    }
}

#[tokio::test]
async fn local_vector_engine_startup_e2e() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let mut config = Config::default();
    config.vector.enabled = true;
    config.vector.engine = VectorEngineKind::Local;
    config.vector.local.data_dir = Some(temp_dir.path().join("vector"));

    let storage = Arc::new(
        GraphStorage::new_with_config(PropertyGraphConfig::test())
            .expect("Failed to create storage"),
    );

    let graph_service = GraphService::new_for_test(config, storage).await;
    let vector_api = graph_service
        .vector_api()
        .expect("Vector API should be available with local engine enabled");

    let collection = vector_api
        .create_index(1, "item", "vec", 3, DistanceMetric::Cosine)
        .await
        .expect("create_index should succeed");
    assert_eq!(collection, "space_1");

    vector_api
        .insert_vector_batch(
            1,
            "item",
            "vec",
            vec![
                point(1, vec![1.0, 0.0, 0.0], "item_vec"),
                point(2, vec![0.0, 1.0, 0.0], "item_vec"),
                point(3, vec![0.0, 0.0, 1.0], "item_vec"),
            ],
        )
        .await
        .expect("insert_vector_batch should succeed");

    let results = vector_api
        .search_with_options(SearchOptions::new(1, "item", "vec", vec![1.0, 0.0, 0.0], 2))
        .await
        .expect("search should succeed");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].id, PointId::Num(1));
    assert!(results[0].score >= 0.9999);

    let got = vector_api
        .get_vector(1, "item", "vec", "2")
        .await
        .expect("get_vector should succeed")
        .expect("point 2 should exist");
    assert_eq!(got.vector, vec![0.0, 1.0, 0.0]);

    let count = vector_api
        .count(1, "item", "vec")
        .await
        .expect("count should succeed");
    assert_eq!(count, 3);

    vector_api
        .delete_vector(1, "item", "vec", "2")
        .await
        .expect("delete_vector should succeed");
    let count_after_delete = vector_api
        .count(1, "item", "vec")
        .await
        .expect("count after delete should succeed");
    assert_eq!(count_after_delete, 2);

    vector_api
        .drop_index(1, "item", "vec")
        .await
        .expect("drop_index should succeed");
    let list = vector_api.list_indexes();
    assert!(list.is_empty());
}

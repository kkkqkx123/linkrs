//! Vector sync integration tests at the root crate boundary.
//!
//! These tests exercise the public coordinator surface against the remote
//! backend shell (a disabled Qdrant manager) and therefore require
//! `vector-qdrant`. Disabled-engine contract: user-facing operations fail
//! with `EngineDisabled`, delivery-plane batches are skipped and accounted,
//! logical index metadata keeps working.
#![cfg(feature = "vector-qdrant")]

use std::collections::HashMap;
use std::sync::Arc;

use graphdb::core::Value;
use graphdb::sync::{
    VectorBackend, VectorChangeContext, VectorChangeType, VectorClientConfig, VectorIndexLocation,
    VectorManager, VectorPointData, VectorSyncCoordinator,
};

async fn disabled_coordinator() -> Arc<VectorSyncCoordinator> {
    let backend = VectorBackend::qdrant(Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .unwrap(),
    ));
    Arc::new(VectorSyncCoordinator::new_without_embedding(
        backend,
        tokio::runtime::Handle::current(),
    ))
}

#[tokio::test]
async fn disabled_engine_reports_disabled_state() {
    let coordinator = disabled_coordinator().await;
    assert_eq!(
        coordinator.engine_state(),
        graphdb::sync::VectorEngineState::Disabled
    );
}

#[tokio::test]
async fn disabled_engine_skips_delivery_and_fails_searches() {
    let coordinator = disabled_coordinator().await;
    assert_eq!(coordinator.disabled_skip_count(), 0);

    // Register logical indexes so delivery has real targets to skip.
    coordinator
        .create_vector_index(
            1,
            "docs",
            "embedding",
            3,
            graphdb::sync::vector_sync::DistanceMetric::Cosine,
        )
        .await
        .unwrap();
    assert!(coordinator.index_exists(1, "docs", "embedding"));

    let contexts = vec![
        VectorChangeContext::new(
            1,
            "docs",
            "embedding",
            VectorChangeType::Insert,
            VectorPointData {
                id: "doc_1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                payload: HashMap::new(),
            },
        ),
        VectorChangeContext::new(
            1,
            "docs",
            "embedding",
            VectorChangeType::Delete,
            VectorPointData {
                id: "doc_2".to_string(),
                vector: Vec::new(),
                payload: HashMap::new(),
            },
        ),
    ];
    // Delivery plane: skipped, but accounted and reported as Ok so the sync
    // pipeline keeps recording its LSN.
    coordinator
        .on_vector_change_batch(contexts)
        .await
        .expect("delivery to a disabled engine is a skip-and-account no-op");
    assert_eq!(
        coordinator.disabled_skip_count(),
        2,
        "every skipped item must be counted"
    );

    // Query plane: loud typed error instead of an empty set that would be
    // indistinguishable from "no matching data".
    let error = coordinator
        .search_with_options(graphdb::sync::vector_sync::SearchOptions::new(
            1,
            "docs",
            "embedding",
            vec![1.0, 2.0, 3.0],
            10,
        ))
        .await
        .expect_err("searches against a disabled engine must fail loudly");
    assert!(
        error.to_string().contains("disabled"),
        "error should identify the disabled engine: {error}"
    );
}

#[tokio::test]
async fn index_location_naming_and_logical_isolation() {
    let location = VectorIndexLocation::new(5, "Products", "image_embedding");
    assert_eq!(location.space_id, 5);
    assert_eq!(location.tag_name, "Products");
    assert_eq!(location.field_name, "image_embedding");

    // All indexes of a space share one physical collection; the group id is
    // the logical isolation key inside it.
    assert_eq!(location.to_collection_name(), "space_5");
    assert_eq!(location.group_id(), "Products_image_embedding");
}

#[tokio::test]
async fn point_data_carries_user_payload() {
    let mut payload: HashMap<String, Value> = HashMap::new();
    payload.insert("category".to_string(), Value::string("electronics"));
    payload.insert("price".to_string(), Value::string("99.99"));

    let point = VectorPointData {
        id: "point_1".to_string(),
        vector: vec![1.5, 2.5, 3.5, 4.5],
        payload,
    };

    assert_eq!(point.id, "point_1");
    assert_eq!(point.vector.len(), 4);
    assert_eq!(
        point.payload.get("category").unwrap(),
        &Value::string("electronics")
    );
    assert_eq!(point.payload.get("price").unwrap(), &Value::string("99.99"));
}

#[tokio::test]
async fn multiple_locations_deliver_in_one_batch() {
    let coordinator = disabled_coordinator().await;

    let locations: Vec<(u64, &str, &str)> = vec![
        (1, "users", "face_embedding"),
        (1, "products", "image_embedding"),
        (2, "articles", "text_embedding"),
    ];
    let contexts = locations
        .iter()
        .enumerate()
        .map(|(i, (space, tag, field))| {
            VectorChangeContext::new(
                *space,
                *tag,
                *field,
                VectorChangeType::Insert,
                VectorPointData {
                    id: format!("item_{}", i),
                    vector: vec![i as f32; 4],
                    payload: HashMap::new(),
                },
            )
        })
        .collect();

    coordinator
        .on_vector_change_batch(contexts)
        .await
        .expect("multi-location delivery is a skip-and-account no-op on a disabled engine");
    assert_eq!(
        coordinator.disabled_skip_count(),
        locations.len() as u64,
        "skips from every location must be counted"
    );
}

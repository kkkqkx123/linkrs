use crate::e2e_tests::common::{create_e2e_client, ensure_deleted, test_collection};
use crate::e2e_tests::e2e_cleanup::cleanup_old_e2e_collections;
use vector_client::types::{CollectionConfig, DistanceMetric, SearchQuery, VectorPoint};

#[tokio::test]
async fn test_e2e_basic_crud() {
    cleanup_old_e2e_collections();
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("basic_crud");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .await
        .expect("create collection");

    assert!(
        client
            .engine()
            .collection_exists(&collection)
            .await
            .expect("exists"),
        "collection should exist",
    );

    let point = VectorPoint::new(1u64, vec![1.0f32; 4]);
    client
        .engine()
        .upsert(&collection, point)
        .await
        .expect("upsert");

    let count = client.engine().count(&collection).await.expect("count");
    assert_eq!(count, 1);

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
    assert!(
        !client
            .engine()
            .collection_exists(&collection)
            .await
            .expect("not exists"),
        "collection should not exist after delete",
    );
}

#[tokio::test]
async fn test_e2e_search_basic() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("search_basic");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .await
        .expect("create");

    let mut points = Vec::new();
    for i in 0..4u64 {
        let mut v = vec![0.0f32; 4];
        v[i as usize] = 1.0;
        points.push(VectorPoint::new(i + 1, v));
    }
    client
        .engine()
        .upsert_batch(&collection, points)
        .await
        .expect("upsert batch");

    let mut query = vec![0.0f32; 4];
    query[0] = 1.0;
    let search_query = SearchQuery::new(query, 3);
    let results = client
        .engine()
        .search(&collection, search_query)
        .await
        .expect("search");

    assert!(!results.is_empty(), "should find results");
    assert!(results.len() <= 3, "should respect limit");

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_health_check() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let status = client.engine().health_check().await.expect("health check");
    assert!(status.is_healthy, "qdrant should be healthy");
}

#[tokio::test]
async fn test_e2e_collection_info() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("info_test");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Euclid),
        )
        .await
        .expect("create");

    let info = client
        .engine()
        .collection_info(&collection)
        .await
        .expect("info");
    assert_eq!(info.name, collection);
    assert_eq!(info.config.vector_size, 4);
    assert_eq!(info.config.distance, DistanceMetric::Euclid);

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_payload_operations() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("payload_ops");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .await
        .expect("create");

    let mut payload = std::collections::HashMap::new();
    payload.insert("title".to_string(), serde_json::json!("Test Doc"));
    let point = VectorPoint::new(1u64, vec![0.5f32; 4]).with_payload(payload.clone());
    client
        .engine()
        .upsert(&collection, point)
        .await
        .expect("upsert");

    let result = client
        .engine()
        .get(&collection, "1")
        .await
        .expect("get")
        .expect("point exists");
    assert!(result.payload.is_some());
    let p = result.payload.unwrap();
    assert_eq!(p.get("title").and_then(|v| v.as_str()), Some("Test Doc"));

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

#[tokio::test]
async fn test_e2e_scroll() {
    let client = match create_e2e_client().await {
        Some(c) => c,
        None => return,
    };
    let collection = test_collection("scroll_test");
    ensure_deleted(&client, &collection).await;

    client
        .engine()
        .create_collection(
            &collection,
            CollectionConfig::new(4, DistanceMetric::Cosine),
        )
        .await
        .expect("create");

    let points: Vec<VectorPoint> = (0..5u64)
        .map(|i| VectorPoint::new(i + 1, vec![i as f32; 4]))
        .collect();
    client
        .engine()
        .upsert_batch(&collection, points)
        .await
        .expect("upsert batch");

    let (page1, next) = client
        .engine()
        .scroll(&collection, 3, None, None, None)
        .await
        .expect("scroll");
    assert_eq!(page1.len(), 3, "first page should have 3 items");
    assert!(next.is_some(), "should have next offset");

    let (page2, _next2) = client
        .engine()
        .scroll(&collection, 3, next.as_deref(), None, None)
        .await
        .expect("scroll");
    assert!(page2.len() <= 3, "second page should have at most 3 items");

    client
        .engine()
        .delete_collection(&collection)
        .await
        .expect("delete");
}

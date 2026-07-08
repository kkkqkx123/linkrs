use super::common::{create_payload, create_test_points, generate_test_vectors, VectorTestContext};
use vector_client::types::{
    DistanceMetric, FilterCondition, PayloadSchemaType, SearchQuery, VectorFilter, VectorPoint,
};

/// Scroll through all points
#[tokio::test]
async fn test_scroll_all_points() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(15, 64, 42);
    let ids: Vec<String> = (0..15).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let collection = "space_1_Doc_emb";
    let (all_points, _next) = ctx
        .manager
        .engine()
        .scroll(collection, 100, None, None, None)
        .await
        .expect("scroll");
    assert_eq!(all_points.len(), 15);
}

/// Scroll with pagination (limit + offset)
#[tokio::test]
async fn test_scroll_with_pagination() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(10, 64, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let collection = "space_1_Doc_emb";
    let (page1, next) = ctx
        .manager
        .engine()
        .scroll(collection, 4, None, None, None)
        .await
        .expect("scroll");
    assert_eq!(page1.len(), 4);
    assert!(next.is_some(), "should have next page offset");

    let (page2, next2) = ctx
        .manager
        .engine()
        .scroll(collection, 4, next.as_deref(), None, None)
        .await
        .expect("scroll");
    assert_eq!(page2.len(), 4);
    assert!(next2.is_some(), "should have third page offset");

    let (page3, next3) = ctx
        .manager
        .engine()
        .scroll(collection, 4, next2.as_deref(), None, None)
        .await
        .expect("scroll");
    assert_eq!(page3.len(), 2);
    assert!(next3.is_none(), "no more pages");
}

/// Delete by filter
#[tokio::test]
async fn test_delete_by_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let pts: Vec<VectorPoint> = (0..5)
        .map(|i| {
            let cat = if i < 3 { "tech" } else { "news" };
            VectorPoint::new(format!("doc_{}", i), vectors[i].clone())
                .with_payload(create_payload(vec![("cat", serde_json::json!(cat))]))
        })
        .collect();
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must(FilterCondition::match_value("cat", "tech"));
    ctx.manager
        .engine()
        .delete_by_filter("space_1_Doc_emb", filter)
        .await
        .expect("delete");

    let count = ctx.count(1, "Doc", "emb").await.expect("count");
    assert_eq!(count, 2, "should have 2 non-tech docs remaining");
}

/// Set payload on existing points
#[tokio::test]
async fn test_set_payload() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(2, 64, 42);
    let ids: Vec<&str> = vec!["d1", "d2"];
    let points = create_test_points(ids, vectors.clone(), None);
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let payload = create_payload(vec![("status", serde_json::json!("active"))]);
    ctx.manager
        .engine()
        .set_payload("space_1_Doc_emb", vec!["d1"], payload)
        .await
        .expect("set payload");

    let point = ctx
        .get_vector(1, "Doc", "emb", "d1")
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(
        point
            .payload
            .as_ref()
            .and_then(|p| p.get("status"))
            .and_then(|v| v.as_str()),
        Some("active")
    );
}

/// Delete payload from existing points
#[tokio::test]
async fn test_delete_payload() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(1, 64, 42);
    let point = VectorPoint::new("d1", vectors[0].clone()).with_payload(create_payload(vec![
        ("keep", serde_json::json!("yes")),
        ("remove", serde_json::json!("bye")),
    ]));
    ctx.insert_test_vectors(1, "Doc", "emb", vec![point])
        .await
        .expect("insert");

    ctx.manager
        .engine()
        .delete_payload("space_1_Doc_emb", vec!["d1"], vec!["remove"])
        .await
        .expect("delete payload");

    let point = ctx
        .get_vector(1, "Doc", "emb", "d1")
        .await
        .expect("get")
        .expect("exists");
    let p = point.payload.unwrap();
    assert_eq!(p.get("keep").and_then(|v| v.as_str()), Some("yes"));
    assert!(!p.contains_key("remove"), "remove field should be gone");
}

/// Create, list, and delete payload indexes
#[tokio::test]
async fn test_payload_index_crud() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    ctx.manager
        .engine()
        .create_payload_index("space_1_Doc_emb", "category", PayloadSchemaType::Keyword)
        .await
        .expect("create payload index");

    let indexes = ctx
        .manager
        .engine()
        .list_payload_indexes("space_1_Doc_emb")
        .await
        .expect("list");
    assert!(
        !indexes.is_empty(),
        "should have at least one payload index"
    );

    ctx.manager
        .engine()
        .delete_payload_index("space_1_Doc_emb", "category")
        .await
        .expect("delete payload index");
}

/// Collection info
#[tokio::test]
async fn test_collection_info() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Doc", "emb", Some(128), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(5, 128, 42);
    let ids: Vec<String> = (0..5).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let info = ctx
        .manager
        .engine()
        .collection_info("space_1_Doc_emb")
        .await
        .expect("info");
    assert_eq!(info.name, "space_1_Doc_emb");
    assert_eq!(info.config.vector_size, 128);
    assert_eq!(info.config.distance, DistanceMetric::Cosine);
}

/// Health check
#[tokio::test]
async fn test_health_check() {
    let ctx = VectorTestContext::with_dimension(64);
    let status = ctx
        .manager
        .engine()
        .health_check()
        .await
        .expect("health check");
    assert!(status.is_healthy);
}

/// Batch insert + search_batch
#[tokio::test]
async fn test_search_batch() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(10, 64, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let queries: Vec<SearchQuery> = vectors[0..3]
        .iter()
        .map(|v| SearchQuery::new(v.clone(), 5))
        .collect();
    let results = ctx
        .manager
        .engine()
        .search_batch("space_1_Doc_emb", queries)
        .await
        .expect("batch search");

    assert_eq!(results.len(), 3);
    for (i, r) in results.iter().enumerate() {
        assert!(!r.is_empty(), "result set {} should not be empty", i);
    }
}

/// Delete by filter with must_not
#[tokio::test]
async fn test_delete_by_filter_must_not() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(4, 64, 42);
    let pts: Vec<VectorPoint> = (0..4)
        .map(|i| {
            let cat = if i % 2 == 0 { "even" } else { "odd" };
            VectorPoint::new(format!("doc_{}", i), vectors[i].clone())
                .with_payload(create_payload(vec![("parity", serde_json::json!(cat))]))
        })
        .collect();
    ctx.insert_test_vectors(1, "Doc", "emb", pts)
        .await
        .expect("insert");

    let filter = VectorFilter::new().must_not(FilterCondition::match_value("parity", "even"));
    ctx.manager
        .engine()
        .delete_by_filter("space_1_Doc_emb", filter)
        .await
        .expect("delete");

    let count = ctx.count(1, "Doc", "emb").await.expect("count");
    assert_eq!(count, 2, "even docs should remain");
}

/// Non-existent collection errors
#[tokio::test]
async fn test_scroll_nonexistent_collection() {
    let ctx = VectorTestContext::with_dimension(64);
    let result = ctx
        .manager
        .engine()
        .scroll("does_not_exist", 10, None, None, None)
        .await;
    assert!(result.is_err(), "scroll on missing collection should fail");
}

#[tokio::test]
async fn test_set_payload_nonexistent_collection() {
    let ctx = VectorTestContext::with_dimension(64);
    let result = ctx
        .manager
        .engine()
        .set_payload("does_not_exist", vec!["x"], Default::default())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_collection_info_nonexistent() {
    let ctx = VectorTestContext::with_dimension(64);
    let result = ctx
        .manager
        .engine()
        .collection_info("no_such_collection")
        .await;
    assert!(result.is_err());
}

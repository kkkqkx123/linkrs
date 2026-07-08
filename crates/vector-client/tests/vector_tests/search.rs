//! Vector Integration Tests - Search Functionality
//!
//! Test scope:
//! - Similarity search with different distance metrics
//! - Filtered search with payload conditions
//! - Batch search operations
//! - Search with pagination
//!
//! Test cases: TC-VEC-SEARCH-001 ~ TC-VEC-SEARCH-010

use super::common::{
    assert_results_sorted_by_score, assert_search_result_contains, create_payload,
    create_test_points, generate_test_vectors, VectorTestContext,
};
use vector_client::types::{
    DistanceMetric, FilterCondition, SearchQuery, VectorFilter, VectorPoint,
};

// ==================== Distance Metric Tests ====================

/// TC-VEC-SEARCH-001: Cosine Similarity Search
#[tokio::test]
async fn test_cosine_similarity_search() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let mut vectors = Vec::new();
    for i in 0..5 {
        let mut v = vec![0.0f32; 64];
        v[i] = 1.0;
        vectors.push(v);
    }

    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points = create_test_points(ids.to_vec(), vectors.clone(), None);

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vec![1.0f32, 0.0, 0.0, 0.0, 0.0];
    let mut query_vec = vec![0.0f32; 64];
    query_vec[0..5].copy_from_slice(&query);

    let results = ctx
        .search(1, "Document", "embedding", query_vec, 10)
        .await
        .expect("Search should succeed");

    assert!(!results.is_empty(), "Should have results");
    assert_eq!(
        results[0].id.to_string(),
        "doc_1",
        "First result should be doc_1 with highest similarity"
    );
    assert!(
        results[0].score > 0.99,
        "Score should be close to 1.0 for identical vectors"
    );
}

/// TC-VEC-SEARCH-002: Euclidean Distance Search
#[tokio::test]
async fn test_euclidean_distance_search() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Euclid),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points = create_test_points(ids.to_vec(), vectors.clone(), None);

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();
    let results = ctx
        .search(1, "Document", "embedding", query, 10)
        .await
        .expect("Search should succeed");

    assert_results_sorted_by_score(&results).expect("Results should be sorted by score");
    assert_eq!(
        results[0].id.to_string(),
        "doc_1",
        "First result should be doc_1"
    );
}

/// TC-VEC-SEARCH-003: Dot Product Search
#[tokio::test]
async fn test_dot_product_search() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Dot),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points = create_test_points(ids.to_vec(), vectors.clone(), None);

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();
    let results = ctx
        .search(1, "Document", "embedding", query, 10)
        .await
        .expect("Search should succeed");

    assert_results_sorted_by_score(&results).expect("Results should be sorted by score");
}

// ==================== Filtered Search Tests ====================

/// TC-VEC-SEARCH-004: Search with Payload Filter
#[tokio::test]
async fn test_search_with_payload_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let payloads: Vec<_> = (0..5)
        .map(|i| {
            create_payload(vec![(
                "category",
                serde_json::json!(if i < 3 { "tech" } else { "news" }),
            )])
        })
        .collect();

    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points: Vec<VectorPoint> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            VectorPoint::new(id.to_string(), vectors[i].clone()).with_payload(payloads[i].clone())
        })
        .collect();

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();
    let filter = VectorFilter::new().must(FilterCondition::match_value("category", "tech"));

    let collection_name = "space_1_Document_embedding";
    let search_query = SearchQuery::new(query, 10).with_filter(filter);

    let results = ctx
        .manager
        .search(collection_name, search_query)
        .await
        .expect("Search should succeed");

    for result in &results {
        if let Some(ref payload) = result.payload {
            if let Some(cat) = payload.get("category") {
                assert_eq!(
                    cat.as_str().unwrap(),
                    "tech",
                    "Should only return tech category"
                );
            }
        }
    }
}

/// TC-VEC-SEARCH-005: Search with Range Filter
#[tokio::test]
async fn test_search_with_range_filter() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let payloads: Vec<_> = (0..5)
        .map(|i| create_payload(vec![("score", serde_json::json!(i as f64 * 10.0))]))
        .collect();

    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points: Vec<VectorPoint> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            VectorPoint::new(id.to_string(), vectors[i].clone()).with_payload(payloads[i].clone())
        })
        .collect();

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();
    let filter = VectorFilter::new().must(FilterCondition::range(
        "score",
        vector_client::types::RangeCondition::new()
            .gte(20.0)
            .lt(50.0),
    ));

    let collection_name = "space_1_Document_embedding";
    let search_query = SearchQuery::new(query, 10).with_filter(filter);

    let results = ctx
        .manager
        .search(collection_name, search_query)
        .await
        .expect("Search should succeed");

    for result in &results {
        if let Some(ref payload) = result.payload {
            if let Some(score) = payload.get("score") {
                let score_val = score.as_f64().unwrap();
                assert!(
                    (20.0..50.0).contains(&score_val),
                    "Score should be in range [20, 50)"
                );
            }
        }
    }
}

// ==================== Batch Search Tests ====================

/// TC-VEC-SEARCH-006: Batch Search
#[tokio::test]
async fn test_batch_search() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(10, 64, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let queries: Vec<SearchQuery> = vectors[0..3]
        .iter()
        .map(|v| SearchQuery::new(v.clone(), 5))
        .collect();

    let collection_name = "space_1_Document_embedding";
    let results = ctx
        .manager
        .engine()
        .search_batch(collection_name, queries)
        .await
        .expect("Batch search should succeed");

    assert_eq!(results.len(), 3, "Should have 3 result sets");
    for (i, result_set) in results.iter().enumerate() {
        assert!(
            !result_set.is_empty(),
            "Result set {} should not be empty",
            i
        );
    }
}

// ==================== Pagination Tests ====================

/// TC-VEC-SEARCH-007: Search with Offset
#[tokio::test]
async fn test_search_with_offset() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(20, 64, 42);
    let ids: Vec<String> = (0..20).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();

    let collection_name = "space_1_Document_embedding";
    let search_query = SearchQuery::new(query.clone(), 10).with_offset(5);

    let results = ctx
        .manager
        .search(collection_name, search_query)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 10, "Should return 10 results with offset");
}

/// TC-VEC-SEARCH-008: Search with Vector Return
#[tokio::test]
async fn test_search_with_vector_return() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points = create_test_points(ids.to_vec(), vectors.clone(), None);

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();

    let collection_name = "space_1_Document_embedding";
    let search_query = SearchQuery::new(query, 10).with_vector(true);

    let results = ctx
        .manager
        .search(collection_name, search_query)
        .await
        .expect("Search should succeed");

    for result in &results {
        assert!(result.vector.is_some(), "Vector should be returned");
        assert_eq!(
            result.vector.as_ref().unwrap().len(),
            64,
            "Vector dimension should be 64"
        );
    }
}

/// TC-VEC-SEARCH-009: Search without Payload
#[tokio::test]
async fn test_search_without_payload() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let payloads: Vec<_> = (0..5)
        .map(|i| create_payload(vec![("title", serde_json::json!(format!("Doc {}", i)))]))
        .collect();

    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points: Vec<VectorPoint> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            VectorPoint::new(id.to_string(), vectors[i].clone()).with_payload(payloads[i].clone())
        })
        .collect();

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();

    let collection_name = "space_1_Document_embedding";
    let search_query = SearchQuery::new(query, 10).with_payload(false);

    let results = ctx
        .manager
        .search(collection_name, search_query)
        .await
        .expect("Search should succeed");

    for result in &results {
        assert!(result.payload.is_none(), "Payload should not be returned");
    }
}

/// TC-VEC-SEARCH-010: Multi-space Isolation
#[tokio::test]
async fn test_multi_space_isolation() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index for space 1");

    ctx.create_test_index(
        2,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index for space 2");

    let vectors = generate_test_vectors(5, 64, 42);
    let ids: Vec<&str> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"];
    let points = create_test_points(ids.to_vec(), vectors.clone(), None);

    ctx.insert_test_vectors(1, "Document", "embedding", points.clone())
        .await
        .expect("Failed to insert vectors to space 1");

    let vectors2 = generate_test_vectors(3, 64, 100);
    let ids2: Vec<&str> = vec!["doc_a", "doc_b", "doc_c"];
    let points2 = create_test_points(ids2.to_vec(), vectors2.clone(), None);

    ctx.insert_test_vectors(2, "Document", "embedding", points2)
        .await
        .expect("Failed to insert vectors to space 2");

    let count1 = ctx
        .count(1, "Document", "embedding")
        .await
        .expect("Failed to count space 1");
    let count2 = ctx
        .count(2, "Document", "embedding")
        .await
        .expect("Failed to count space 2");

    assert_eq!(count1, 5, "Space 1 should have 5 vectors");
    assert_eq!(count2, 3, "Space 2 should have 3 vectors");

    let results1 = ctx
        .search(1, "Document", "embedding", vectors[0].clone(), 10)
        .await
        .expect("Search in space 1 should succeed");

    let results2 = ctx
        .search(2, "Document", "embedding", vectors2[0].clone(), 10)
        .await
        .expect("Search in space 2 should succeed");

    assert_search_result_contains(&results1, "doc_1").expect("Space 1 should contain doc_1");
    assert_search_result_contains(&results2, "doc_a").expect("Space 2 should contain doc_a");
}

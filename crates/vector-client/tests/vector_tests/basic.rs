//! Vector Integration Tests - Basic CRUD Operations
//!
//! Test scope:
//! - Index metadata management (create, drop, query, list)
//! - Vector operations (insert, update, delete, batch operations)
//! - Search functionality (similarity search, limit, threshold)
//!
//! Test cases: TC-VEC-001 ~ TC-VEC-015

use super::common::{
    assert_results_sorted_by_score, assert_scores_above_threshold, assert_search_result_contains,
    assert_search_result_count, create_simple_payload, create_test_points, generate_random_vector,
    generate_test_vectors, VectorTestContext,
};
use vector_client::types::DistanceMetric;

// ==================== Index Management Tests ====================

/// TC-VEC-001: Create Vector Index with Cosine Distance
#[tokio::test]
async fn test_create_vector_index_cosine() {
    let ctx = VectorTestContext::new();

    let result = ctx
        .create_test_index(
            1,
            "Document",
            "embedding",
            Some(128),
            Some(DistanceMetric::Cosine),
        )
        .await;

    assert!(result.is_ok(), "Index creation should succeed");
    let collection_name = result.unwrap();

    assert_eq!(
        collection_name, "space_1_Document_embedding",
        "Collection name format should be correct"
    );

    assert!(
        ctx.has_index(1, "Document", "embedding"),
        "Index should exist after creation"
    );
}

/// TC-VEC-001b: Create Vector Index with Euclidean Distance
#[tokio::test]
async fn test_create_vector_index_euclidean() {
    let ctx = VectorTestContext::new();

    let result = ctx
        .create_test_index(
            1,
            "Document",
            "embedding",
            Some(128),
            Some(DistanceMetric::Euclid),
        )
        .await;

    assert!(
        result.is_ok(),
        "Index creation with Euclidean distance should succeed"
    );
}

/// TC-VEC-001c: Create Vector Index with Dot Product Distance
#[tokio::test]
async fn test_create_vector_index_dot() {
    let ctx = VectorTestContext::new();

    let result = ctx
        .create_test_index(
            1,
            "Document",
            "embedding",
            Some(128),
            Some(DistanceMetric::Dot),
        )
        .await;

    assert!(
        result.is_ok(),
        "Index creation with Dot distance should succeed"
    );
}

/// TC-VEC-002: Create Duplicate Index
#[tokio::test]
async fn test_create_duplicate_index() {
    let ctx = VectorTestContext::new();

    let result1 = ctx
        .create_test_index(
            1,
            "Document",
            "embedding",
            Some(128),
            Some(DistanceMetric::Cosine),
        )
        .await;
    assert!(result1.is_ok(), "First index creation should succeed");

    let result2 = ctx
        .create_test_index(
            1,
            "Document",
            "embedding",
            Some(128),
            Some(DistanceMetric::Cosine),
        )
        .await;

    assert!(result2.is_err(), "Duplicate index creation should fail");
    assert!(
        matches!(
            result2.unwrap_err(),
            vector_client::VectorClientError::IndexAlreadyExists(_)
        ),
        "Should return IndexAlreadyExists error"
    );
}

/// TC-VEC-003: Drop Index
#[tokio::test]
async fn test_drop_index() {
    let ctx = VectorTestContext::new();

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(128),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let result = ctx.drop_index(1, "Document", "embedding").await;
    assert!(result.is_ok(), "Index drop should succeed");

    assert!(
        !ctx.has_index(1, "Document", "embedding"),
        "Index should not exist after dropping"
    );
}

/// TC-VEC-004: Get Index Metadata
#[tokio::test]
async fn test_get_index_metadata() {
    let ctx = VectorTestContext::new();

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(128),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let metadata = ctx.manager.get_index_metadata("space_1_Document_embedding");
    assert!(metadata.is_some(), "Metadata should exist");

    let metadata = metadata.unwrap();
    assert_eq!(metadata.name, "space_1_Document_embedding");
    assert_eq!(metadata.config.vector_size, 128);
    assert_eq!(metadata.config.distance, DistanceMetric::Cosine);
}

/// TC-VEC-005: List Space Indexes
#[tokio::test]
async fn test_list_indexes() {
    let ctx = VectorTestContext::new();

    ctx.create_test_index(
        1,
        "Document",
        "title_emb",
        Some(128),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");
    ctx.create_test_index(
        1,
        "Document",
        "content_emb",
        Some(256),
        Some(DistanceMetric::Euclid),
    )
    .await
    .expect("Failed to create index");
    ctx.create_test_index(1, "Image", "feature", Some(512), Some(DistanceMetric::Dot))
        .await
        .expect("Failed to create index");

    let indexes = ctx.manager.list_indexes();
    assert_eq!(indexes.len(), 3, "Should have 3 indexes");
}

// ==================== Vector Operation Tests ====================

/// TC-VEC-006: Insert Single Vector
#[tokio::test]
async fn test_insert_single_vector() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vector = generate_random_vector(128);
    let result = ctx
        .insert_test_vector(1, "Document", "embedding", "doc_1", vector.clone(), None)
        .await;

    assert!(result.is_ok(), "Insert should succeed");

    let count = ctx
        .count(1, "Document", "embedding")
        .await
        .expect("Failed to count");
    assert_eq!(count, 1, "Should have 1 vector");
}

/// TC-VEC-007: Insert Vector with Payload
#[tokio::test]
async fn test_insert_vector_with_payload() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vector = generate_random_vector(128);
    let payload = create_simple_payload("title", "Test Document");

    let result = ctx
        .insert_test_vector(1, "Document", "embedding", "doc_1", vector, Some(payload))
        .await;

    assert!(result.is_ok(), "Insert with payload should succeed");

    let point = ctx
        .get_vector(1, "Document", "embedding", "doc_1")
        .await
        .expect("Failed to get vector")
        .expect("Vector should exist");

    assert!(point.payload.is_some(), "Payload should be present");
    let payload = point.payload.unwrap();
    assert_eq!(
        payload.get("title").unwrap().as_str().unwrap(),
        "Test Document"
    );
}

/// TC-VEC-008: Batch Insert Vectors
#[tokio::test]
async fn test_batch_insert_vectors() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vectors = generate_test_vectors(10, 128, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);

    let result = ctx
        .insert_test_vectors(1, "Document", "embedding", points)
        .await;
    assert!(result.is_ok(), "Batch insert should succeed");

    let count = ctx
        .count(1, "Document", "embedding")
        .await
        .expect("Failed to count");
    assert_eq!(count, 10, "Should have 10 vectors");
}

/// TC-VEC-009: Update Vector
#[tokio::test]
async fn test_update_vector() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let old_vector = generate_test_vectors(1, 128, 1).into_iter().next().unwrap();
    ctx.insert_test_vector(1, "Document", "embedding", "doc_1", old_vector, None)
        .await
        .expect("Failed to insert");

    let new_vector = generate_test_vectors(1, 128, 2).into_iter().next().unwrap();
    let result = ctx
        .insert_test_vector(
            1,
            "Document",
            "embedding",
            "doc_1",
            new_vector.clone(),
            None,
        )
        .await;

    assert!(result.is_ok(), "Update should succeed");

    let point = ctx
        .get_vector(1, "Document", "embedding", "doc_1")
        .await
        .expect("Failed to get vector")
        .expect("Vector should exist");

    assert_eq!(point.vector.len(), 128);
}

/// TC-VEC-010: Delete Vector
#[tokio::test]
async fn test_delete_vector() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vector = generate_random_vector(128);
    ctx.insert_test_vector(1, "Document", "embedding", "doc_1", vector, None)
        .await
        .expect("Failed to insert");

    let result = ctx.delete_vector(1, "Document", "embedding", "doc_1").await;
    assert!(result.is_ok(), "Delete should succeed");

    let point = ctx.get_vector(1, "Document", "embedding", "doc_1").await;
    assert!(point.is_ok(), "Get should succeed");
    assert!(point.unwrap().is_none(), "Vector should be deleted");
}

/// TC-VEC-011: Batch Delete Vectors
#[tokio::test]
async fn test_batch_delete_vectors() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vectors = generate_test_vectors(10, 128, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to batch insert");

    let collection_name = "space_1_Document_embedding";
    let ids_to_delete: Vec<&str> = ids[0..5].iter().map(|s| s.as_str()).collect();
    let result = ctx
        .manager
        .delete_batch(collection_name, ids_to_delete)
        .await;

    assert!(result.is_ok(), "Batch delete should succeed");

    let count = ctx
        .count(1, "Document", "embedding")
        .await
        .expect("Failed to count");
    assert_eq!(count, 5, "Should have 5 vectors remaining");
}

// ==================== Search Functionality Tests ====================

/// TC-VEC-012: Basic Similarity Search
#[tokio::test]
async fn test_basic_similarity_search() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vectors = generate_test_vectors(5, 128, 42);
    let ids: Vec<String> = vec!["doc_1", "doc_2", "doc_3", "doc_4", "doc_5"]
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();
    let results = ctx
        .search(1, "Document", "embedding", query, 10)
        .await
        .expect("Search should succeed");

    assert_search_result_contains(&results, "doc_1").expect("Should find doc_1");
    assert_results_sorted_by_score(&results).expect("Results should be sorted by score");
}

/// TC-VEC-013: Search with Limit
#[tokio::test]
async fn test_search_with_limit() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vectors = generate_test_vectors(100, 128, 42);
    let ids: Vec<String> = (0..100).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = generate_random_vector(128);
    let results = ctx
        .search(1, "Document", "embedding", query, 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 10).expect("Should return exactly 10 results");
}

/// TC-VEC-014: Search with Score Threshold
#[tokio::test]
async fn test_search_with_threshold() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vectors = generate_test_vectors(10, 128, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = vectors[0].clone();
    let threshold = 0.9;
    let results = ctx
        .search_with_threshold(1, "Document", "embedding", query, 100, threshold)
        .await
        .expect("Search should succeed");

    assert_scores_above_threshold(&results, threshold)
        .expect("All scores should be above threshold");
}

/// TC-VEC-015: Empty Search Results
#[tokio::test]
async fn test_empty_search_results() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let query = generate_random_vector(128);
    let results = ctx
        .search(1, "Document", "embedding", query, 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 0).expect("Should return empty results");
}

/// TC-VEC-016: Search with Very High Threshold
#[tokio::test]
async fn test_search_with_very_high_threshold() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(1, "Document", "embedding", None, None)
        .await
        .expect("Failed to create index");

    let vectors = generate_test_vectors(10, 128, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let query = generate_random_vector(128);
    let results = ctx
        .search_with_threshold(1, "Document", "embedding", query, 100, 0.9999)
        .await
        .expect("Search should succeed");

    assert!(
        results.len() <= 10,
        "Should return at most 10 results with very high threshold"
    );
}

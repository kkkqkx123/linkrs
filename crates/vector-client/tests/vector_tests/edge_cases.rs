//! Vector Integration Tests - Edge Cases
//!
//! Test scope:
//! - Empty vector handling
//! - Dimension mismatch
//! - Invalid operations
//! - Boundary conditions
//! - Unicode and special characters in payloads
//!
//! Test cases: TC-VEC-EDGE-001 ~ TC-VEC-EDGE-010

use super::common::{
    assert_search_result_count, create_payload, create_test_points, generate_random_vector,
    generate_test_vectors, VectorTestContext,
};
use vector_client::types::DistanceMetric;
use vector_client::VectorClientError;

/// TC-VEC-EDGE-001: Dimension Mismatch on Insert
#[tokio::test]
async fn test_dimension_mismatch_insert() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(128),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let wrong_dimension_vector = generate_random_vector(64);
    let result = ctx
        .insert_test_vector(
            1,
            "Document",
            "embedding",
            "doc_1",
            wrong_dimension_vector,
            None,
        )
        .await;

    assert!(result.is_err(), "Insert with wrong dimension should fail");
    let err = result.unwrap_err();
    assert!(
        matches!(err, VectorClientError::InvalidVectorDimension { .. }),
        "Should return InvalidVectorDimension error"
    );
}

/// TC-VEC-EDGE-002: Search in Non-existent Collection
#[tokio::test]
async fn test_search_nonexistent_collection() {
    let ctx = VectorTestContext::with_dimension(64);

    let query = generate_random_vector(64);
    let result = ctx.search(999, "Document", "embedding", query, 10).await;

    assert!(
        result.is_err(),
        "Search in non-existent collection should fail"
    );
}

/// TC-VEC-EDGE-003: Insert to Non-existent Collection
#[tokio::test]
async fn test_insert_nonexistent_collection() {
    let ctx = VectorTestContext::with_dimension(64);

    let vector = generate_random_vector(64);
    let result = ctx
        .insert_test_vector(999, "Document", "embedding", "doc_1", vector, None)
        .await;

    assert!(
        result.is_err(),
        "Insert to non-existent collection should fail"
    );
}

/// TC-VEC-EDGE-004: Get Non-existent Vector
#[tokio::test]
async fn test_get_nonexistent_vector() {
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

    let result = ctx
        .get_vector(1, "Document", "embedding", "nonexistent")
        .await;

    assert!(result.is_ok(), "Get should succeed");
    assert!(
        result.unwrap().is_none(),
        "Should return None for non-existent vector"
    );
}

/// TC-VEC-EDGE-005: Delete Non-existent Vector
#[tokio::test]
async fn test_delete_nonexistent_vector() {
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

    let result = ctx
        .delete_vector(1, "Document", "embedding", "nonexistent")
        .await;

    assert!(
        result.is_ok(),
        "Delete of non-existent vector should succeed"
    );
}

/// TC-VEC-EDGE-006: Empty Payload
#[tokio::test]
async fn test_empty_payload() {
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

    let vector = generate_random_vector(64);
    let empty_payload = std::collections::HashMap::new();

    let result = ctx
        .insert_test_vector(
            1,
            "Document",
            "embedding",
            "doc_1",
            vector,
            Some(empty_payload),
        )
        .await;

    assert!(result.is_ok(), "Insert with empty payload should succeed");

    let point = ctx
        .get_vector(1, "Document", "embedding", "doc_1")
        .await
        .expect("Failed to get vector")
        .expect("Vector should exist");

    assert!(point.payload.is_some(), "Payload should be present");
    assert!(point.payload.unwrap().is_empty(), "Payload should be empty");
}

/// TC-VEC-EDGE-007: Unicode in Payload
#[tokio::test]
async fn test_unicode_in_payload() {
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

    let vector = generate_random_vector(64);
    let payload = create_payload(vec![
        ("title", serde_json::json!("中文标题")),
        ("description", serde_json::json!("日本語の説明")),
        ("emoji", serde_json::json!("🎉🎊🎁")),
    ]);

    let result = ctx
        .insert_test_vector(1, "Document", "embedding", "doc_1", vector, Some(payload))
        .await;

    assert!(result.is_ok(), "Insert with unicode payload should succeed");

    let point = ctx
        .get_vector(1, "Document", "embedding", "doc_1")
        .await
        .expect("Failed to get vector")
        .expect("Vector should exist");

    let payload = point.payload.unwrap();
    assert_eq!(payload.get("title").unwrap().as_str().unwrap(), "中文标题");
    assert_eq!(
        payload.get("description").unwrap().as_str().unwrap(),
        "日本語の説明"
    );
    assert_eq!(payload.get("emoji").unwrap().as_str().unwrap(), "🎉🎊🎁");
}

/// TC-VEC-EDGE-008: Very Long Vector ID
#[tokio::test]
async fn test_very_long_vector_id() {
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

    let long_id = "doc_".repeat(100);
    let vector = generate_random_vector(64);

    let result = ctx
        .insert_test_vector(1, "Document", "embedding", &long_id, vector.clone(), None)
        .await;

    assert!(result.is_ok(), "Insert with long ID should succeed");

    let point = ctx
        .get_vector(1, "Document", "embedding", &long_id)
        .await
        .expect("Failed to get vector")
        .expect("Vector should exist");

    assert_eq!(point.id.to_string(), long_id);
}

/// TC-VEC-EDGE-009: Zero Vector
#[tokio::test]
async fn test_zero_vector() {
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

    let zero_vector = vec![0.0f32; 64];

    let result = ctx
        .insert_test_vector(
            1,
            "Document",
            "embedding",
            "doc_1",
            zero_vector.clone(),
            None,
        )
        .await;

    assert!(result.is_ok(), "Insert with zero vector should succeed");

    let query = generate_random_vector(64);
    let results = ctx
        .search(1, "Document", "embedding", query, 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 1).expect("Should find the zero vector");
}

/// TC-VEC-EDGE-010: Batch Insert
#[tokio::test]
async fn test_batch_insert() {
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

    let batch_size = 50;
    let vectors = generate_test_vectors(batch_size, 64, 42);
    let ids: Vec<String> = (0..batch_size).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(ids.iter().map(|s| s.as_str()).collect(), vectors, None);

    let result = ctx
        .insert_test_vectors(1, "Document", "embedding", points)
        .await;

    assert!(result.is_ok(), "Batch insert should succeed");

    let count = ctx
        .count(1, "Document", "embedding")
        .await
        .expect("Failed to count");
    assert_eq!(count, batch_size as u64, "Should have all vectors inserted");
}

/// TC-VEC-EDGE-011: Special Characters in Collection Name
#[tokio::test]
async fn test_special_characters_in_tag_name() {
    let ctx = VectorTestContext::with_dimension(64);

    let result = ctx
        .create_test_index(
            1,
            "Tag-With-Special",
            "embedding",
            Some(64),
            Some(DistanceMetric::Cosine),
        )
        .await;

    assert!(
        result.is_ok(),
        "Index creation with special characters in tag name should succeed"
    );
}

/// TC-VEC-EDGE-012: Nested Payload
#[tokio::test]
async fn test_nested_payload() {
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

    let vector = generate_random_vector(64);
    let nested_payload = serde_json::json!({
        "metadata": {
            "author": {
                "name": "Test Author",
                "email": "test@example.com"
            },
            "tags": ["tag1", "tag2", "tag3"]
        }
    });

    let payload: std::collections::HashMap<String, serde_json::Value> = {
        let mut map = std::collections::HashMap::new();
        map.insert("data".to_string(), nested_payload);
        map
    };

    let result = ctx
        .insert_test_vector(1, "Document", "embedding", "doc_1", vector, Some(payload))
        .await;

    assert!(result.is_ok(), "Insert with nested payload should succeed");

    let point = ctx
        .get_vector(1, "Document", "embedding", "doc_1")
        .await
        .expect("Failed to get vector")
        .expect("Vector should exist");

    let payload = point.payload.unwrap();
    assert!(payload.get("data").unwrap().get("metadata").is_some());
}

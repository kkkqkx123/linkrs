//! Vector Integration Tests - Concurrent Operations
//!
//! Test scope:
//! - Concurrent inserts
//! - Concurrent searches
//! - Mixed concurrent operations
//! - Thread safety verification
//!
//! Note: Tests use low concurrency (3-5 tasks) to verify correctness
//! without high load stress testing.
//!
//! Test cases: TC-VEC-CONC-001 ~ TC-VEC-CONC-005

use std::sync::Arc;

use super::common::{
    create_test_points, generate_random_vector, generate_test_vectors, VectorTestContext,
};
use tokio::task::JoinSet;
use vector_client::types::{DistanceMetric, VectorPoint};

/// TC-VEC-CONC-001: Concurrent Inserts
#[tokio::test]
async fn test_concurrent_inserts() {
    let ctx = Arc::new(VectorTestContext::with_dimension(64));

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let mut tasks = JoinSet::new();
    let num_tasks = 3;
    let vectors_per_task = 5;

    for task_id in 0..num_tasks {
        let ctx_clone = ctx.clone();
        tasks.spawn(async move {
            let vectors = generate_test_vectors(vectors_per_task, 64, task_id as u64);
            let points: Vec<VectorPoint> = vectors
                .into_iter()
                .enumerate()
                .map(|(i, v)| VectorPoint::new(format!("task_{}_doc_{}", task_id, i), v))
                .collect();

            ctx_clone
                .insert_test_vectors(1, "Document", "embedding", points)
                .await
                .expect("Insert should succeed");
        });
    }

    while tasks.join_next().await.is_some() {}

    let count = ctx
        .count(1, "Document", "embedding")
        .await
        .expect("Failed to count");
    assert_eq!(
        count,
        (num_tasks * vectors_per_task) as u64,
        "Should have all vectors inserted"
    );
}

/// TC-VEC-CONC-002: Concurrent Searches
#[tokio::test]
async fn test_concurrent_searches() {
    let ctx = Arc::new(VectorTestContext::with_dimension(64));

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

    let mut tasks = JoinSet::new();
    let num_searches = 5;

    for i in 0..num_searches {
        let ctx_clone = ctx.clone();
        let query = vectors[i as usize % vectors.len()].clone();
        tasks.spawn(async move {
            let results = ctx_clone
                .search(1, "Document", "embedding", query, 10)
                .await
                .expect("Search should succeed");
            assert!(!results.is_empty(), "Should have search results");
        });
    }

    while tasks.join_next().await.is_some() {}
}

/// TC-VEC-CONC-003: Concurrent Mixed Operations
#[tokio::test]
async fn test_concurrent_mixed_operations() {
    let ctx = Arc::new(VectorTestContext::with_dimension(64));

    ctx.create_test_index(
        1,
        "Document",
        "embedding",
        Some(64),
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to create index");

    let initial_vectors = generate_test_vectors(10, 64, 0);
    let initial_ids: Vec<String> = (0..10).map(|i| format!("initial_doc_{}", i)).collect();
    let initial_points = create_test_points(
        initial_ids.iter().map(|s| s.as_str()).collect(),
        initial_vectors.clone(),
        None,
    );

    ctx.insert_test_vectors(1, "Document", "embedding", initial_points)
        .await
        .expect("Failed to insert initial vectors");

    let mut tasks = JoinSet::new();

    for i in 0..3 {
        let ctx_clone = ctx.clone();
        let vectors = generate_test_vectors(3, 64, (i + 100) as u64);
        tasks.spawn(async move {
            let points: Vec<VectorPoint> = vectors
                .into_iter()
                .enumerate()
                .map(|(j, v)| VectorPoint::new(format!("concurrent_doc_{}_{}", i, j), v))
                .collect();
            ctx_clone
                .insert_test_vectors(1, "Document", "embedding", points)
                .await
                .expect("Insert should succeed");
        });
    }

    for i in 0..3 {
        let ctx_clone = ctx.clone();
        let query = initial_vectors[i as usize % initial_vectors.len()].clone();
        tasks.spawn(async move {
            let results = ctx_clone
                .search(1, "Document", "embedding", query, 10)
                .await
                .expect("Search should succeed");
            assert!(!results.is_empty(), "Should have search results");
        });
    }

    while tasks.join_next().await.is_some() {}

    let count = ctx
        .count(1, "Document", "embedding")
        .await
        .expect("Failed to count");
    assert!(count >= 10, "Should have at least initial vectors");
}

/// TC-VEC-CONC-004: Concurrent Reads and Writes
#[tokio::test]
async fn test_concurrent_reads_and_writes() {
    let ctx = Arc::new(VectorTestContext::with_dimension(64));

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
    let ids: Vec<String> = (0..5).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );

    ctx.insert_test_vectors(1, "Document", "embedding", points)
        .await
        .expect("Failed to insert vectors");

    let mut tasks = JoinSet::new();

    for id in ids.iter().take(2) {
        let ctx_clone = ctx.clone();
        let id = id.clone();
        tasks.spawn(async move {
            let result = ctx_clone.get_vector(1, "Document", "embedding", &id).await;
            assert!(result.is_ok(), "Get should succeed");
        });
    }

    for i in 0..2 {
        let ctx_clone = ctx.clone();
        let vector = generate_random_vector(64);
        let id = format!("new_doc_{}", i);
        tasks.spawn(async move {
            let result = ctx_clone
                .insert_test_vector(1, "Document", "embedding", &id, vector, None)
                .await;
            assert!(result.is_ok(), "Insert should succeed");
        });
    }

    for vector in vectors.iter().take(2) {
        let ctx_clone = ctx.clone();
        let query = vector.clone();
        tasks.spawn(async move {
            let result = ctx_clone
                .search(1, "Document", "embedding", query, 10)
                .await;
            assert!(result.is_ok(), "Search should succeed");
        });
    }

    while tasks.join_next().await.is_some() {}
}

/// TC-VEC-CONC-005: Concurrent Index Operations
#[tokio::test]
async fn test_concurrent_index_operations() {
    let ctx = Arc::new(VectorTestContext::with_dimension(64));

    let mut tasks = JoinSet::new();

    for i in 0..3 {
        let ctx_clone = ctx.clone();
        tasks.spawn(async move {
            let result = ctx_clone
                .create_test_index(
                    i,
                    "Document",
                    "embedding",
                    Some(64),
                    Some(DistanceMetric::Cosine),
                )
                .await;
            assert!(result.is_ok(), "Index creation should succeed");
        });
    }

    while tasks.join_next().await.is_some() {}

    for i in 0..3 {
        assert!(
            ctx.has_index(i, "Document", "embedding"),
            "Index {} should exist",
            i
        );
    }
}

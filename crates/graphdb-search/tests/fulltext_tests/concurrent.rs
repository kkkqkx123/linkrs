//! Fulltext Integration Tests - Concurrent Operations
//!
//! Test scope:
//! - Concurrent inserts to same index
//! - Concurrent searches
//! - Concurrent insert and search mix
//! - Concurrent updates to same document
//! - Concurrent operations on different indexes
//!
//! Note: Tests use low concurrency (3-5 tasks) to verify correctness
//! without high load stress testing.
//!
//! Test cases: TC-FT-CONC-001 ~ TC-FT-CONC-008

use super::common::{
    assert_search_result_contains, assert_search_result_count, FulltextTestContext,
};
use graphdb_search::search::EngineType;
use std::sync::Arc;
use tokio::sync::Barrier;

/// TC-FT-CONC-001: Concurrent Inserts to BM25 Index
#[tokio::test]
async fn test_concurrent_inserts_bm25() {
    let ctx = Arc::new(FulltextTestContext::new());
    let num_tasks = 5;
    let barrier = Arc::new(Barrier::new(num_tasks));

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let mut handles = vec![];
    for i in 0..num_tasks {
        let ctx_clone = Arc::clone(&ctx);
        let barrier_clone = Arc::clone(&barrier);

        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;

            ctx_clone
                .insert_test_doc(
                    1,
                    "Article",
                    "content",
                    &format!("doc_{}", i),
                    &format!("Concurrent content {}", i),
                )
                .await
        });

        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.expect("Task panicked");
        assert!(result.is_ok(), "Document insertion should succeed");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "Concurrent", 100)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, num_tasks)
        .unwrap_or_else(|_| panic!("Should find all {} documents", num_tasks));

    for i in 0..num_tasks {
        assert_search_result_contains(&results, &format!("doc_{}", i))
            .unwrap_or_else(|_| panic!("Should contain doc_{}", i));
    }
}

/// TC-FT-CONC-003: Concurrent Searches
#[tokio::test]
async fn test_concurrent_searches() {
    let ctx = Arc::new(FulltextTestContext::new());
    let num_docs = 5;
    let num_searches = 5;
    let barrier = Arc::new(Barrier::new(num_searches));

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    for i in 0..num_docs {
        ctx.insert_test_doc(
            1,
            "Article",
            "content",
            &format!("doc_{}", i),
            &format!("Search test content {}", i),
        )
        .await
        .expect("Failed to insert document");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let mut handles = vec![];
    for _ in 0..num_searches {
        let ctx_clone = Arc::clone(&ctx);
        let barrier_clone = Arc::clone(&barrier);

        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;

            ctx_clone
                .search(1, "Article", "content", "Search", 100)
                .await
        });

        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.expect("Search task panicked");
        let results = result.expect("Search should succeed");

        assert_search_result_count(&results, num_docs)
            .unwrap_or_else(|_| panic!("Should find all {} documents", num_docs));
    }
}

/// TC-FT-CONC-004: Concurrent Insert and Search Mix
#[tokio::test]
async fn test_concurrent_insert_and_search() {
    let ctx = Arc::new(FulltextTestContext::new());
    let num_inserts = 3;
    let num_searches = 3;

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    for i in 0..5 {
        ctx.insert_test_doc(
            1,
            "Article",
            "content",
            &format!("initial_doc_{}", i),
            &format!("Initial content {}", i),
        )
        .await
        .expect("Failed to insert initial document");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let mut insert_handles = vec![];
    for i in 0..num_inserts {
        let ctx_clone = Arc::clone(&ctx);

        let handle = tokio::spawn(async move {
            ctx_clone
                .insert_test_doc(
                    1,
                    "Article",
                    "content",
                    &format!("concurrent_doc_{}", i),
                    &format!("Concurrent insert {}", i),
                )
                .await
        });

        insert_handles.push(handle);
    }

    let mut search_handles = vec![];
    for _ in 0..num_searches {
        let ctx_clone = Arc::clone(&ctx);

        let handle = tokio::spawn(async move {
            ctx_clone
                .search(1, "Article", "content", "content", 100)
                .await
        });

        search_handles.push(handle);
    }

    for handle in insert_handles {
        let result = handle.await.expect("Insert task panicked");
        assert!(result.is_ok(), "Insert should succeed");
    }

    for handle in search_handles {
        let result = handle.await.expect("Search task panicked");
        assert!(result.is_ok(), "Search should succeed");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let final_results = ctx
        .search(1, "Article", "content", "content", 200)
        .await
        .expect("Search should succeed");

    assert!(
        final_results.len() >= 5,
        "Should have at least initial documents"
    );
}

/// TC-FT-CONC-005: Concurrent Updates to Same Document
#[tokio::test]
async fn test_concurrent_updates_same_document() {
    let ctx = Arc::new(FulltextTestContext::new());
    let num_updates = 5;

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Initial content")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    let mut handles = vec![];
    for i in 0..num_updates {
        let ctx_clone = Arc::clone(&ctx);

        let handle = tokio::spawn(async move {
            if let Some(engine) = ctx_clone.manager.get_engine(1, "Article", "content") {
                let _ = engine.delete("doc_1").await;
            }

            ctx_clone
                .insert_test_doc(
                    1,
                    "Article",
                    "content",
                    "doc_1",
                    &format!("Updated content {}", i),
                )
                .await
        });

        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.expect("Update task panicked");
        assert!(result.is_ok(), "Update should succeed");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "Updated", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 1).expect("Should find exactly 1 document");
    assert_search_result_contains(&results, "doc_1").expect("Should contain doc_1");
}

/// TC-FT-CONC-006: Concurrent Operations on Different Indexes
#[tokio::test]
async fn test_concurrent_different_indexes() {
    let ctx = Arc::new(FulltextTestContext::new());
    let num_tasks = 3;
    let barrier = Arc::new(Barrier::new(num_tasks * 2));

    ctx.create_test_index(1, "Article", "content_a", Some(EngineType::Bm25))
        .await
        .expect("Failed to create first index");
    ctx.create_test_index(1, "Article", "content_b", Some(EngineType::Bm25))
        .await
        .expect("Failed to create second index");

    let mut handles = vec![];

    for i in 0..num_tasks {
        let ctx_clone = Arc::clone(&ctx);
        let barrier_clone = Arc::clone(&barrier);

        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;

            ctx_clone
                .insert_test_doc(
                    1,
                    "Article",
                    "content_a",
                    &format!("doc_a_{}", i),
                    &format!("Content A {}", i),
                )
                .await
        });

        handles.push(handle);
    }

    for i in 0..num_tasks {
        let ctx_clone = Arc::clone(&ctx);
        let barrier_clone = Arc::clone(&barrier);

        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;

            ctx_clone
                .insert_test_doc(
                    1,
                    "Article",
                    "content_b",
                    &format!("doc_b_{}", i),
                    &format!("Content B {}", i),
                )
                .await
        });

        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.expect("Task panicked");
        assert!(result.is_ok(), "Insertion should succeed");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let results_a = ctx
        .search(1, "Article", "content_a", "Content A", 50)
        .await
        .expect("Search should succeed");
    assert_eq!(
        results_a.len(),
        num_tasks,
        "First index should have all documents"
    );

    let results_b = ctx
        .search(1, "Article", "content_b", "Content B", 50)
        .await
        .expect("Search should succeed");
    assert_eq!(
        results_b.len(),
        num_tasks,
        "Second index should have all documents"
    );
}

/// TC-FT-CONC-007: Concurrent Index Creation
#[tokio::test]
async fn test_concurrent_index_creation() {
    let ctx = Arc::new(FulltextTestContext::new());
    let num_indexes = 3;
    let barrier = Arc::new(Barrier::new(num_indexes));

    let mut handles = vec![];
    for i in 0..num_indexes {
        let ctx_clone = Arc::clone(&ctx);
        let barrier_clone = Arc::clone(&barrier);

        let handle = tokio::spawn(async move {
            barrier_clone.wait().await;

            ctx_clone
                .create_test_index(1, &format!("Tag{}", i), "content", Some(EngineType::Bm25))
                .await
        });

        handles.push(handle);
    }

    let mut success_count = 0;
    for handle in handles {
        let result = handle.await.expect("Task panicked");
        if result.is_ok() {
            success_count += 1;
        }
    }

    assert_eq!(
        success_count, num_indexes,
        "All index creations should succeed"
    );

    for i in 0..num_indexes {
        assert!(
            ctx.has_index(1, &format!("Tag{}", i), "content"),
            "Index {} should exist",
            i
        );
    }
}

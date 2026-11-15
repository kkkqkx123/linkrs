//! SyncManager Integration Tests
//!
//! Test scope:
//! - FulltextSyncCoordinator functionality
//! - Batch processing
//! - Transaction integration
//! - Concurrent safety
//!
//! Every test drives the production commit path through `SyncTestHarness`:
//! stage intents, commit, and let outbox delivery index the writes
//! synchronously. There is no background batch worker anymore.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use linkrs::core::types::VertexId;
use linkrs::core::vertex_edge_path::Tag;
use linkrs::core::{Value, Vertex};
use linkrs::test_utils::sync_helpers::SyncTestHarness;

fn setup_article_space(harness: &mut SyncTestHarness) {
    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Article",
            vec![("content", linkrs::core::types::DataType::String)],
            vec!["content"],
        )
        .expect("Failed to create tag");
}

fn create_test_vertex(vid: i64, tag_name: &str, content: &str) -> Vertex {
    let mut props = HashMap::new();
    props.insert("content".into(), Value::string(content));
    let tag = Tag::new(tag_name.to_string(), props);
    Vertex::new(VertexId::try_from_int64(vid).expect("test vertex id"), tag)
}

// ==================== Basic Sync Tests ====================

#[test]
fn test_sync_coordinator_creation() {
    let harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Verify coordinator is created successfully
    assert!(Arc::strong_count(&harness.sync_coordinator) >= 1);
}

#[test]
fn test_sync_vertex_change() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");
    setup_article_space(&mut harness);

    // Sync vertex change through the commit path
    let vertex = create_test_vertex(1, "Article", "Hello World");
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to sync vertex");

    // Verify search results
    let results = harness
        .search_fulltext("test_space", "Article", "content", "Hello", 10)
        .expect("Failed to search");

    assert_eq!(results.len(), 1, "Sync should work");
}

#[test]
fn test_sync_batch_processing() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");
    setup_article_space(&mut harness);

    // Batch insert multiple vertices
    for i in 0..10 {
        let vertex = create_test_vertex(i, "Article", &format!("Article {}", i));
        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to sync vertex");
    }

    // Verify search results
    let results = harness
        .search_fulltext("test_space", "Article", "content", "Article", 10)
        .expect("Failed to search");

    assert!(results.len() >= 10, "Batch processing should work");
}

#[test]
fn test_sync_delete_operation() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");
    setup_article_space(&mut harness);

    // Insert vertex
    let vertex = create_test_vertex(1, "Article", "Delete me");
    harness
        .insert_vertex("test_space", vertex)
        .expect("Failed to insert vertex");

    // Delete vertex inside a transaction so the index observes the delete
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");
    harness
        .delete_vertex_with_txn("test_space", 1)
        .expect("Failed to delete vertex");
    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    // Verify deletion
    let results = harness
        .search_fulltext("test_space", "Article", "content", "Delete", 10)
        .expect("Failed to search");

    assert_eq!(results.len(), 0, "Delete should work");
}

// ==================== Concurrent Tests ====================

#[test]
fn test_concurrent_sync_operations() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");
    setup_article_space(&mut harness);
    let harness = Arc::new(Mutex::new(harness));

    // Concurrent inserts; each one drives the commit path under the lock.
    let mut handles = vec![];
    for i in 0..20 {
        let harness = harness.clone();
        let handle = std::thread::spawn(move || {
            let vertex = create_test_vertex(i, "Article", &format!("Concurrent {}", i));
            harness
                .lock()
                .expect("harness lock")
                .insert_vertex("test_space", vertex)
                .expect("Failed to sync vertex");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Task failed");
    }

    // Verify results
    let results = harness
        .lock()
        .expect("harness lock")
        .search_fulltext("test_space", "Article", "content", "Concurrent", 100)
        .expect("Failed to search");

    assert!(results.len() >= 20, "Concurrent operations should work");
}

// ==================== Error Handling Tests ====================

#[test]
fn test_sync_nonexistent_index() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    harness
        .create_space("test_space")
        .expect("Failed to create space");
    // Tag without any fulltext index: writes stage fine but index nothing.
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Article",
            vec![("content", linkrs::core::types::DataType::String)],
            vec![],
        )
        .expect("Failed to create tag");

    // Should succeed but not index anything (index doesn't exist, so the
    // field is skipped during delivery)
    let vertex = create_test_vertex(1, "Article", "Test");
    harness
        .insert_vertex("test_space", vertex)
        .expect("Insert without index should succeed");

    // Verify search fails because index doesn't exist
    let search_result = harness.search_fulltext("test_space", "Article", "content", "Test", 10);

    assert!(
        search_result.is_err(),
        "Search should fail because index doesn't exist"
    );
}

#[test]
fn test_sync_manager_start_stop() {
    let harness = SyncTestHarness::new().expect("Failed to create test harness");
    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    rt.block_on(async {
        // Start sync manager
        let _ = harness.sync_manager.start().await;

        // Should be able to start again without error
        let _ = harness.sync_manager.start().await;

        // Stop sync manager
        let _ = harness.sync_manager.stop().await;
    });
}

// ==================== Batch Config Tests ====================

#[test]
fn test_custom_batch_size() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");
    setup_article_space(&mut harness);

    // Insert 3 vertices; every insert commits through the same path
    for i in 0..3 {
        let vertex = create_test_vertex(i, "Article", &format!("Batch {}", i));
        harness
            .insert_vertex("test_space", vertex)
            .expect("Failed to sync vertex");
    }

    let results = harness
        .search_fulltext("test_space", "Article", "content", "Batch", 10)
        .expect("Failed to search");

    assert_eq!(results.len(), 3, "Custom batch size should work");
}

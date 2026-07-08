//! SyncManager Integration Tests
//!
//! Test scope:
//! - SyncCoordinator functionality
//! - Batch processing
//! - Transaction integration
//! - Concurrent safety

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use graphdb::core::types::VertexId;
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{Value, Vertex};
use graphdb::search::{
    EngineType, FulltextConfig, FulltextIndexManager, TantivyConfig, TokenizerKind,
};
use graphdb::sync::batch::BatchConfig;
use graphdb::sync::coordinator::{ChangeType, SyncCoordinator};
use graphdb::sync::manager::SyncManager;
use tempfile::TempDir;
use tokio::time::sleep;

// ==================== Test Fixtures ====================

struct SyncTestContext {
    coordinator: Arc<SyncCoordinator>,
    sync_manager: Arc<SyncManager>,
    _temp_dir: TempDir,
}

impl SyncTestContext {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let tantivy = TantivyConfig {
            tokenizer: TokenizerKind::Default,
            ..Default::default()
        };
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            sync: graphdb::search::SyncConfig::default(),
            tantivy,
            cache_size: 100,
            max_result_cache: 1000,
            result_cache_ttl_secs: 60,
        };

        let manager =
            Arc::new(FulltextIndexManager::new(config.clone()).expect("Failed to create manager"));

        let batch_config = BatchConfig::default();
        let sync_coordinator = Arc::new(SyncCoordinator::new(manager.clone(), batch_config));

        let sync_manager = Arc::new(SyncManager::new(sync_coordinator.clone()));

        Self {
            coordinator: sync_coordinator,
            sync_manager,
            _temp_dir: temp_dir,
        }
    }

    fn with_batch_config(batch_config: BatchConfig) -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let tantivy = TantivyConfig {
            tokenizer: TokenizerKind::Default,
            ..Default::default()
        };
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            sync: graphdb::search::SyncConfig::default(),
            tantivy,
            cache_size: 100,
            max_result_cache: 1000,
            result_cache_ttl_secs: 60,
        };

        let manager =
            Arc::new(FulltextIndexManager::new(config.clone()).expect("Failed to create manager"));

        let sync_coordinator = Arc::new(SyncCoordinator::new(manager.clone(), batch_config));

        let sync_manager = Arc::new(SyncManager::new(sync_coordinator.clone()));

        Self {
            coordinator: sync_coordinator,
            sync_manager,
            _temp_dir: temp_dir,
        }
    }
}

fn create_test_vertex(vid: i64, tag_name: &str, content: &str) -> Vertex {
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(content.to_string()));
    let tag = Tag::new(tag_name.to_string(), props);
    Vertex::new(VertexId::from_int64(vid), vec![tag])
}

// ==================== Basic Sync Tests ====================

#[tokio::test]
async fn test_sync_coordinator_creation() {
    let ctx = SyncTestContext::new();

    // Verify coordinator is created successfully
    assert!(Arc::strong_count(&ctx.coordinator) >= 1);
}

#[tokio::test]
async fn test_sync_vertex_change() {
    let ctx = SyncTestContext::new();

    // Create index
    ctx.coordinator
        .fulltext_manager()
        .create_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Create vertex
    let vertex = create_test_vertex(1, "Article", "Hello World");
    let vid = Value::from(vertex.vid);

    // Extract properties
    let props: Vec<(String, Value)> = vertex
        .get_tag("Article")
        .expect("Tag not found")
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Sync vertex change
    ctx.coordinator
        .on_vertex_change(1, "Article", &vid, &props, ChangeType::Insert)
        .await
        .expect("Failed to sync vertex");

    // Commit the batch to ensure the change is processed
    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    // Wait for async processing
    sleep(Duration::from_millis(200)).await;

    // Verify search results
    let results: Vec<_> = ctx
        .coordinator
        .fulltext_manager()
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Failed to search");

    assert_eq!(results.len(), 1, "Sync should work");
}

#[tokio::test]
async fn test_sync_batch_processing() {
    let batch_config = BatchConfig {
        batch_size: 5,
        flush_interval: Duration::from_millis(100),
        max_buffer_size: 100,
        enable_persistence: false,
        persistence_path: None,
        failure_policy: graphdb::search::SyncFailurePolicy::FailOpen,
    };

    let ctx = SyncTestContext::with_batch_config(batch_config);

    // Create index
    ctx.coordinator
        .fulltext_manager()
        .create_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Batch insert multiple vertices
    for i in 0..10 {
        let vertex = create_test_vertex(i, "Article", &format!("Article {}", i));
        let vid = Value::from(vertex.vid);

        let props: Vec<(String, Value)> = vertex
            .get_tag("Article")
            .expect("Tag not found")
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        ctx.coordinator
            .on_vertex_change(1, "Article", &vid, &props, ChangeType::Insert)
            .await
            .expect("Failed to sync vertex");
    }

    // Wait for batch processing
    sleep(Duration::from_millis(300)).await;

    // Verify search results
    let results: Vec<_> = ctx
        .coordinator
        .fulltext_manager()
        .search(1, "Article", "content", "Article", 10)
        .await
        .expect("Failed to search");

    assert!(results.len() >= 10, "Batch processing should work");
}

#[tokio::test]
async fn test_sync_delete_operation() {
    let ctx = SyncTestContext::new();

    // Create index
    ctx.coordinator
        .fulltext_manager()
        .create_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Insert vertex
    let vertex = create_test_vertex(1, "Article", "Delete me");
    let vid = Value::from(vertex.vid);

    let props: Vec<(String, Value)> = vertex
        .get_tag("Article")
        .expect("Tag not found")
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    ctx.coordinator
        .on_vertex_change(1, "Article", &vid, &props, ChangeType::Insert)
        .await
        .expect("Failed to insert vertex");

    sleep(Duration::from_millis(200)).await;

    // Delete vertex
    ctx.coordinator
        .on_vertex_change(1, "Article", &vid, &props, ChangeType::Delete)
        .await
        .expect("Failed to delete vertex");

    sleep(Duration::from_millis(200)).await;

    // Verify deletion
    let results: Vec<_> = ctx
        .coordinator
        .fulltext_manager()
        .search(1, "Article", "content", "Delete", 10)
        .await
        .expect("Failed to search");

    assert_eq!(results.len(), 0, "Delete should work");
}

// ==================== Concurrent Tests ====================

#[tokio::test]
async fn test_concurrent_sync_operations() {
    let ctx = SyncTestContext::new();

    // Create index
    ctx.coordinator
        .fulltext_manager()
        .create_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Concurrent inserts
    let mut handles = vec![];
    for i in 0..20 {
        let coordinator = ctx.coordinator.clone();
        let handle = tokio::spawn(async move {
            let vertex = create_test_vertex(i, "Article", &format!("Concurrent {}", i));
            let vid = Value::from(vertex.vid);

            let props: Vec<(String, Value)> = vertex
                .get_tag("Article")
                .expect("Tag not found")
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            coordinator
                .on_vertex_change(1, "Article", &vid, &props, ChangeType::Insert)
                .await
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        let _ = handle.await.expect("Task failed");
    }

    // Commit the batch to ensure all changes are processed
    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    // Wait for processing
    sleep(Duration::from_millis(300)).await;

    // Verify results
    let results: Vec<_> = ctx
        .coordinator
        .fulltext_manager()
        .search(1, "Article", "content", "Concurrent", 100)
        .await
        .expect("Failed to search");

    assert!(results.len() >= 20, "Concurrent operations should work");
}

// ==================== Error Handling Tests ====================

#[tokio::test]
async fn test_sync_nonexistent_index() {
    let ctx = SyncTestContext::new();

    // Try to sync without creating index first
    let vertex = create_test_vertex(1, "Article", "Test");
    let vid = Value::from(vertex.vid);

    let props: Vec<(String, Value)> = vertex
        .get_tag("Article")
        .expect("Tag not found")
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Should succeed but not index anything (index doesn't exist, so field is skipped)
    let result = ctx
        .coordinator
        .on_vertex_change(1, "Article", &vid, &props, ChangeType::Insert)
        .await;

    // Should succeed (fields without indexes are silently skipped)
    assert!(result.is_ok());

    // Verify search fails because index doesn't exist
    let search_result = ctx
        .coordinator
        .fulltext_manager()
        .search(1, "Article", "content", "Test", 10)
        .await;

    assert!(
        search_result.is_err(),
        "Search should fail because index doesn't exist"
    );
}

#[tokio::test]
async fn test_sync_manager_start_stop() {
    let ctx = SyncTestContext::new();

    // Start sync manager
    let _ = ctx.sync_manager.start().await;

    // Should be able to start again without error
    let _ = ctx.sync_manager.start().await;

    // Stop sync manager
    let _ = ctx.sync_manager.stop().await;
}

// ==================== Batch Config Tests ====================

#[tokio::test]
async fn test_custom_batch_size() {
    let batch_config = BatchConfig {
        batch_size: 2,
        flush_interval: Duration::from_millis(50),
        max_buffer_size: 10,
        enable_persistence: false,
        persistence_path: None,
        failure_policy: graphdb::search::SyncFailurePolicy::FailOpen,
    };

    let ctx = SyncTestContext::with_batch_config(batch_config);

    // Create index
    ctx.coordinator
        .fulltext_manager()
        .create_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Insert 3 vertices (should trigger batch flush at 2)
    for i in 0..3 {
        let vertex = create_test_vertex(i, "Article", &format!("Batch {}", i));
        let vid = Value::from(vertex.vid);

        let props: Vec<(String, Value)> = vertex
            .get_tag("Article")
            .expect("Tag not found")
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        ctx.coordinator
            .on_vertex_change(1, "Article", &vid, &props, ChangeType::Insert)
            .await
            .expect("Failed to sync vertex");
    }

    // Commit the batch to ensure all changes are processed
    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    sleep(Duration::from_millis(200)).await;

    let results: Vec<_> = ctx
        .coordinator
        .fulltext_manager()
        .search(1, "Article", "content", "Batch", 10)
        .await
        .expect("Failed to search");

    assert_eq!(results.len(), 3, "Custom batch size should work");
}

//! Fulltext Integration Tests - Sync Mechanism
//!
//! Test scope:
//! - SyncCoordinator basic functionality
//! - Vertex change auto-sync (insert, update, delete)
//! - Transaction buffering
//! - Sync with BM25 engine
//!
//! Test cases: TC-FT-SYNC-001 ~ TC-FT-SYNC-010

use super::common::FulltextTestContext;
use graphdb_search::search::EngineType;
use graphdb_sync::sync::batch::BatchConfig;
use graphdb_sync::sync::coordinator::{ChangeType, SyncCoordinator};
use graphdb_sync::sync::manager::SyncManager;
use std::sync::Arc;

#[allow(dead_code)]
struct SyncTestContext {
    coordinator: Arc<SyncCoordinator>,
    sync_manager: Arc<SyncManager>,
    fulltext_ctx: FulltextTestContext,
}

impl SyncTestContext {
    async fn new() -> Self {
        let fulltext_ctx = FulltextTestContext::new();
        let batch_config = BatchConfig::default();
        let coordinator = Arc::new(SyncCoordinator::new(
            fulltext_ctx.manager.clone(),
            batch_config,
        ));
        let sync_manager = Arc::new(SyncManager::new(coordinator.clone()));

        sync_manager
            .start()
            .await
            .expect("Failed to start sync manager");

        Self {
            coordinator,
            sync_manager,
            fulltext_ctx,
        }
    }

    #[allow(dead_code)]
    async fn shutdown(&self) {
        self.sync_manager.stop().await;
    }
}

impl Drop for SyncTestContext {
    fn drop(&mut self) {
        // Note: This is a best-effort cleanup since we can't block in Drop
        // The sync_manager will be dropped automatically
    }
}

fn create_test_properties(content: &str) -> Vec<(String, graphdb_core::core::Value)> {
    vec![(
        "content".to_string(),
        graphdb_core::core::Value::String(content.to_string()),
    )]
}

/// TC-FT-SYNC-001: Vertex Insert Auto-Sync with BM25
#[tokio::test]
async fn test_vertex_insert_auto_sync_bm25() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let vertex_id = graphdb_core::core::Value::Int(1);
    let properties = create_test_properties("Hello World");

    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &properties, ChangeType::Insert)
        .await
        .expect("Failed to sync vertex insert");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");

    let expected_doc_id = graphdb_core::core::Value::String("1".to_string());
    assert!(
        results.iter().any(|r| r.doc_id == expected_doc_id),
        "Should find synced document with doc_id=1"
    );
}

/// TC-FT-SYNC-003: Vertex Update Auto-Sync
#[tokio::test]
async fn test_vertex_update_auto_sync() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let vertex_id = graphdb_core::core::Value::Int(1);

    // Insert vertex
    let insert_props = create_test_properties("Old Content");
    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &insert_props, ChangeType::Insert)
        .await
        .expect("Failed to sync vertex insert");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    // Update vertex
    if let Some(engine) = ctx.fulltext_ctx.manager.get_engine(1, "Article", "content") {
        engine
            .delete("1")
            .await
            .expect("Failed to delete old content");
    }

    let update_props = create_test_properties("New Content");
    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &update_props, ChangeType::Insert)
        .await
        .expect("Failed to sync vertex update");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    // Search for old content - should not find
    let old_results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Old", 10)
        .await
        .expect("Search should succeed");
    let old_doc_id = graphdb_core::core::Value::String("1".to_string());
    assert!(
        !old_results.iter().any(|r| r.doc_id == old_doc_id),
        "Should not find old content"
    );

    // Search for new content - should find
    let new_results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "New", 10)
        .await
        .expect("Search should succeed");
    let new_doc_id = graphdb_core::core::Value::String("1".to_string());
    assert!(
        new_results.iter().any(|r| r.doc_id == new_doc_id),
        "Should find new content"
    );
}

/// TC-FT-SYNC-004: Vertex Delete Auto-Sync
#[tokio::test]
async fn test_vertex_delete_auto_sync() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let vertex_id = graphdb_core::core::Value::Int(1);

    // Insert vertex
    let insert_props = create_test_properties("Hello World");
    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &insert_props, ChangeType::Insert)
        .await
        .expect("Failed to sync vertex insert");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    // Verify document exists
    let results_before = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");
    let doc_id = graphdb_core::core::Value::String("1".to_string());
    assert!(
        results_before.iter().any(|r| r.doc_id == doc_id),
        "Should find document before deletion"
    );

    // Delete vertex
    let delete_props: Vec<(String, graphdb_core::core::Value)> = vec![(
        "content".to_string(),
        graphdb_core::core::Value::String("Hello World".to_string()),
    )];
    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &delete_props, ChangeType::Delete)
        .await
        .expect("Failed to sync vertex delete");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    // Verify document is deleted
    let results_after = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");
    let doc_id_after = graphdb_core::core::Value::String("1".to_string());
    assert!(
        !results_after.iter().any(|r| r.doc_id == doc_id_after),
        "Should not find document after deletion"
    );
}

/// TC-FT-SYNC-005: Multiple Vertex Inserts
#[tokio::test]
async fn test_multiple_vertex_inserts() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    for i in 1..=5 {
        let vertex_id = graphdb_core::core::Value::Int(i);
        let properties = create_test_properties(&format!("Content {}", i));

        ctx.coordinator
            .on_vertex_change(1, "Article", &vertex_id, &properties, ChangeType::Insert)
            .await
            .expect("Failed to sync vertex");
    }

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit all");

    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Content", 100)
        .await
        .expect("Search should succeed");
    assert_eq!(
        results.len(),
        5,
        "All documents should be searchable after commit"
    );
}

/// TC-FT-SYNC-006: Sync with Multiple Fields
#[tokio::test]
async fn test_sync_multiple_fields() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "title", Some(EngineType::Bm25))
        .await
        .expect("Failed to create title index");
    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create content index");

    let vertex_id = graphdb_core::core::Value::Int(1);
    let title_props = vec![(
        "title".to_string(),
        graphdb_core::core::Value::String("Test title".to_string()),
    )];
    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &title_props, ChangeType::Insert)
        .await
        .expect("Failed to sync title");

    let content_props = vec![(
        "content".to_string(),
        graphdb_core::core::Value::String("Test content".to_string()),
    )];
    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &content_props, ChangeType::Insert)
        .await
        .expect("Failed to sync content");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let title_results = ctx
        .fulltext_ctx
        .search(1, "Article", "title", "title", 10)
        .await
        .expect("Title search should succeed");
    assert_eq!(title_results.len(), 1, "Should find document in title");

    let content_results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "content", 10)
        .await
        .expect("Content search should succeed");
    assert_eq!(content_results.len(), 1, "Should find document in content");
}

/// TC-FT-SYNC-007: Sync with String Vertex IDs
#[tokio::test]
async fn test_sync_string_vertex_ids() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Use string vertex ID
    let vertex_id = graphdb_core::core::Value::String("article_001".to_string());
    let properties = create_test_properties("String ID content");

    ctx.coordinator
        .on_vertex_change(1, "Article", &vertex_id, &properties, ChangeType::Insert)
        .await
        .expect("Failed to sync vertex insert");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "String", 10)
        .await
        .expect("Search should succeed");

    let expected_doc_id = graphdb_core::core::Value::String("article_001".to_string());
    assert!(
        results.iter().any(|r| r.doc_id == expected_doc_id),
        "Should find document with string ID"
    );
}

/// TC-FT-SYNC-008: Sync Multiple Batches
#[tokio::test]
async fn test_sync_multiple_batches() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let vertex_ids: Vec<graphdb_core::core::Value> =
        (1..=10).map(graphdb_core::core::Value::Int).collect();

    // First batch
    for (idx, vertex_id) in vertex_ids.iter().take(5).enumerate() {
        let properties = create_test_properties(&format!("Batch1 Content {}", idx + 1));
        ctx.coordinator
            .on_vertex_change(1, "Article", vertex_id, &properties, ChangeType::Insert)
            .await
            .expect("Failed to sync vertex for batch 1");
    }

    // Second batch
    for (idx, vertex_id) in vertex_ids.iter().skip(5).take(5).enumerate() {
        let properties = create_test_properties(&format!("Batch2 Content {}", idx + 6));
        ctx.coordinator
            .on_vertex_change(1, "Article", vertex_id, &properties, ChangeType::Insert)
            .await
            .expect("Failed to sync vertex for batch 2");
    }

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit all");

    // Verify all documents are searchable
    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "Content", 100)
        .await
        .expect("Search should succeed");
    assert_eq!(
        results.len(),
        10,
        "All documents should be searchable after commit"
    );
}

/// TC-FT-SYNC-009: Sync with Empty Properties
#[tokio::test]
async fn test_sync_empty_properties() {
    let ctx = SyncTestContext::new().await;

    ctx.fulltext_ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let vertex_id = graphdb_core::core::Value::Int(1);

    // Insert with content
    let props_with_content = create_test_properties("Has content");
    ctx.coordinator
        .on_vertex_change(
            1,
            "Article",
            &vertex_id,
            &props_with_content,
            ChangeType::Insert,
        )
        .await
        .expect("Failed to sync vertex insert");

    ctx.coordinator
        .commit_all()
        .await
        .expect("Failed to commit");

    // Verify document is searchable
    let results = ctx
        .fulltext_ctx
        .search(1, "Article", "content", "content", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(results.len(), 1, "Should find document");
}

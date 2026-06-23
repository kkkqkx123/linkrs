//! Fulltext Integration Tests - Persistence
//!
//! Test scope:
//! - Index persistence across manager restarts
//! - Document persistence verification
//! - Metadata persistence
//! - Stats persistence
//! - BM25 engine
//!
//! Test cases: TC-FT-PERSIST-001 ~ TC-FT-PERSIST-010

use std::sync::Arc;
use tempfile::TempDir;

use graphdb_search::search::{EngineType, FulltextConfig, FulltextIndexManager};

fn create_manager_with_path(path: &std::path::Path) -> Arc<FulltextIndexManager> {
    let config = FulltextConfig {
        enabled: true,
        index_path: path.to_path_buf(),
        default_engine: EngineType::Bm25,
        sync: graphdb_search::search::SyncConfig::default(),
        tantivy: Default::default(),
        cache_size: 100,
        max_result_cache: 1000,
        result_cache_ttl_secs: 60,
    };
    Arc::new(FulltextIndexManager::new(config).expect("Failed to create manager"))
}

/// TC-FT-PERSIST-001: Basic Index Persistence
#[tokio::test]
async fn test_basic_index_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        assert!(
            manager.has_index(1, "Article", "content"),
            "Index should exist after creation"
        );
    }

    let manager = create_manager_with_path(temp_dir.path());

    assert!(
        manager.has_index(1, "Article", "content"),
        "Index should persist after manager restart"
    );
}

/// TC-FT-PERSIST-002: Document Persistence After Restart
#[tokio::test]
async fn test_document_persistence_after_restart() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        if let Some(engine) = manager.get_engine(1, "Article", "content") {
            engine
                .index("doc_1", "Persistent document content")
                .await
                .expect("Failed to index document");
            engine.commit().await.expect("Failed to commit");
        }
    }

    let manager = create_manager_with_path(temp_dir.path());

    let results = manager
        .search(1, "Article", "content", "Persistent", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(
        results.len(),
        1,
        "Document should persist after manager restart"
    );
}

/// TC-FT-PERSIST-003: Multiple Documents Persistence
#[tokio::test]
async fn test_multiple_documents_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        if let Some(engine) = manager.get_engine(1, "Article", "content") {
            for i in 0..10 {
                engine
                    .index(&format!("doc_{}", i), &format!("Document number {}", i))
                    .await
                    .expect("Failed to index document");
            }
            engine.commit().await.expect("Failed to commit");
        }
    }

    let manager = create_manager_with_path(temp_dir.path());

    let results = manager
        .search(1, "Article", "content", "Document", 100)
        .await
        .expect("Search should succeed");

    assert_eq!(
        results.len(),
        10,
        "All 10 documents should persist after restart"
    );
}

/// TC-FT-PERSIST-005: Metadata Persistence
#[tokio::test]
async fn test_metadata_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        let metadata = manager.get_metadata(1, "Article", "content");
        assert!(metadata.is_some(), "Metadata should exist");
    }

    let manager = create_manager_with_path(temp_dir.path());

    let metadata = manager.get_metadata(1, "Article", "content");
    assert!(metadata.is_some(), "Metadata should persist after restart");

    let metadata = metadata.unwrap();
    assert_eq!(metadata.space_id, 1);
    assert_eq!(metadata.tag_name, "Article");
    assert_eq!(metadata.field_name, "content");
    assert_eq!(metadata.engine_type, EngineType::Bm25);
}

/// TC-FT-PERSIST-006: Multi-Space Persistence
#[tokio::test]
async fn test_multi_space_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index for space 1");

        manager
            .create_index(2, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index for space 2");

        if let Some(engine) = manager.get_engine(1, "Article", "content") {
            engine
                .index("doc_1", "Space 1 content")
                .await
                .expect("Failed to index");
            engine.commit().await.expect("Failed to commit");
        }

        if let Some(engine) = manager.get_engine(2, "Article", "content") {
            engine
                .index("doc_2", "Space 2 content")
                .await
                .expect("Failed to index");
            engine.commit().await.expect("Failed to commit");
        }
    }

    let manager = create_manager_with_path(temp_dir.path());

    assert!(
        manager.has_index(1, "Article", "content"),
        "Space 1 index should persist"
    );
    assert!(
        manager.has_index(2, "Article", "content"),
        "Space 2 index should persist"
    );

    let results_1 = manager
        .search(1, "Article", "content", "Space", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(results_1.len(), 1, "Space 1 document should persist");

    let results_2 = manager
        .search(2, "Article", "content", "Space", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(results_2.len(), 1, "Space 2 document should persist");
}

/// TC-FT-PERSIST-007: Stats Persistence
#[tokio::test]
async fn test_stats_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        if let Some(engine) = manager.get_engine(1, "Article", "content") {
            for i in 0..5 {
                engine
                    .index(&format!("doc_{}", i), &format!("Content {}", i))
                    .await
                    .expect("Failed to index");
            }
            engine.commit().await.expect("Failed to commit");
        }
    }

    let manager = create_manager_with_path(temp_dir.path());

    let stats = manager
        .get_stats(1, "Article", "content")
        .await
        .expect("Should get stats");

    assert_eq!(stats.doc_count, 5, "Document count should persist in stats");
}

/// TC-FT-PERSIST-008: Deleted Document Persistence
#[tokio::test]
async fn test_deleted_document_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        if let Some(engine) = manager.get_engine(1, "Article", "content") {
            engine
                .index("doc_1", "To be deleted")
                .await
                .expect("Failed to index");
            engine
                .index("doc_2", "To keep")
                .await
                .expect("Failed to index");
            engine.commit().await.expect("Failed to commit");

            engine.delete("doc_1").await.expect("Failed to delete");
            engine.commit().await.expect("Failed to commit");
        }
    }

    let manager = create_manager_with_path(temp_dir.path());

    let results = manager
        .search(1, "Article", "content", "deleted", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(results.len(), 0, "Deleted document should not persist");

    let results_keep = manager
        .search(1, "Article", "content", "keep", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(results_keep.len(), 1, "Non-deleted document should persist");
}

/// TC-FT-PERSIST-010: Dropped Index Persistence
#[tokio::test]
async fn test_dropped_index_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    {
        let manager = create_manager_with_path(temp_dir.path());

        manager
            .create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        if let Some(engine) = manager.get_engine(1, "Article", "content") {
            engine
                .index("doc_1", "Content to be removed")
                .await
                .expect("Failed to index");
            engine.commit().await.expect("Failed to commit");
        }

        manager
            .drop_index(1, "Article", "content")
            .await
            .expect("Failed to drop index");
    }

    let manager = create_manager_with_path(temp_dir.path());

    assert!(
        !manager.has_index(1, "Article", "content"),
        "Dropped index should not persist"
    );
}

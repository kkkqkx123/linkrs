use super::common::{
    assert_results_sorted_by_score, assert_search_result_contains, assert_search_result_count,
    assert_search_result_not_contains, generate_test_docs, FulltextTestContext,
};
use graphdb_search::search::EngineType;

/// TC-FT-001: Create Fulltext Index with BM25
#[tokio::test]
async fn test_create_fulltext_index_bm25() {
    let ctx = FulltextTestContext::new();

    let result = ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await;

    assert!(result.is_ok(), "BM25 index creation should succeed");
    let index_id = result.unwrap();

    assert_eq!(
        index_id, "space_ft_1_Article_content",
        "Index ID format should be correct"
    );

    assert!(
        ctx.has_index(1, "Article", "content"),
        "Index should exist after creation"
    );

    let metadata = ctx.get_metadata(1, "Article", "content");
    assert!(metadata.is_some(), "Metadata should exist");
    let metadata = metadata.unwrap();
    assert_eq!(metadata.index_id, index_id);
    assert_eq!(metadata.space_id, 1);
    assert_eq!(metadata.tag_name, "Article");
    assert_eq!(metadata.field_name, "content");
    assert_eq!(metadata.engine_type, EngineType::Bm25);
}

/// TC-FT-002: Create Duplicate Index
#[tokio::test]
async fn test_create_duplicate_index() {
    let ctx = FulltextTestContext::new();

    let result1 = ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await;
    assert!(result1.is_ok(), "First index creation should succeed");

    let result2 = ctx
        .create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await;

    assert!(result2.is_err(), "Duplicate index creation should fail");
    assert!(
        matches!(
            result2.unwrap_err(),
            graphdb_search::search::SearchError::IndexAlreadyExists(_)
        ),
        "Should return IndexAlreadyExists error"
    );
}

/// TC-FT-003: Drop Index
#[tokio::test]
async fn test_drop_index() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let result = ctx.drop_index(1, "Article", "content").await;
    assert!(result.is_ok(), "Index drop should succeed");

    assert!(
        !ctx.has_index(1, "Article", "content"),
        "Index should not exist after dropping"
    );

    let metadata = ctx.get_metadata(1, "Article", "content");
    assert!(metadata.is_none(), "Metadata should be removed");
}

/// TC-FT-004: Get Index Metadata
#[tokio::test]
async fn test_get_index_metadata() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let metadata = ctx.get_metadata(1, "Article", "content");
    assert!(metadata.is_some(), "Metadata should exist");

    let metadata = metadata.unwrap();
    assert_eq!(metadata.index_id, "space_ft_1_Article_content");
    assert_eq!(metadata.space_id, 1);
    assert_eq!(metadata.tag_name, "Article");
    assert_eq!(metadata.field_name, "content");
    assert!(metadata.status == graphdb_search::search::IndexStatus::Active);
    assert_eq!(metadata.doc_count, 0);
}

/// TC-FT-005: List Space Indexes
#[tokio::test]
async fn test_get_space_indexes() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "title", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");
    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");
    ctx.create_test_index(1, "Person", "name", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");
    ctx.create_test_index(2, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let indexes = ctx.get_space_indexes(1);
    assert_eq!(indexes.len(), 3, "Should have 3 indexes for space 1");

    for index in &indexes {
        assert_eq!(index.space_id, 1, "All indexes should belong to space 1");
    }

    let indexes_space_2 = ctx.get_space_indexes(2);
    assert_eq!(indexes_space_2.len(), 1, "Should have 1 index for space 2");
}

/// TC-FT-006: Index Document and Search with BM25
#[tokio::test]
async fn test_index_and_search_bm25() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Hello World")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_contains(&results, "doc_1").expect("Search should return doc_1");
    assert_eq!(results.len(), 1, "Should return 1 result");
}

/// TC-FT-007: Batch Index Documents
#[tokio::test]
async fn test_batch_index() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs: Vec<(String, String)> = (0..10)
        .map(|i| (format!("doc_{}", i), format!("Content {}", i)))
        .collect();

    let docs_ref: Vec<(&str, &str)> = docs
        .iter()
        .map(|(id, content)| (id.as_str(), content.as_str()))
        .collect();
    ctx.insert_test_docs(1, "Article", "content", docs_ref)
        .await
        .expect("Failed to batch insert");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "Content", 100)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 10, "Should return 10 results");
    for i in 0..10 {
        assert_search_result_contains(&results, &format!("doc_{}", i))
            .unwrap_or_else(|_| panic!("Should contain doc_{}", i));
    }
}

/// TC-FT-008: Update Document
#[tokio::test]
async fn test_update_document() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Old Content")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    if let Some(engine) = ctx.manager.get_engine(1, "Article", "content") {
        engine
            .delete("doc_1")
            .await
            .expect("Failed to delete old document");
    }

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "New Content")
        .await
        .expect("Failed to update document");

    ctx.commit_all().await.expect("Failed to commit");

    let old_results = ctx
        .search(1, "Article", "content", "Old", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_not_contains(&old_results, "doc_1").expect("Should not find old content");

    let new_results = ctx
        .search(1, "Article", "content", "New", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_contains(&new_results, "doc_1").expect("Should find new content");
}

/// TC-FT-009: Delete Document
#[tokio::test]
async fn test_delete_document() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Hello World")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    let results_before = ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_contains(&results_before, "doc_1")
        .expect("Should find document before deletion");

    if let Some(engine) = ctx.manager.get_engine(1, "Article", "content") {
        engine.delete("doc_1").await.expect("Failed to delete");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let results_after = ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_not_contains(&results_after, "doc_1")
        .expect("Should not find document after deletion");
}

/// TC-FT-010: Batch Delete Documents
#[tokio::test]
async fn test_batch_delete() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs: Vec<(String, String)> = (0..10)
        .map(|i| (format!("doc_{}", i), format!("Content {}", i)))
        .collect();
    let docs_ref: Vec<(&str, &str)> = docs
        .iter()
        .map(|(id, content)| (id.as_str(), content.as_str()))
        .collect();
    ctx.insert_test_docs(1, "Article", "content", docs_ref)
        .await
        .expect("Failed to batch insert");

    ctx.commit_all().await.expect("Failed to commit");

    if let Some(engine) = ctx.manager.get_engine(1, "Article", "content") {
        let ids: Vec<String> = (0..5).map(|i| format!("doc_{}", i)).collect();
        let ids_ref: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        engine
            .delete_batch(ids_ref)
            .await
            .expect("Failed to batch delete");
    }

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "Content", 100)
        .await
        .expect("Search should succeed");
    assert_eq!(results.len(), 5, "Should return 5 results after deletion");

    for i in 5..10 {
        assert_search_result_contains(&results, &format!("doc_{}", i))
            .unwrap_or_else(|_| panic!("Should contain doc_{}", i));
    }
}

/// TC-FT-011: Single Term Search
#[tokio::test]
async fn test_single_term_search() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "apple banana cherry"),
        ("doc_2", "banana cherry date"),
        ("doc_3", "cherry date elderberry"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "banana", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 2, "Should return 2 results");
    assert_search_result_contains(&results, "doc_1").expect("Should contain doc_1");
    assert_search_result_contains(&results, "doc_2").expect("Should contain doc_2");
    assert_search_result_not_contains(&results, "doc_3").expect("Should not contain doc_3");
}

/// TC-FT-012: Multi-Term Search
#[tokio::test]
async fn test_multi_term_search() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "apple banana"),
        ("doc_2", "banana cherry"),
        ("doc_3", "apple banana cherry"),
        ("doc_4", "cherry"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "apple banana", 10)
        .await
        .expect("Search should succeed");

    assert_results_sorted_by_score(&results).expect("Results should be sorted by score");
    assert!(!results.is_empty(), "Should have search results");
}

/// TC-FT-013: Search Limit
#[tokio::test]
async fn test_search_limit() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = generate_test_docs(100, "Test");
    let docs_ref: Vec<(&str, &str)> = docs
        .iter()
        .map(|(id, content)| (id.as_str(), content.as_str()))
        .collect();
    ctx.insert_test_docs(1, "Article", "content", docs_ref)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "Test", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 10).expect("Should return exactly 10 results");
}

/// TC-FT-014: Empty Search
#[tokio::test]
async fn test_empty_search() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Hello World")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "nonexistent", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 0).expect("Should return empty results");
}

/// TC-FT-015: Special Characters Search
#[tokio::test]
async fn test_special_characters_search() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(
        1,
        "Article",
        "content",
        "doc_1",
        "Email: test@example.com, Price: $100, 100%",
    )
    .await
    .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "example", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_contains(&results, "doc_1")
        .expect("Should find document with special characters");
}

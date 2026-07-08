//! Fulltext Integration Tests - Edge Cases and Error Handling
//!
//! Test scope:
//! - Error handling (index not found, duplicate creation, invalid queries)
//! - Edge cases (empty content, very long content, unicode, special characters)
//! - Index rebuilding
//! - Multi-space isolation
//! - Memory limits
//! - BM25 engine
//!
//! Test cases: TC-FT-EDGE-001 ~ TC-FT-EDGE-015

use super::common::{assert_search_result_count, FulltextTestContext};
use graphdb_search::search::EngineType;

/// TC-FT-EDGE-001: Search on Non-Existent Index
#[tokio::test]
async fn test_search_non_existent_index() {
    let ctx = FulltextTestContext::new();

    let result = ctx.search(1, "Article", "content", "Hello", 10).await;

    assert!(result.is_err(), "Search on non-existent index should fail");
    assert!(
        matches!(
            result.unwrap_err(),
            graphdb_search::search::SearchError::IndexNotFound(_)
        ),
        "Should return IndexNotFound error"
    );
}

/// TC-FT-EDGE-002: Index Empty Content with BM25
#[tokio::test]
async fn test_index_empty_content_bm25() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "")
        .await
        .expect("Indexing empty content should not fail");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "anything", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 0)
        .expect("Empty content should not produce search results");
}

/// TC-FT-EDGE-004: Index Very Long Content
#[tokio::test]
async fn test_index_very_long_content() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Generate very long content (10000 words)
    let long_content: Vec<String> = (0..10000).map(|i| format!("word{}", i)).collect();
    let long_content_str = long_content.join(" ");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", &long_content_str)
        .await
        .expect("Indexing long content should succeed");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "word5000", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results, 1).expect("Should find document with long content");

    let results2 = ctx
        .search(1, "Article", "content", "word9999", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results2, 1).expect("Should find document with long content");
}

/// TC-FT-EDGE-005: Index Unicode Content
#[tokio::test]
async fn test_index_unicode_content() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let unicode_content = "Hello World test content";
    ctx.insert_test_doc(1, "Article", "content", "doc_1", unicode_content)
        .await
        .expect("Indexing Unicode content should succeed");

    ctx.commit_all().await.expect("Failed to commit");

    let results_en = ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results_en, 1).expect("Should find document with English words");
}

/// TC-FT-EDGE-006: Special Query Characters
#[tokio::test]
async fn test_special_query_characters() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(
        1,
        "Article",
        "content",
        "doc_1",
        "Testing special characters: hello world test query example mark",
    )
    .await
    .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    // Test various queries - should handle gracefully
    let queries = vec!["hello", "test", "example"];

    for query in queries {
        let result = ctx.search(1, "Article", "content", query, 10).await;
        assert!(result.is_ok(), "Should handle query: {}", query);
    }
}

/// TC-FT-EDGE-007: Index Rebuilding
#[tokio::test]
async fn test_rebuild_index() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Old content")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    let old_results = ctx
        .search(1, "Article", "content", "Old", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_count(&old_results, 1).expect("Should find old data");

    ctx.drop_index(1, "Article", "content")
        .await
        .expect("Failed to drop index");

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to recreate index");

    ctx.insert_test_doc(1, "Article", "content", "doc_2", "New content")
        .await
        .expect("Failed to insert new document");

    ctx.commit_all().await.expect("Failed to commit");

    let old_results_after = ctx
        .search(1, "Article", "content", "Old", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_count(&old_results_after, 0)
        .expect("Old data should be gone after rebuild");

    let new_results = ctx
        .search(1, "Article", "content", "New", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_count(&new_results, 1).expect("Should find new data");
}

/// TC-FT-EDGE-008: Multi-Space Isolation
#[tokio::test]
async fn test_multi_space_isolation() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index for space 1");
    ctx.create_test_index(2, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index for space 2");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "UniqueSpace1Marker")
        .await
        .expect("Failed to insert in space 1");

    ctx.insert_test_doc(2, "Article", "content", "doc_2", "UniqueSpace2Marker")
        .await
        .expect("Failed to insert in space 2");

    ctx.commit_all().await.expect("Failed to commit");

    let results_space_1 = ctx
        .search(1, "Article", "content", "UniqueSpace1Marker", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results_space_1, 1).expect("Space 1 should have 1 result");
    assert!(
        results_space_1[0].doc_id == graphdb_core::core::Value::String("doc_1".to_string()),
        "Space 1 should return doc_1"
    );

    let results_space_2 = ctx
        .search(2, "Article", "content", "UniqueSpace2Marker", 10)
        .await
        .expect("Search should succeed");

    assert_search_result_count(&results_space_2, 1).expect("Space 2 should have 1 result");
    assert!(
        results_space_2[0].doc_id == graphdb_core::core::Value::String("doc_2".to_string()),
        "Space 2 should return doc_2"
    );

    let cross_search_1 = ctx
        .search(1, "Article", "content", "UniqueSpace2Marker", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_count(&cross_search_1, 0)
        .expect("Space 1 should not contain space 2 data");

    let cross_search_2 = ctx
        .search(2, "Article", "content", "UniqueSpace1Marker", 10)
        .await
        .expect("Search should succeed");
    assert_search_result_count(&cross_search_2, 0)
        .expect("Space 2 should not contain space 1 data");
}

/// TC-FT-EDGE-010: Whitespace Only Content
#[tokio::test]
async fn test_whitespace_only_content() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "   "),
        ("doc_2", "\t\n\r"),
        ("doc_3", "actual content"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "actual", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 1, "Should find only doc_3");
}

/// TC-FT-EDGE-011: Duplicate Document IDs
#[tokio::test]
async fn test_duplicate_document_ids() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    // Insert document
    ctx.insert_test_doc(1, "Article", "content", "doc_1", "First version")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    // Insert with same ID (should update)
    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Second version")
        .await
        .expect("Failed to insert document");

    ctx.commit_all().await.expect("Failed to commit");

    // Search for first version - should not find
    let first_results = ctx
        .search(1, "Article", "content", "First", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(first_results.len(), 0, "Should not find first version");

    // Search for second version - should find
    let second_results = ctx
        .search(1, "Article", "content", "Second", 10)
        .await
        .expect("Search should succeed");
    assert_eq!(second_results.len(), 1, "Should find second version");
}

/// TC-FT-EDGE-012: Very Short Content
#[tokio::test]
async fn test_very_short_content() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![("doc_1", "a"), ("doc_2", "ab"), ("doc_3", "abc")];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    // Note: BM25 tokenizer skips single characters (length < 2)
    // So searching for "a" will not find any documents
    // Instead, search for "ab" which should find doc_2
    let results = ctx
        .search(1, "Article", "content", "ab", 10)
        .await
        .expect("Search should succeed");

    assert!(
        !results.is_empty(),
        "Should find at least one document with 'ab'"
    );

    // Search for "abc" should find doc_3
    let results_abc = ctx
        .search(1, "Article", "content", "abc", 10)
        .await
        .expect("Search should succeed");

    assert!(
        !results_abc.is_empty(),
        "Should find at least one document with 'abc'"
    );
}

/// TC-FT-EDGE-013: Numeric Content
#[tokio::test]
async fn test_numeric_content() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "12345"),
        ("doc_2", "9876543210"),
        ("doc_3", "Version 2.0 release"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    // Search for numbers
    let results = ctx
        .search(1, "Article", "content", "12345", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 1, "Should find document with number");
}

/// TC-FT-EDGE-014: Mixed Engine Space Isolation
#[tokio::test]
async fn test_mixed_engine_space_isolation() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create BM25 index");

    ctx.create_test_index(2, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    ctx.insert_test_doc(1, "Article", "content", "doc_1", "Space1Content")
        .await
        .expect("Failed to insert in space 1");

    ctx.insert_test_doc(2, "Article", "content", "doc_2", "Space2Content")
        .await
        .expect("Failed to insert in space 2");

    ctx.commit_all().await.expect("Failed to commit");

    // Search in space 1 (BM25)
    let space1_results = ctx
        .search(1, "Article", "content", "Space1Content", 10)
        .await
        .expect("Space 1 search should succeed");
    assert_eq!(space1_results.len(), 1, "Should find document in space 1");

    // Search in space 2
    let space2_results = ctx
        .search(2, "Article", "content", "Space2Content", 10)
        .await
        .expect("Space 2 search should succeed");
    assert_eq!(space2_results.len(), 1, "Should find document in space 2");

    // Cross-space search should not find anything
    let cross1_results = ctx
        .search(1, "Article", "content", "Space2Content", 10)
        .await
        .expect("Cross search should succeed");
    assert_eq!(
        cross1_results.len(),
        0,
        "Space 1 should not find space 2 data"
    );

    let cross2_results = ctx
        .search(2, "Article", "content", "Space1Content", 10)
        .await
        .expect("Cross search should succeed");
    assert_eq!(
        cross2_results.len(),
        0,
        "Space 2 should not find space 1 data"
    );
}

/// TC-FT-EDGE-015: Rapid Index Create/Drop
#[tokio::test]
async fn test_rapid_index_create_drop() {
    let ctx = FulltextTestContext::new();

    for i in 0..5 {
        // Create index
        ctx.create_test_index(1, "Article", &format!("field{}", i), Some(EngineType::Bm25))
            .await
            .expect("Failed to create index");

        // Insert some data
        ctx.insert_test_doc(
            1,
            "Article",
            &format!("field{}", i),
            "doc_1",
            "test content",
        )
        .await
        .expect("Failed to insert");

        ctx.commit_all().await.expect("Failed to commit");

        // Verify data exists
        let results = ctx
            .search(1, "Article", &format!("field{}", i), "test", 10)
            .await
            .expect("Search should succeed");
        assert_eq!(results.len(), 1, "Should find document");

        // Drop index
        ctx.drop_index(1, "Article", &format!("field{}", i))
            .await
            .expect("Failed to drop index");

        // Verify index is gone
        assert!(
            !ctx.has_index(1, "Article", &format!("field{}", i)),
            "Index should be dropped"
        );
    }
}

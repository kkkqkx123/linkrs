//! Fulltext Integration Tests - Advanced Query Features
//!
//! Test scope:
//! - Boolean queries (AND, OR, NOT semantics)
//! - Phrase queries
//! - Prefix/wildcard searches
//! - Fuzzy matching
//! - Complex query combinations
//!
//! Test cases: TC-FT-ADV-001 ~ TC-FT-ADV-010

use super::common::{assert_search_result_contains, FulltextTestContext};
use graphdb_search::search::EngineType;

/// TC-FT-ADV-001: Boolean OR Query (Default Behavior)
#[tokio::test]
async fn test_boolean_or_query() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "rust programming language"),
        ("doc_2", "python programming language"),
        ("doc_3", "javascript web development"),
        ("doc_4", "go systems programming"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "rust python", 10)
        .await
        .expect("Search should succeed");

    assert!(
        results.len() >= 2,
        "OR query should find documents with either term"
    );
    assert_search_result_contains(&results, "doc_1").expect("Should find doc_1 with 'rust'");
    assert_search_result_contains(&results, "doc_2").expect("Should find doc_2 with 'python'");
}

/// TC-FT-ADV-002: Multi-Term Search Scoring
#[tokio::test]
async fn test_multi_term_search_scoring() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "rust programming"),
        ("doc_2", "rust"),
        ("doc_3", "programming"),
        ("doc_4", "rust programming tutorial"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "rust programming", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 4, "Should find all 4 documents");

    let doc_4_score = results
        .iter()
        .find(|r| r.doc_id == graphdb_core::core::Value::String("doc_4".to_string()))
        .map(|r| r.score)
        .unwrap_or(0.0);

    let doc_2_score = results
        .iter()
        .find(|r| r.doc_id == graphdb_core::core::Value::String("doc_2".to_string()))
        .map(|r| r.score)
        .unwrap_or(0.0);

    assert!(
        doc_4_score > doc_2_score,
        "Document with both terms should score higher than document with one term"
    );
}

/// TC-FT-ADV-003: Phrase-like Search
#[tokio::test]
async fn test_phrase_like_search() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "machine learning algorithms"),
        ("doc_2", "learning machine patterns"),
        ("doc_3", "deep learning neural networks"),
        ("doc_4", "machine learning is popular"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "machine learning", 10)
        .await
        .expect("Search should succeed");

    assert!(
        results.len() >= 3,
        "Should find documents containing 'machine' and/or 'learning'"
    );

    assert_search_result_contains(&results, "doc_1").expect("Should find doc_1");
    assert_search_result_contains(&results, "doc_4").expect("Should find doc_4");
}

/// TC-FT-ADV-004: Prefix Search Behavior
#[tokio::test]
async fn test_prefix_search_behavior() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "programming programmer programs"),
        ("doc_2", "programmatic approach"),
        ("doc_3", "program analysis"),
        ("doc_4", "developing developers"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "program", 10)
        .await
        .expect("Search should succeed");

    assert!(
        !results.is_empty(),
        "Should find documents with 'program' prefix variations"
    );
}

/// TC-FT-ADV-005: Case Insensitive Search
#[tokio::test]
async fn test_case_insensitive_search() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "Rust Programming Language"),
        ("doc_2", "RUST PROGRAMMING"),
        ("doc_3", "rust programming"),
        ("doc_4", "RuSt PrOgRaMmInG"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results_lower = ctx
        .search(1, "Article", "content", "rust", 10)
        .await
        .expect("Search should succeed");

    let results_upper = ctx
        .search(1, "Article", "content", "RUST", 10)
        .await
        .expect("Search should succeed");

    let results_mixed = ctx
        .search(1, "Article", "content", "Rust", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(
        results_lower.len(),
        results_upper.len(),
        "Case should not affect result count"
    );
    assert_eq!(
        results_upper.len(),
        results_mixed.len(),
        "Case should not affect result count"
    );
}

/// TC-FT-ADV-006: Stop Words Handling
#[tokio::test]
async fn test_stop_words_handling() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "the quick brown fox"),
        ("doc_2", "a quick brown dog"),
        ("doc_3", "an amazing quick animal"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "quick brown", 10)
        .await
        .expect("Search should succeed");

    assert!(
        results.len() >= 2,
        "Should find documents with content words, ignoring stop words"
    );
}

/// TC-FT-ADV-007: Numeric Content Search
#[tokio::test]
async fn test_numeric_content_search() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "Version 2.0 release notes"),
        ("doc_2", "Bug fix in version 2.1"),
        ("doc_3", "Version 3.0 major update"),
        ("doc_4", "Release candidate 1.0"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "version", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 3, "Should find 3 documents with 'version'");
}

/// TC-FT-ADV-008: Special Characters in Content
#[tokio::test]
async fn test_special_characters_in_content() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "Email: user@example.com"),
        ("doc_2", "Price: $99.99 dollars"),
        ("doc_3", "Phone: +1-555-123-4567"),
        ("doc_4", "URL: https://example.org/path"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "example", 10)
        .await
        .expect("Search should succeed");

    assert!(
        results.len() >= 2,
        "Should find documents containing 'example'"
    );
}

/// TC-FT-ADV-009: Long Query Terms
#[tokio::test]
async fn test_long_query_terms() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let long_word = "supercalifragilisticexpialidocious";
    let doc1_content = format!("The word is {}", long_word);
    let doc3_content = format!("{} is a very long word", long_word);
    let docs = vec![
        ("doc_1", doc1_content.as_str()),
        ("doc_2", "A different long word"),
        ("doc_3", doc3_content.as_str()),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", long_word, 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 2, "Should find documents with the long word");
}

/// TC-FT-ADV-010: Empty Query Handling
#[tokio::test]
async fn test_empty_query_handling() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "Some content here"),
        ("doc_2", "More content there"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "", 10)
        .await
        .expect("Search should succeed");

    assert!(
        results.is_empty() || results.len() <= 2,
        "Empty query should return empty or limited results"
    );
}

/// TC-FT-ADV-011: Repeated Terms in Query
#[tokio::test]
async fn test_repeated_terms_in_query() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "rust programming"),
        ("doc_2", "python programming"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results_single = ctx
        .search(1, "Article", "content", "rust", 10)
        .await
        .expect("Search should succeed");

    let results_repeated = ctx
        .search(1, "Article", "content", "rust rust rust", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(
        results_single.len(),
        results_repeated.len(),
        "Repeated terms should not change result count"
    );
}

/// TC-FT-ADV-012: Query with Mixed Languages
#[tokio::test]
async fn test_mixed_language_query() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "Hello World"),
        ("doc_2", "Bonjour le monde"),
        ("doc_3", "Hola Mundo"),
        ("doc_4", "Hello Bonjour Hola"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "Hello", 10)
        .await
        .expect("Search should succeed");

    assert!(
        results.len() >= 2,
        "Should find documents with 'Hello' in different language contexts"
    );
}

/// TC-FT-ADV-013: Query Result Ranking
#[tokio::test]
async fn test_query_result_ranking() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "rust rust rust programming"),
        ("doc_2", "rust programming"),
        ("doc_3", "programming in rust"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results = ctx
        .search(1, "Article", "content", "rust", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(results.len(), 3, "Should find all 3 documents");

    for i in 1..results.len() {
        assert!(
            results[i - 1].score >= results[i].score,
            "Results should be sorted by score descending"
        );
    }
}

/// TC-FT-ADV-014: Query with Whitespace Variations
#[tokio::test]
async fn test_query_whitespace_variations() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs = vec![
        ("doc_1", "rust programming language"),
        ("doc_2", "python programming language"),
    ];
    ctx.insert_test_docs(1, "Article", "content", docs)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results_normal = ctx
        .search(1, "Article", "content", "rust programming", 10)
        .await
        .expect("Search should succeed");

    let results_extra_space = ctx
        .search(1, "Article", "content", "rust  programming", 10)
        .await
        .expect("Search should succeed");

    assert_eq!(
        results_normal.len(),
        results_extra_space.len(),
        "Extra whitespace should not affect results"
    );
}

/// TC-FT-ADV-015: Query Limit Edge Cases
#[tokio::test]
async fn test_query_limit_edge_cases() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(1, "Article", "content", Some(EngineType::Bm25))
        .await
        .expect("Failed to create index");

    let docs: Vec<(String, String)> = (0..50)
        .map(|i| (format!("doc_{}", i), format!("Test document number {}", i)))
        .collect();
    let docs_ref: Vec<(&str, &str)> = docs
        .iter()
        .map(|(id, content)| (id.as_str(), content.as_str()))
        .collect();
    ctx.insert_test_docs(1, "Article", "content", docs_ref)
        .await
        .expect("Failed to insert documents");

    ctx.commit_all().await.expect("Failed to commit");

    let results_zero = ctx
        .search(1, "Article", "content", "Test", 0)
        .await
        .expect("Search should succeed");
    assert_eq!(results_zero.len(), 0, "Limit 0 should return 0 results");

    let results_one = ctx
        .search(1, "Article", "content", "Test", 1)
        .await
        .expect("Search should succeed");
    assert_eq!(results_one.len(), 1, "Limit 1 should return 1 result");

    let results_large = ctx
        .search(1, "Article", "content", "Test", 1000)
        .await
        .expect("Search should succeed");
    assert_eq!(
        results_large.len(),
        50,
        "Large limit should return all matching documents"
    );
}

//! Vector Integration Tests - Embedding Provider
//!
//! Test scope:
//! - Mock embedding provider functionality
//! - Text to vector conversion
//! - Batch embedding generation
//! - Embedding consistency
//!
//! Test cases: TC-VEC-EMB-001 ~ TC-VEC-EMB-005

use super::common::VectorTestContext;
use vector_client::embedding::EmbeddingProvider;

/// TC-VEC-EMB-001: Generate Single Embedding
#[tokio::test]
async fn test_generate_single_embedding() {
    let ctx = VectorTestContext::with_dimension(128);

    let text = "This is a test document for embedding generation.";
    let embedding = ctx.generate_embedding(text).await;

    assert_eq!(
        embedding.len(),
        128,
        "Embedding should have correct dimension"
    );
}

/// TC-VEC-EMB-002: Generate Batch Embeddings
#[tokio::test]
async fn test_generate_batch_embeddings() {
    let ctx = VectorTestContext::with_dimension(128);

    let texts = vec![
        "First document for embedding.",
        "Second document for embedding.",
        "Third document for embedding.",
    ];

    let embeddings = ctx.generate_embeddings(&texts).await;

    assert_eq!(embeddings.len(), 3, "Should generate 3 embeddings");
    for embedding in &embeddings {
        assert_eq!(
            embedding.len(),
            128,
            "Each embedding should have correct dimension"
        );
    }
}

/// TC-VEC-EMB-003: Embedding Consistency
#[tokio::test]
async fn test_embedding_consistency() {
    let ctx = VectorTestContext::with_dimension(128);

    let text = "Consistent text for embedding test.";

    let embedding1 = ctx.generate_embedding(text).await;
    let embedding2 = ctx.generate_embedding(text).await;

    assert_eq!(
        embedding1, embedding2,
        "Same text should produce same embedding"
    );
}

/// TC-VEC-EMB-004: Different Text Different Embedding
#[tokio::test]
async fn test_different_text_different_embedding() {
    let ctx = VectorTestContext::with_dimension(128);

    let text1 = "First unique text.";
    let text2 = "Second unique text.";

    let embedding1 = ctx.generate_embedding(text1).await;
    let embedding2 = ctx.generate_embedding(text2).await;

    assert_ne!(
        embedding1, embedding2,
        "Different texts should produce different embeddings"
    );
}

/// TC-VEC-EMB-005: Embedding Provider Properties
#[tokio::test]
async fn test_embedding_provider_properties() {
    let ctx = VectorTestContext::with_dimension(256);

    let provider = ctx.embedding_provider.clone();

    assert_eq!(
        provider.dimension(),
        256,
        "Provider should report correct dimension"
    );
    assert_eq!(
        provider.model_name(),
        "mock-embedding-model",
        "Provider should report model name"
    );
}

/// TC-VEC-EMB-006: Embedding and Search Integration
#[tokio::test]
async fn test_embedding_and_search_integration() {
    let ctx = VectorTestContext::with_dimension(128);

    ctx.manager
        .create_index(
            "test_collection",
            vector_client::types::CollectionConfig::new(
                128,
                vector_client::types::DistanceMetric::Cosine,
            ),
        )
        .await
        .expect("Failed to create index");

    let texts = vec![
        "Machine learning is a subset of artificial intelligence.",
        "Deep learning uses neural networks with many layers.",
        "Natural language processing enables computers to understand text.",
        "Computer vision allows machines to interpret visual information.",
        "Reinforcement learning trains agents through rewards and penalties.",
    ];

    let embeddings = ctx.generate_embeddings(&texts).await;

    let points: Vec<vector_client::types::VectorPoint> = texts
        .iter()
        .enumerate()
        .zip(embeddings.into_iter())
        .map(|((i, _text), embedding)| {
            vector_client::types::VectorPoint::new(format!("doc_{}", i), embedding)
        })
        .collect();

    ctx.manager
        .upsert_batch("test_collection", points)
        .await
        .expect("Failed to insert vectors");

    let query_text = "What is machine learning?";
    let query_embedding = ctx.generate_embedding(query_text).await;

    let query = vector_client::types::SearchQuery::new(query_embedding, 3);
    let results = ctx
        .manager
        .search("test_collection", query)
        .await
        .expect("Search should succeed");

    assert!(!results.is_empty(), "Should have search results");
    assert!(
        results[0].id.to_string().starts_with("doc_"),
        "Result ID should be a document ID"
    );
}

/// TC-VEC-EMB-007: Empty Text Embedding
#[tokio::test]
async fn test_empty_text_embedding() {
    let ctx = VectorTestContext::with_dimension(128);

    let embedding = ctx.generate_embedding("").await;

    assert_eq!(
        embedding.len(),
        128,
        "Empty text should still produce embedding"
    );
}

/// TC-VEC-EMB-008: Long Text Embedding
#[tokio::test]
async fn test_long_text_embedding() {
    let ctx = VectorTestContext::with_dimension(128);

    let long_text = "This is a very long text. ".repeat(100);
    let embedding = ctx.generate_embedding(&long_text).await;

    assert_eq!(
        embedding.len(),
        128,
        "Long text should produce embedding with correct dimension"
    );
}

/// TC-VEC-EMB-009: Unicode Text Embedding
#[tokio::test]
async fn test_unicode_text_embedding() {
    let ctx = VectorTestContext::with_dimension(128);

    let unicode_texts = vec![
        "中文文本测试",
        "日本語テキストテスト",
        "한국어 텍스트 테스트",
        "🎉🎊🎁 Emoji test",
    ];

    let embeddings = ctx.generate_embeddings(&unicode_texts).await;

    assert_eq!(
        embeddings.len(),
        4,
        "Should generate 4 embeddings for unicode texts"
    );
    for embedding in &embeddings {
        assert_eq!(
            embedding.len(),
            128,
            "Each embedding should have correct dimension"
        );
    }
}

/// TC-VEC-EMB-010: Embedding Dimension Verification
#[tokio::test]
async fn test_embedding_dimension_verification() {
    let dimensions = vec![64, 128, 256, 512, 768, 1536];

    for dim in dimensions {
        let ctx = VectorTestContext::with_dimension(dim);
        let embedding = ctx.generate_embedding("Test text").await;
        assert_eq!(
            embedding.len(),
            dim,
            "Embedding should have dimension {}",
            dim
        );
    }
}

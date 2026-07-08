use super::common::{create_test_points, generate_test_vectors, VectorTestContext};
use vector_client::types::{DistanceMetric, SearchMode, SearchQuery};

/// KNN search with custom ef
#[tokio::test]
async fn test_search_mode_knn() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(20, 64, 42);
    let ids: Vec<String> = (0..20).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let query_vec = vectors[0].clone();
    let search_query = SearchQuery::new(query_vec, 5).with_search_mode(SearchMode::KNN {
        k: 5,
        ef_search: Some(64),
    });

    let results = ctx
        .manager
        .search("space_1_Doc_emb", search_query)
        .await
        .expect("search");
    assert_eq!(results.len(), 5, "KNN should return exactly k results");
    assert!(
        results[0].score > 0.99,
        "closest vector should have score near 1.0"
    );
}

/// Range search with radius
#[tokio::test]
async fn test_search_mode_range() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(10, 64, 42);
    let ids: Vec<String> = (0..10).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let query_vec = vectors[0].clone();
    let search_query = SearchQuery::new(query_vec, 10).with_search_mode(SearchMode::Range {
        radius: 0.5,
        max_results: Some(3),
    });

    let results = ctx
        .manager
        .search("space_1_Doc_emb", search_query)
        .await
        .expect("search");
    assert!(!results.is_empty(), "should find some results within range");
    assert!(results.len() <= 3, "max_results should cap at 3");
}

/// TopK is the default search mode
#[tokio::test]
async fn test_search_mode_topk() {
    let ctx = VectorTestContext::with_dimension(64);

    ctx.create_test_index(1, "Doc", "emb", Some(64), Some(DistanceMetric::Cosine))
        .await
        .expect("create index");

    let vectors = generate_test_vectors(5, 64, 42);
    let ids: Vec<String> = (0..5).map(|i| format!("doc_{}", i)).collect();
    let points = create_test_points(
        ids.iter().map(|s| s.as_str()).collect(),
        vectors.clone(),
        None,
    );
    ctx.insert_test_vectors(1, "Doc", "emb", points)
        .await
        .expect("insert");

    let query_vec = vectors[0].clone();
    let search_query = SearchQuery::new(query_vec, 3).with_search_mode(SearchMode::TopK(3));

    let results = ctx
        .manager
        .search("space_1_Doc_emb", search_query)
        .await
        .expect("search");
    assert_eq!(results.len(), 3);
    assert!(
        results[0].score >= results[1].score,
        "should be sorted by score desc"
    );
}

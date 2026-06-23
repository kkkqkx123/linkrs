//! Cache Warmup Tests

use graphdb::query::cache::{QueryStats, WarmupConfig, WarmupCte, WarmupQuery, WarmupResult};

#[test]
fn test_warmup_config_default() {
    let config = WarmupConfig::default();

    assert!(config.queries.is_empty());
    assert!(config.ctes.is_empty());
    assert_eq!(config.min_frequency_threshold, 0);
}

#[test]
fn test_warmup_config_with_query() {
    let config = WarmupConfig::new()
        .with_query("SELECT * FROM users")
        .with_query("SELECT * FROM posts");

    assert_eq!(config.queries.len(), 2);
    assert_eq!(config.queries[0].query, "SELECT * FROM users");
    assert_eq!(config.queries[1].query, "SELECT * FROM posts");
}

#[test]
fn test_warmup_query_from_str() {
    let query: WarmupQuery = "SELECT * FROM users".into();

    assert_eq!(query.query, "SELECT * FROM users");
    assert!(query.frequency.is_none());
    assert!(query.tables.is_none());
}

#[test]
fn test_warmup_query_with_fields() {
    let query = WarmupQuery {
        query: "MATCH (n:Person) RETURN n".to_string(),
        frequency: Some(5),
        tables: Some(vec!["Person".to_string()]),
    };

    assert_eq!(query.query, "MATCH (n:Person) RETURN n");
    assert_eq!(query.frequency, Some(5));
    assert_eq!(query.tables, Some(vec!["Person".to_string()]));
}

#[test]
fn test_warmup_cte_creation() {
    let cte = WarmupCte {
        definition: "WITH cte AS (SELECT 1) SELECT * FROM cte".to_string(),
        estimated_rows: 100,
        compute_cost_ms: Some(10),
        tables: None,
    };

    assert_eq!(cte.definition, "WITH cte AS (SELECT 1) SELECT * FROM cte");
    assert_eq!(cte.estimated_rows, 100);
    assert_eq!(cte.compute_cost_ms, Some(10));
}

#[test]
fn test_warmup_result_default() {
    let result = WarmupResult::default();

    assert_eq!(result.successful_queries, 0);
    assert_eq!(result.failed_queries, 0);
    assert_eq!(result.successful_ctes, 0);
    assert_eq!(result.failed_ctes, 0);
    assert!(result.errors.is_empty());
}

#[test]
fn test_warmup_result_is_success() {
    let success = WarmupResult {
        successful_queries: 10,
        failed_queries: 0,
        successful_ctes: 5,
        failed_ctes: 0,
        errors: vec![],
        duration_ms: 100,
    };
    assert!(success.is_success());

    let failure = WarmupResult {
        successful_queries: 10,
        failed_queries: 1,
        successful_ctes: 5,
        failed_ctes: 0,
        errors: vec!["error".to_string()],
        duration_ms: 100,
    };
    assert!(!failure.is_success());
}

#[test]
fn test_warmup_result_format() {
    let result = WarmupResult {
        successful_queries: 10,
        failed_queries: 2,
        successful_ctes: 5,
        failed_ctes: 1,
        errors: vec!["error1".to_string()],
        duration_ms: 100,
    };

    let formatted = result.format();
    assert!(formatted.contains("Queries: 10 successful, 2 failed"));
    assert!(formatted.contains("CTEs: 5 successful, 1 failed"));
    assert!(formatted.contains("Duration: 100ms"));
}

#[test]
fn test_query_stats() {
    let mut stats = QueryStats::new();

    stats.record_query("SELECT * FROM users");
    stats.record_query("SELECT * FROM users");
    stats.record_query("SELECT * FROM posts");

    assert_eq!(stats.total_queries(), 3);
    assert_eq!(stats.query_frequency("SELECT * FROM users"), 2);
    assert_eq!(stats.query_frequency("SELECT * FROM posts"), 1);
}

#[test]
fn test_query_stats_most_frequent() {
    let mut stats = QueryStats::new();

    stats.record_query("query1");
    stats.record_query("query1");
    stats.record_query("query1");

    stats.record_query("query2");
    stats.record_query("query2");

    let most_frequent = stats.most_frequent_queries(2);

    assert_eq!(most_frequent.len(), 2);
    assert_eq!(most_frequent[0].0, "query1");
    assert_eq!(most_frequent[0].1, 3);
    assert_eq!(most_frequent[1].0, "query2");
    assert_eq!(most_frequent[1].1, 2);
}

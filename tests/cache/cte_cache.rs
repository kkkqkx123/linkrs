//! CTE Cache Tests

use graphdb::query::cache::{CachePriority, CteCacheDecisionMaker, CteCacheEntry, CteCacheManager};
use std::sync::Arc;
use std::time::Instant;

#[test]
fn test_cte_cache_manager_creation() {
    let cache = CteCacheManager::new();

    assert_eq!(cache.entry_count(), 0);
}

#[test]
fn test_cte_cache_manager_clear() {
    let cache = CteCacheManager::new();
    cache.clear();

    assert_eq!(cache.entry_count(), 0);
}

#[test]
fn test_cte_cache_manager_stats() {
    let cache = CteCacheManager::new();
    let stats = cache.stats();

    assert_eq!(stats.memory.entry_count(), 0);
    assert_eq!(stats.memory.current_bytes(), 0);
}

#[test]
fn test_cte_cache_entry_creation() {
    let entry = CteCacheEntry {
        data: Arc::new(vec![1, 2, 3, 4]),
        row_count: 100,
        data_size: 1024,
        created_at: Instant::now(),
        last_accessed: Instant::now(),
        access_count: 0,
        reuse_probability: 0.5,
        cte_hash: "test_hash".to_string(),
        cte_definition: "WITH cte AS (SELECT 1) SELECT * FROM cte".to_string(),
        priority: CachePriority::Normal,
        compute_cost_ms: 10,
        access_frequency: 1.0,
        dependent_tables: vec!["users".to_string()],
    };

    assert_eq!(entry.cte_hash, "test_hash");
    assert_eq!(entry.row_count, 100);
    assert_eq!(entry.dependent_tables.len(), 1);
}

#[test]
fn test_cte_cache_entry_data_size() {
    let entry = CteCacheEntry {
        data: Arc::new(vec![1, 2, 3, 4]),
        row_count: 100,
        data_size: 2048,
        created_at: Instant::now(),
        last_accessed: Instant::now(),
        access_count: 0,
        reuse_probability: 0.5,
        cte_hash: "test_hash".to_string(),
        cte_definition: "WITH cte AS (SELECT 1) SELECT * FROM cte".to_string(),
        priority: CachePriority::Normal,
        compute_cost_ms: 10,
        access_frequency: 1.0,
        dependent_tables: vec!["users".to_string(), "posts".to_string()],
    };

    assert!(entry.data_size > 0);
}

#[test]
fn test_cte_cache_entry_access_recording() {
    let mut entry = CteCacheEntry {
        data: Arc::new(vec![1, 2, 3, 4]),
        row_count: 100,
        data_size: 1024,
        created_at: Instant::now(),
        last_accessed: Instant::now(),
        access_count: 0,
        reuse_probability: 0.5,
        cte_hash: "test_hash".to_string(),
        cte_definition: "WITH cte AS (SELECT 1) SELECT * FROM cte".to_string(),
        priority: CachePriority::Normal,
        compute_cost_ms: 10,
        access_frequency: 1.0,
        dependent_tables: vec!["users".to_string()],
    };

    entry.record_access();
    entry.record_access();
    entry.record_access();

    assert_eq!(entry.access_count, 3);
}

#[test]
fn test_cte_cache_entry_age() {
    let entry = CteCacheEntry {
        data: Arc::new(vec![1, 2, 3, 4]),
        row_count: 100,
        data_size: 1024,
        created_at: Instant::now(),
        last_accessed: Instant::now(),
        access_count: 0,
        reuse_probability: 0.5,
        cte_hash: "test_hash".to_string(),
        cte_definition: "WITH cte AS (SELECT 1) SELECT * FROM cte".to_string(),
        priority: CachePriority::Normal,
        compute_cost_ms: 10,
        access_frequency: 1.0,
        dependent_tables: vec!["users".to_string()],
    };

    let age = entry.age();
    assert!(age.as_millis() < 100);
}

#[test]
fn test_cte_cache_entry_value_score() {
    let entry = CteCacheEntry {
        data: Arc::new(vec![1, 2, 3, 4]),
        row_count: 1000,
        data_size: 1024,
        created_at: Instant::now(),
        last_accessed: Instant::now(),
        access_count: 5,
        reuse_probability: 0.8,
        cte_hash: "test_hash".to_string(),
        cte_definition: "WITH cte AS (SELECT 1) SELECT * FROM cte".to_string(),
        priority: CachePriority::High,
        compute_cost_ms: 50,
        access_frequency: 2.0,
        dependent_tables: vec!["users".to_string()],
    };

    let score = entry.value_score();
    assert!(score > 0.0);
}

#[test]
fn test_cte_cache_decision_maker() {
    let cache = Arc::new(CteCacheManager::new());
    let maker = CteCacheDecisionMaker::new(cache);

    let decision = maker.decide("WITH cte AS (SELECT 1) SELECT * FROM cte", 1000, 50.0);
    assert!(!decision.reason.is_empty());
}

#[test]
fn test_cte_cache_decision_maker_no_cache_small_rows() {
    let cache = Arc::new(CteCacheManager::new());
    let maker = CteCacheDecisionMaker::new(cache);

    let decision = maker.decide("WITH cte AS (SELECT 1) SELECT * FROM cte", 10, 1.0);

    assert!(!decision.should_cache);
}

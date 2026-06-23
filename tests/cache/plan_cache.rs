//! Plan Cache Tests

use graphdb::query::cache::{PlanCacheConfig, PlanCacheKey, QueryPlanCache};

#[test]
fn test_plan_cache_key_creation() {
    let key = PlanCacheKey::from_query("SELECT * FROM users WHERE id = 1");

    assert_eq!(key.query_text(), "SELECT * FROM users WHERE id = 1");
}

#[test]
fn test_plan_cache_key_query_text() {
    let query = "MATCH (n:Person) RETURN n.name";
    let key = PlanCacheKey::from_query(query);

    assert_eq!(key.query_text(), query);
}

#[test]
fn test_plan_cache_key_verify() {
    let key1 = PlanCacheKey::from_query("SELECT * FROM users");
    let key2 = PlanCacheKey::from_query("SELECT * FROM users");
    let key3 = PlanCacheKey::from_query("SELECT * FROM posts");

    assert_eq!(key1, key2);
    assert_ne!(key1, key3);
}

#[test]
fn test_query_plan_cache_creation() {
    let config = PlanCacheConfig::default();
    let cache = QueryPlanCache::new(config);

    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_query_plan_cache_contains() {
    let config = PlanCacheConfig::default();
    let cache = QueryPlanCache::new(config);
    let key = "SELECT * FROM users";

    assert!(!cache.contains(key));
}

#[test]
fn test_query_plan_cache_clear() {
    let config = PlanCacheConfig::default();
    let cache = QueryPlanCache::new(config);
    cache.clear();

    assert!(cache.is_empty());
}

#[test]
fn test_query_plan_cache_stats() {
    let config = PlanCacheConfig::default();
    let cache = QueryPlanCache::new(config);
    let stats = cache.stats();

    assert_eq!(stats.memory.entry_count(), 0);
    assert_eq!(stats.memory.current_bytes(), 0);
}

#[test]
fn test_query_plan_cache_stats_snapshot() {
    let config = PlanCacheConfig::default();
    let cache = QueryPlanCache::new(config);
    let snapshot = cache.stats_snapshot();

    assert_eq!(snapshot.entry_count, 0);
    assert_eq!(snapshot.current_memory, 0);
}

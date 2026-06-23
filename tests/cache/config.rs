//! Cache Configuration Tests

use graphdb::query::cache::{
    CacheAllocations, CacheManagerConfig, CachePriority, CteCacheConfig, PlanCacheConfig,
};

#[test]
fn test_plan_cache_config_default() {
    let config = PlanCacheConfig::default();
    assert!(config.max_entries > 0);
    assert!(config.memory_budget > 0);
}

#[test]
fn test_cte_cache_config_default() {
    let config = CteCacheConfig::default();
    assert!(config.enabled);
    assert!(config.max_size > 0);
}

#[test]
fn test_cache_manager_config_validation() {
    let config = CacheManagerConfig::default();
    assert!(config.validate().is_ok());

    let invalid = CacheManagerConfig {
        total_budget: 0,
        ..Default::default()
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_cache_manager_config_presets() {
    let minimal = CacheManagerConfig::minimal();
    assert_eq!(minimal.total_budget, 32 * 1024 * 1024);

    let balanced = CacheManagerConfig::balanced();
    assert_eq!(balanced.total_budget, 128 * 1024 * 1024);

    let high_perf = CacheManagerConfig::high_performance();
    assert_eq!(high_perf.total_budget, 512 * 1024 * 1024);
}

#[test]
fn test_cache_allocations_validation() {
    let valid = CacheAllocations {
        plan_cache_ratio: 0.5,
        cte_cache_ratio: 0.3,
        reserve_ratio: 0.2,
    };
    assert!(valid.validate());

    let invalid = CacheAllocations {
        plan_cache_ratio: 0.6,
        cte_cache_ratio: 0.5,
        reserve_ratio: 0.1,
    };
    assert!(!invalid.validate());
}

#[test]
fn test_cache_priority_values() {
    assert_eq!(CachePriority::Low as i32, 0);
    assert_eq!(CachePriority::Normal as i32, 1);
    assert_eq!(CachePriority::High as i32, 2);
    assert_eq!(CachePriority::Critical as i32, 3);
}

#[test]
fn test_cache_priority_ordering() {
    assert!(CachePriority::Low < CachePriority::Normal);
    assert!(CachePriority::Normal < CachePriority::High);
    assert!(CachePriority::High < CachePriority::Critical);
}

#[test]
fn test_cache_priority_default() {
    let priority = CachePriority::default();
    assert_eq!(priority, CachePriority::Normal);
}

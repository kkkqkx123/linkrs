//! Basic CacheManager and Integration Tests

use graphdb::query::cache::{
    CacheAllocations, CacheManager, CacheManagerConfig, CacheStatsSummary, CteCacheStatsSnapshot,
    DataChangeEvent, GlobalCacheStatsSnapshot, PlanCacheStatsSnapshot,
};

// ==================== CacheManager Tests ====================

#[test]
fn test_cache_manager_creation() {
    let manager = CacheManager::new();
    assert_eq!(manager.total_memory_usage(), 0);
    assert!(manager.is_enabled());
}

#[test]
fn test_cache_manager_with_config() {
    let config = CacheManagerConfig {
        total_budget: 256 * 1024 * 1024,
        ..Default::default()
    };

    let manager = CacheManager::with_config(config);
    assert_eq!(manager.total_budget(), 256 * 1024 * 1024);
}

#[test]
fn test_cache_manager_presets() {
    let minimal = CacheManager::minimal();
    assert_eq!(minimal.total_budget(), 32 * 1024 * 1024);

    let balanced = CacheManager::balanced();
    assert_eq!(balanced.total_budget(), 128 * 1024 * 1024);

    let high_perf = CacheManager::high_performance();
    assert_eq!(high_perf.total_budget(), 512 * 1024 * 1024);
}

#[test]
fn test_cache_manager_stats() {
    let manager = CacheManager::new();
    let stats = manager.stats();

    assert_eq!(stats.total_budget, 128 * 1024 * 1024);
    assert_eq!(stats.total_memory, 0);
    assert_eq!(stats.total_hits, 0);
    assert_eq!(stats.total_misses, 0);
}

#[test]
fn test_cache_manager_resize_budget() {
    let manager = CacheManager::new();

    let result = manager.resize_budget(256 * 1024 * 1024);
    assert!(result.is_ok());
    assert_eq!(manager.total_budget(), 256 * 1024 * 1024);

    let result = manager.resize_budget(512);
    assert!(result.is_err());
}

#[test]
fn test_cache_manager_reallocate() {
    let manager = CacheManager::new();

    let new_allocations = CacheAllocations {
        plan_cache_ratio: 0.5,
        cte_cache_ratio: 0.3,
        reserve_ratio: 0.2,
    };

    let result = manager.reallocate(new_allocations);
    assert!(result.is_ok());

    let invalid_allocations = CacheAllocations {
        plan_cache_ratio: 0.6,
        cte_cache_ratio: 0.6,
        reserve_ratio: 0.2,
    };

    let result = manager.reallocate(invalid_allocations);
    assert!(result.is_err());
}

#[test]
fn test_cache_manager_clear() {
    let manager = CacheManager::new();
    manager.clear_all();

    assert_eq!(manager.total_memory_usage(), 0);
}

#[test]
fn test_cache_manager_memory_usage_ratio() {
    let manager = CacheManager::new();
    let ratio = manager.memory_usage_ratio();
    assert_eq!(ratio, 0.0);
}

#[test]
fn test_cache_manager_plan_cache_access() {
    let manager = CacheManager::new();
    let plan_cache = manager.plan_cache();

    assert!(plan_cache.is_empty());
}

#[test]
fn test_cache_manager_cte_cache_access() {
    let manager = CacheManager::new();
    let cte_cache = manager.cte_cache();

    assert_eq!(cte_cache.entry_count(), 0);
}

// ==================== CacheStatsSummary Tests ====================

#[test]
fn test_cache_stats_summary_from_snapshot() {
    let snapshot = GlobalCacheStatsSnapshot {
        plan_cache: PlanCacheStatsSnapshot {
            entry_count: 100,
            hit_rate: 0.85,
            ..Default::default()
        },
        cte_cache: CteCacheStatsSnapshot {
            entry_count: 50,
            hit_rate: 0.75,
            ..Default::default()
        },
        total_memory: 10 * 1024 * 1024,
        total_budget: 100 * 1024 * 1024,
        ..Default::default()
    };

    let summary = CacheStatsSummary::from_snapshot(&snapshot);

    assert_eq!(summary.plan_cache_entries, 100);
    assert_eq!(summary.cte_cache_entries, 50);
    assert!((summary.plan_cache_hit_rate - 0.85).abs() < 0.01);
    assert!((summary.cte_cache_hit_rate - 0.75).abs() < 0.01);
    assert_eq!(summary.total_memory_bytes, 10 * 1024 * 1024);
}

#[test]
fn test_cache_stats_summary_format() {
    let summary = CacheStatsSummary {
        plan_cache_entries: 100,
        plan_cache_hit_rate: 0.85,
        cte_cache_entries: 50,
        cte_cache_hit_rate: 0.75,
        total_memory_bytes: 10 * 1024 * 1024,
    };

    let formatted = summary.format();
    assert!(formatted.contains("Plan Cache: 100 entries"));
    assert!(formatted.contains("85.00% hit rate"));
    assert!(formatted.contains("CTE Cache: 50 entries"));
    assert!(formatted.contains("75.00% hit rate"));
    assert!(formatted.contains("Total Memory: 10.00 MB"));
}

// ==================== Integration Tests ====================

#[test]
fn test_cache_manager_full_workflow() {
    let manager = CacheManager::new();

    let plan_cache = manager.plan_cache();
    let cte_cache = manager.cte_cache();

    assert!(plan_cache.is_empty());
    assert_eq!(cte_cache.entry_count(), 0);

    manager.update_memory_usage();

    let stats = manager.stats();
    assert_eq!(stats.total_memory, 0);

    manager.clear_all();
}

#[test]
fn test_cache_invalidation_workflow() {
    let manager = CacheManager::new();

    manager.register_plan_dependency("SELECT * FROM users", vec!["users".to_string()]);

    let event = DataChangeEvent::update("users");
    manager.on_data_change(&event);

    let invalidation_stats = manager.invalidation_stats();
    assert!(invalidation_stats.total_invalidations > 0);
}

#[test]
fn test_cache_budget_resize_workflow() {
    let manager = CacheManager::new();

    let initial_budget = manager.total_budget();

    let result = manager.resize_budget(initial_budget * 2);
    assert!(result.is_ok());

    assert_eq!(manager.total_budget(), initial_budget * 2);
}

#[test]
fn test_cache_reallocate_workflow() {
    let manager = CacheManager::new();

    let new_allocations = CacheAllocations {
        plan_cache_ratio: 0.6,
        cte_cache_ratio: 0.3,
        reserve_ratio: 0.1,
    };

    let result = manager.reallocate(new_allocations);
    assert!(result.is_ok());
}

#[test]
fn test_cache_stats_collection() {
    let manager = CacheManager::new();

    let stats = manager.stats();

    assert!(stats.total_budget > 0);
    assert_eq!(stats.total_memory, 0);
    assert_eq!(stats.total_hits, 0);
    assert_eq!(stats.total_misses, 0);
    assert_eq!(stats.evictions, 0);
}

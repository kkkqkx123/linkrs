//! Query Cache Module
//!
//! Provide a unified cache management function, including:
//! Query plan cache (Prepared Statement)
//! CTE result caching
//! Other cache types that may be added in the future:
//!
//! # Design Goals
//!
//! 1. Centralized management of all caches facilitates configuration and monitoring.
//! 2. Unified memory budget management
//! 3. Shared caching strategies (LRU, TTL, etc.)
//! 4. Unified collection of statistics and indicators
//! 5. Cache invalidation on data changes
//!
//! # Module Structure
//!
//! - `config`: Unified configuration for all cache types
//! - `stats`: Unified statistics collection and reporting
//! - `invalidation`: Cache invalidation strategies
//! - `plan_cache`: Query plan cache (Prepared Statement style)
//! - `cte_cache`: CTE result cache
//! - `manager`: Unified cache manager
//! - `warmup`: Cache warmup functionality

// Submodules
pub mod config;
pub mod cte_cache;
pub mod invalidation;
pub mod manager;
pub mod plan_cache;
pub mod stats;
pub mod warmup;

// Re-export config types
pub use config::{
    CacheAllocations, CacheManagerConfig, CachePriority, CteCacheConfig, PlanCacheConfig,
    PriorityConfig, TtlConfig,
};

// Re-export stats types
pub use stats::{
    CacheStats, CteCacheStats, CteCacheStatsSnapshot, GlobalCacheStatsSnapshot, MemoryStats,
    PlanCacheStats, PlanCacheStatsSnapshot,
};

// Re-export invalidation types
pub use invalidation::{
    CacheInvalidator, DataChangeEvent, DataChangeType, DependencyTracker, InvalidationManager,
    InvalidationStats, InvalidationStrategy, TableBasedInvalidation,
};

// Re-export the plan cache types
pub use plan_cache::{
    CachedPlan, ParamPosition, ParameterizedQueryHandler, PlanCacheKey, QueryPlanCache,
};

// Re-export the CTE cache types
pub use cte_cache::{CteCacheDecision, CteCacheDecisionMaker, CteCacheEntry, CteCacheManager};

// Re-export the manager types
pub use manager::{CacheManager, CacheStatsSummary, GlobalCacheManager, GlobalCacheStats};

// Re-export the warmup module types
pub use warmup::{
    CacheWarmer, QueryStats, WarmupConfig, WarmupCte, WarmupError, WarmupQuery, WarmupResult,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_manager_creation() {
        let manager = CacheManager::new();
        assert_eq!(manager.total_memory_usage(), 0);
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
    fn test_cache_stats_summary() {
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

        let formatted = summary.format();
        assert!(formatted.contains("Plan Cache: 100 entries"));
        assert!(formatted.contains("85.00% hit rate"));
    }

    #[test]
    fn test_data_change_event() {
        let event = DataChangeEvent::insert("users");
        assert_eq!(event.table_name, "users");
        assert_eq!(event.change_type, DataChangeType::Insert);
    }

    #[test]
    fn test_config_validation() {
        let config = CacheManagerConfig::default();
        assert!(config.validate().is_ok());

        let invalid = CacheManagerConfig {
            total_budget: 0,
            ..Default::default()
        };
        assert!(invalid.validate().is_err());
    }
}

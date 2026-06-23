//! Unified Cache Manager Module
//!
//! Centralized management of all caches, coordinating memory allocation,
//! providing unified monitoring interfaces, and cache invalidation.
//!
//! # Design Goals
//!
//! 1. Global memory budget management
//! 2. Unified monitoring and statistics
//! 3. Intelligent eviction policies
//! 4. Cache invalidation on data changes
//! 5. Emergency eviction when memory pressure is high

use parking_lot::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::config::{CacheAllocations, CacheManagerConfig, CteCacheConfig, PlanCacheConfig};
use super::cte_cache::CteCacheManager;
use super::invalidation::{
    CacheInvalidator, DataChangeEvent, InvalidationManager, InvalidationStats,
};
use super::plan_cache::QueryPlanCache;
use super::stats::GlobalCacheStatsSnapshot;
use crate::core::stats::StatsManager;

/// Unified Cache Manager
///
/// Centralized management of all types of caches, with a unified configuration
/// and monitoring interface available.
#[derive(Debug)]
pub struct CacheManager {
    /// Configuration
    config: RwLock<CacheManagerConfig>,
    /// Query plan cache
    plan_cache: Arc<QueryPlanCache>,
    /// CTE result caching
    cte_cache: Arc<CteCacheManager>,
    /// Invalidation manager
    invalidation_manager: InvalidationManager,
    /// Current memory usage
    current_usage: AtomicUsize,
}

impl CacheManager {
    /// Create a new cache manager with default configuration.
    pub fn new() -> Self {
        Self::with_config(CacheManagerConfig::default())
    }

    /// Create using the configuration.
    pub fn with_config(config: CacheManagerConfig) -> Self {
        let plan_config = PlanCacheConfig {
            memory_budget: config.plan_cache_budget(),
            ..config.plan_cache.clone()
        };

        let cte_config = CteCacheConfig {
            max_size: config.cte_cache_budget(),
            ..config.cte_cache.clone()
        };

        let plan_cache = Arc::new(QueryPlanCache::new(plan_config));
        let cte_cache = Arc::new(CteCacheManager::with_config(cte_config));

        let invalidation_manager = InvalidationManager::new();

        Self {
            config: RwLock::new(config),
            plan_cache,
            cte_cache,
            invalidation_manager,
            current_usage: AtomicUsize::new(0),
        }
    }

    /// Create a minimal cache manager for embedded environments
    pub fn minimal() -> Self {
        Self::with_config(CacheManagerConfig::minimal())
    }

    /// Create a balanced cache manager (default)
    pub fn balanced() -> Self {
        Self::with_config(CacheManagerConfig::balanced())
    }

    /// Create a high-performance cache manager for server environments
    pub fn high_performance() -> Self {
        Self::with_config(CacheManagerConfig::high_performance())
    }

    /// Obtain the query plan cache
    pub fn plan_cache(&self) -> Arc<QueryPlanCache> {
        self.plan_cache.clone()
    }

    /// Obtaining the CTE cache
    pub fn cte_cache(&self) -> Arc<CteCacheManager> {
        self.cte_cache.clone()
    }

    /// Wire the stats manager to all caches
    pub fn with_stats_manager(self, stats_manager: Arc<StatsManager>) -> Self {
        self.plan_cache.set_stats_manager(stats_manager.clone());
        self.cte_cache.set_stats_manager(stats_manager);
        self
    }

    /// Get the invalidation manager
    pub fn invalidation_manager(&self) -> &InvalidationManager {
        &self.invalidation_manager
    }

    /// Get the configuration
    pub fn config(&self) -> CacheManagerConfig {
        self.config.read().clone()
    }

    /// Update the configuration
    pub fn set_config(&self, config: CacheManagerConfig) {
        *self.config.write() = config;
    }

    /// Get the total memory budget
    pub fn total_budget(&self) -> usize {
        self.config.read().total_budget
    }

    /// Get the current memory usage
    pub fn total_memory_usage(&self) -> usize {
        self.current_usage.load(Ordering::Relaxed)
    }

    /// Update memory usage statistics
    pub fn update_memory_usage(&self) {
        let plan_stats = self.plan_cache.stats_snapshot();
        let cte_stats = self.cte_cache.get_stats();

        let total_memory = plan_stats.current_memory + cte_stats.current_memory;
        self.current_usage.store(total_memory, Ordering::Relaxed);
    }

    /// Get memory usage ratio
    pub fn memory_usage_ratio(&self) -> f64 {
        let budget = self.total_budget();
        if budget == 0 {
            return 0.0;
        }
        self.total_memory_usage() as f64 / budget as f64
    }

    /// Get cache statistics summary
    pub fn stats(&self) -> GlobalCacheStatsSnapshot {
        self.update_memory_usage();

        let plan_stats = self.plan_cache.stats_snapshot();
        let cte_stats = self.cte_cache.get_stats();

        GlobalCacheStatsSnapshot {
            plan_cache: plan_stats,
            cte_cache: cte_stats,
            total_hits: self.plan_cache.stats().counters.hits()
                + self.cte_cache.stats().counters.hits(),
            total_misses: self.plan_cache.stats().counters.misses()
                + self.cte_cache.stats().counters.misses(),
            total_memory: self.total_memory_usage(),
            total_budget: self.total_budget(),
            evictions: self.plan_cache.stats().counters.evictions()
                + self.cte_cache.stats().counters.evictions(),
        }
    }

    /// Clear all caches
    pub fn clear_all(&self) {
        self.plan_cache.clear();
        self.cte_cache.clear();
        self.current_usage.store(0, Ordering::Relaxed);
    }

    /// Handle a data change event for cache invalidation
    pub fn on_data_change(&self, event: &DataChangeEvent) {
        self.invalidation_manager.on_data_change(event);
    }

    /// Register a table dependency for a plan cache entry
    pub fn register_plan_dependency(&self, query: &str, tables: Vec<String>) {
        for table in &tables {
            self.invalidation_manager
                .register_dependency(query.to_string(), table.clone());
        }
    }

    /// Register a table dependency for a CTE cache entry
    pub fn register_cte_dependency(&self, cte_hash: &str, tables: Vec<String>) {
        for table in &tables {
            self.invalidation_manager
                .register_dependency(cte_hash.to_string(), table.clone());
        }
    }

    /// Resize the total memory budget
    pub fn resize_budget(&self, new_budget: usize) -> Result<(), String> {
        if new_budget < 1024 * 1024 {
            return Err("Budget must be at least 1MB".to_string());
        }

        let mut config = self.config.write();
        config.total_budget = new_budget;

        let plan_budget = config.plan_cache_budget();
        let cte_budget = config.cte_cache_budget();

        self.plan_cache.stats().memory.set_max_bytes(plan_budget);
        self.cte_cache.stats().memory.set_max_bytes(cte_budget);

        Ok(())
    }

    /// Reallocate cache budgets
    pub fn reallocate(&self, new_allocations: CacheAllocations) -> Result<(), String> {
        if !new_allocations.validate() {
            return Err("Invalid cache allocations: ratios must sum to 1.0".to_string());
        }

        let mut config = self.config.write();
        config.allocations = new_allocations;

        let plan_budget = config.plan_cache_budget();
        let cte_budget = config.cte_cache_budget();

        self.plan_cache.stats().memory.set_max_bytes(plan_budget);
        self.cte_cache.stats().memory.set_max_bytes(cte_budget);

        Ok(())
    }

    /// Get invalidation statistics
    pub fn invalidation_stats(&self) -> InvalidationStats {
        self.invalidation_manager.stats()
    }

    /// Check if caches are enabled
    pub fn is_enabled(&self) -> bool {
        self.config.read().cte_cache.enabled
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheInvalidator for CacheManager {
    fn invalidate(&self, key: &str) -> bool {
        let plan_removed = self.plan_cache.invalidate(key);
        let cte_removed = self.cte_cache.invalidate_by_hash(key);
        plan_removed || cte_removed
    }

    fn invalidate_all(&self) {
        self.clear_all();
    }

    fn invalidated_count(&self) -> u64 {
        self.plan_cache.stats().counters.evictions() + self.cte_cache.stats().counters.evictions()
    }
}

/// Cache statistics summary (for backward compatibility)
#[derive(Debug, Clone)]
pub struct CacheStatsSummary {
    /// Number of planned cache entries
    pub plan_cache_entries: usize,
    /// Plan Cache Hit Rate
    pub plan_cache_hit_rate: f64,
    /// Number of CTE cache entries
    pub cte_cache_entries: usize,
    /// CTE Cache Hit Rate
    pub cte_cache_hit_rate: f64,
    /// Total memory usage (in bytes)
    pub total_memory_bytes: usize,
}

impl CacheStatsSummary {
    /// Create from global stats snapshot
    pub fn from_snapshot(snapshot: &GlobalCacheStatsSnapshot) -> Self {
        Self {
            plan_cache_entries: snapshot.plan_cache.entry_count,
            plan_cache_hit_rate: snapshot.plan_cache.hit_rate,
            cte_cache_entries: snapshot.cte_cache.entry_count,
            cte_cache_hit_rate: snapshot.cte_cache.hit_rate,
            total_memory_bytes: snapshot.total_memory,
        }
    }

    /// Format the statistics for display
    pub fn format(&self) -> String {
        format!(
            "Cache Statistics:\n\
             - Plan Cache: {} entries, {:.2}% hit rate\n\
             - CTE Cache: {} entries, {:.2}% hit rate\n\
             - Total Memory: {:.2} MB",
            self.plan_cache_entries,
            self.plan_cache_hit_rate * 100.0,
            self.cte_cache_entries,
            self.cte_cache_hit_rate * 100.0,
            self.total_memory_bytes as f64 / 1024.0 / 1024.0
        )
    }
}

impl From<GlobalCacheStatsSnapshot> for CacheStatsSummary {
    fn from(snapshot: GlobalCacheStatsSnapshot) -> Self {
        Self::from_snapshot(&snapshot)
    }
}

/// Legacy type alias for backward compatibility
pub type GlobalCacheManager = CacheManager;

/// Legacy type alias for backward compatibility
pub type GlobalCacheStats = GlobalCacheStatsSnapshot;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::cache::stats::{CteCacheStatsSnapshot, PlanCacheStatsSnapshot};

    #[test]
    fn test_cache_manager_creation() {
        let manager = CacheManager::new();
        assert_eq!(manager.total_memory_usage(), 0);
        assert!(manager.is_enabled());
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
    fn test_cache_manager_clear() {
        let manager = CacheManager::new();
        manager.clear_all();

        assert_eq!(manager.total_memory_usage(), 0);
    }
}

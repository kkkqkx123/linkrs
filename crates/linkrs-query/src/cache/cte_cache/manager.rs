use moka::sync::Cache;
use std::sync::Arc;
use std::time::Duration;

use crate::cache::config::CteCacheConfig;
use crate::cache::stats::{CteCacheStats, CteCacheStatsSnapshot};
use linkrs_metrics::StatsManager;

use super::entry::CteCacheEntry;

/// CTE Cache Manager
///
/// Managing caching of CTE (Common Table Expression) query results and ensuring thread-safe access.
#[derive(Debug)]
pub struct CteCacheManager {
    /// Cache storage - using moka for high-performance concurrent access with weigher
    cache: Cache<String, Arc<CteCacheEntry>>,
    /// Configuration
    config: Arc<std::sync::RwLock<CteCacheConfig>>,
    /// Statistics
    stats: Arc<CteCacheStats>,
    /// Stats manager for reporting cache metrics
    stats_manager: std::sync::RwLock<Option<Arc<StatsManager>>>,
}

impl CteCacheManager {
    /// Create a new cache manager.
    pub fn new() -> Self {
        Self::with_config(CteCacheConfig::default())
    }

    /// Create using the configuration.
    pub fn with_config(config: CteCacheConfig) -> Self {
        let max_weight = config.max_size as u64;

        let cache = Cache::builder()
            .weigher(|_key, value: &Arc<CteCacheEntry>| -> u32 {
                let arc_overhead = std::mem::size_of::<Arc<CteCacheEntry>>();
                (value.estimate_memory() + arc_overhead) as u32
            })
            .max_capacity(max_weight)
            .time_to_live(Duration::from_secs(config.entry_ttl_seconds))
            .build();

        let stats = Arc::new(CteCacheStats::new(config.max_size));

        Self {
            cache,
            config: Arc::new(std::sync::RwLock::new(config.clone())),
            stats,
            stats_manager: std::sync::RwLock::new(None),
        }
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<StatsManager>) -> Self {
        self.stats_manager = std::sync::RwLock::new(Some(stats_manager));
        self
    }

    /// Set the stats manager after creation
    pub fn set_stats_manager(&self, stats_manager: Arc<StatsManager>) {
        if let Ok(mut guard) = self.stats_manager.write() {
            *guard = Some(stats_manager);
        }
    }

    /// Obtain the configuration.
    pub fn config(&self) -> CteCacheConfig {
        self.config.read().expect("Config lock poisoned").clone()
    }

    /// Update the configuration.
    pub fn set_config(&self, config: CteCacheConfig) {
        let mut cfg = self.config.write().expect("Config lock poisoned");
        self.stats.memory.set_max_bytes(config.max_size);
        *cfg = config;
    }

    /// Determine whether to enable caching.
    pub fn is_enabled(&self) -> bool {
        self.config.read().expect("Config lock poisoned").enabled
    }

    /// Determine whether the results of the CTE (Common Table Expression) are cached.
    ///
    /// # Parameters
    /// `cte_definition`: Text defining the Common Table Expression (CTE).
    /// estimated_rows: The estimated number of rows
    /// `is_deterministic`: Whether the CTE (Common Table Expression) is deterministic.
    pub fn should_cache(
        &self,
        cte_definition: &str,
        estimated_rows: u64,
        is_deterministic: bool,
    ) -> bool {
        let config = self.config.read().expect("Config lock poisoned");

        if !config.enabled {
            return false;
        }

        if !is_deterministic {
            return false;
        }

        if estimated_rows < config.min_row_count || estimated_rows > config.max_row_count {
            return false;
        }

        let reuse_prob = self.predict_reuse_probability(cte_definition);
        if reuse_prob < 0.3 {
            return false;
        }

        true
    }

    /// Predict the probability of reuse
    pub(super) fn predict_reuse_probability(&self, cte_definition: &str) -> f64 {
        let cte_hash = Self::compute_hash(cte_definition);

        if let Some(entry) = self.cache.get(&cte_hash) {
            return entry.reuse_probability;
        }

        let complexity = cte_definition.len() as f64 / 100.0;
        let base_prob = 0.5;
        let complexity_bonus = (complexity / 10.0).min(0.3);

        base_prob + complexity_bonus
    }

    /// Calculate the hash value defined by the CTE (Common Table Expression).
    pub(super) fn compute_hash(cte_definition: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        cte_definition.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Store the data in the cache.
    pub fn put(&self, cte_definition: &str, data: Vec<u8>, row_count: u64) -> Option<String> {
        self.put_with_cost(cte_definition, data, row_count, 100)
    }

    /// Store the data in the cache with compute cost.
    pub fn put_with_cost(
        &self,
        cte_definition: &str,
        data: Vec<u8>,
        row_count: u64,
        compute_cost_ms: u64,
    ) -> Option<String> {
        let config = self.config.read().expect("Config lock poisoned");

        if !config.enabled {
            return None;
        }

        if data.len() > config.max_entry_size {
            self.stats.counters.record_rejection();
            return None;
        }

        drop(config);

        let cte_hash = Self::compute_hash(cte_definition);
        let entry = Arc::new(CteCacheEntry::new(
            cte_hash.clone(),
            cte_definition.to_string(),
            data,
            row_count,
            compute_cost_ms,
        ));

        self.cache.insert(cte_hash.clone(), entry);

        self.stats.counters.record_insertion();
        self.update_stats();

        Some(cte_hash)
    }

    /// Store the data in the cache with dependent tables.
    pub fn put_with_tables(
        &self,
        cte_definition: &str,
        data: Vec<u8>,
        row_count: u64,
        compute_cost_ms: u64,
        dependent_tables: Vec<String>,
    ) -> Option<String> {
        let config = self.config.read().expect("Config lock poisoned");

        if !config.enabled {
            return None;
        }

        if data.len() > config.max_entry_size {
            self.stats.counters.record_rejection();
            return None;
        }

        drop(config);

        let cte_hash = Self::compute_hash(cte_definition);
        let mut entry = CteCacheEntry::new(
            cte_hash.clone(),
            cte_definition.to_string(),
            data,
            row_count,
            compute_cost_ms,
        );
        entry.dependent_tables = dependent_tables;

        self.cache.insert(cte_hash.clone(), Arc::new(entry));

        self.stats.counters.record_insertion();
        self.update_stats();

        Some(cte_hash)
    }

    /// Evict low priority entries
    pub fn evict_low_priority(&self, target_bytes: usize) -> usize {
        let mut freed = 0;
        let mut to_remove = Vec::new();

        let entries: Vec<_> = self
            .cache
            .iter()
            .map(|entry| {
                let value_score = entry.1.value_score();
                (
                    entry.0.as_ref().clone(),
                    value_score,
                    entry.1.data_size,
                    entry.1.priority,
                )
            })
            .collect();

        let mut entries_sorted = entries;
        entries_sorted.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.3.cmp(&b.3))
        });

        for (key, _, size, _) in entries_sorted {
            if freed >= target_bytes {
                break;
            }
            to_remove.push(key);
            freed += size;
        }

        for key in &to_remove {
            self.cache.invalidate(key);
        }

        if freed > 0 {
            self.stats.counters.record_eviction();
            self.update_stats();
        }

        freed
    }

    /// Retrieve data from the cache.
    pub fn get(&self, cte_definition: &str) -> Option<Arc<Vec<u8>>> {
        let config = self.config.read().expect("Config lock poisoned");

        if !config.enabled {
            return None;
        }

        drop(config);

        let cte_hash = Self::compute_hash(cte_definition);

        if let Some(entry) = self.cache.get(&cte_hash) {
            self.stats.counters.record_hit();
            if let Ok(ref sm_guard) = self.stats_manager.read() {
                if let Some(ref sm) = **sm_guard {
                    sm.record_cache_hit_global(true);
                }
            }
            Some(entry.data.clone())
        } else {
            self.stats.counters.record_miss();
            if let Ok(ref sm_guard) = self.stats_manager.read() {
                if let Some(ref sm) = **sm_guard {
                    sm.record_cache_hit_global(false);
                }
            }
            None
        }
    }

    /// Check whether it exists in the cache.
    pub fn contains(&self, cte_definition: &str) -> bool {
        let cte_hash = Self::compute_hash(cte_definition);
        self.cache.contains_key(&cte_hash)
    }

    /// Invalidate the cache entry
    pub fn invalidate(&self, cte_definition: &str) -> bool {
        let cte_hash = Self::compute_hash(cte_definition);
        self.invalidate_by_hash(&cte_hash)
    }

    /// Invalidate cache entry by hash
    pub fn invalidate_by_hash(&self, cte_hash: &str) -> bool {
        let removed = self.cache.remove(cte_hash).is_some();

        if removed {
            self.stats.counters.record_eviction();
            self.update_stats();
        }

        removed
    }

    /// Invalidate cache entries by table name
    pub fn invalidate_by_table(&self, table_name: &str) -> usize {
        let mut count = 0;
        let keys_to_remove: Vec<String> = self
            .cache
            .iter()
            .filter(|entry| entry.1.dependent_tables.iter().any(|t| t == table_name))
            .map(|entry| entry.0.as_ref().clone())
            .collect();

        for key in keys_to_remove {
            if self.cache.remove(&key).is_some() {
                count += 1;
            }
        }

        if count > 0 {
            self.stats.counters.record_eviction();
            self.update_stats();
        }

        count
    }

    /// Get cache entries for eviction (internal use)
    pub fn get_cache_entries(&self) -> Vec<(String, f64, usize)> {
        self.cache
            .iter()
            .map(|entry| {
                let value_score = entry.1.value_score();
                (entry.0.as_ref().clone(), value_score, entry.1.data_size)
            })
            .collect()
    }

    /// Increment eviction count (internal use)
    pub fn increment_evicted_count(&self, count: u64) {
        for _ in 0..count {
            self.stats.counters.record_eviction();
        }
    }

    /// Clear all caches.
    pub fn clear(&self) {
        self.cache.invalidate_all();
        self.stats.reset();
    }

    /// Obtain statistical information
    pub fn get_stats(&self) -> CteCacheStatsSnapshot {
        self.update_stats();
        self.stats.snapshot()
    }

    /// Update internal statistics
    fn update_stats(&self) {
        let current_entries = self.cache.entry_count() as usize;
        let current_memory = self.estimate_current_memory();
        self.stats.memory.update(current_memory, current_entries);
    }

    /// Estimate current memory usage
    fn estimate_current_memory(&self) -> usize {
        self.cache
            .iter()
            .map(|entry| entry.1.estimate_memory())
            .sum()
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        self.stats.reset();
    }

    /// Get the current memory usage
    pub fn current_memory(&self) -> usize {
        self.estimate_current_memory()
    }

    /// Obtain the number of cached entries
    pub fn entry_count(&self) -> usize {
        self.cache.entry_count() as usize
    }

    /// Clearance of obsolete entries
    /// Note: moka handles TTL automatically, so this is a no-op
    pub fn cleanup_expired(&self) -> usize {
        0
    }

    /// Get the statistics object
    pub fn stats(&self) -> Arc<CteCacheStats> {
        self.stats.clone()
    }
}

impl Default for CteCacheManager {
    fn default() -> Self {
        Self::new()
    }
}

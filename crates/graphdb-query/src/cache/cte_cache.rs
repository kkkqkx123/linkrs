//! CTE Results Cache Manager Module
//!
//! CTE (Common Table Expression) query result caching function.
//! Avoid repeated calculation of same CTE to improve query performance.
//!
//! ## Caching policy
//!
//! - LRU elimination policy: eliminate the longest unused entries when the cache is full
//! - Memory budget management: tightly control the upper limit of memory used by the cache
//! - Intelligent caching decision: decide whether to cache or not based on CTE characteristics
//!
//! ## Applicable scenarios
//!
//! 1. Recursive CTEs are referenced multiple times
//! 2. Complex subqueries are used multiple times in a single query
//! 3. Medium-sized result set (100-10,000 rows)
//! 4. CTE is deterministic (no random functions, etc.)

use moka::sync::Cache;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::config::{CachePriority, CteCacheConfig};
use super::stats::CteCacheStats;
use crate::core::stats::StatsManager;

/// CTE cache entries
#[derive(Debug, Clone)]
pub struct CteCacheEntry {
    /// Resulting data (shared using Arc)
    pub data: Arc<Vec<u8>>,
    /// Number of result rows
    pub row_count: u64,
    /// Result size (bytes)
    pub data_size: usize,
    /// Creation time
    pub created_at: Instant,
    /// Last access time
    pub last_accessed: Instant,
    /// Number of visits
    pub access_count: u64,
    /// Estimated probability of reuse
    pub reuse_probability: f64,
    /// CTE definition hash (used to identify identical CTEs)
    pub cte_hash: String,
    /// CTE definition text
    pub cte_definition: String,
    /// Cache priority
    pub priority: CachePriority,
    /// Compute cost (milliseconds)
    pub compute_cost_ms: u64,
    /// Access frequency (per minute)
    pub access_frequency: f64,
    /// Dependent tables (for invalidation detection)
    pub dependent_tables: Vec<String>,
}

impl CteCacheEntry {
    /// Creating a new cache entry
    pub fn new(
        cte_hash: String,
        cte_definition: String,
        data: Vec<u8>,
        row_count: u64,
        compute_cost_ms: u64,
    ) -> Self {
        let data_size = data.len();
        Self {
            data: Arc::new(data),
            row_count,
            data_size,
            created_at: Instant::now(),
            last_accessed: Instant::now(),
            access_count: 0,
            reuse_probability: 0.5,
            cte_hash,
            cte_definition,
            priority: CachePriority::Normal,
            compute_cost_ms,
            access_frequency: 0.0,
            dependent_tables: Vec::new(),
        }
    }

    /// Estimate memory usage (bytes)
    pub fn estimate_memory(&self) -> usize {
        let mut total = 0;

        total += std::mem::size_of::<Arc<Vec<u8>>>();
        total += std::mem::size_of::<Vec<u8>>();
        total += self.data_size;

        total += std::mem::size_of::<String>();
        total += self.cte_hash.capacity();

        total += std::mem::size_of::<String>();
        total += self.cte_definition.capacity();

        total += std::mem::size_of::<Vec<String>>();
        for table in &self.dependent_tables {
            total += std::mem::size_of::<String>();
            total += table.capacity();
        }

        total += std::mem::size_of::<Instant>() * 2;
        total += std::mem::size_of::<u64>() * 3;
        total += std::mem::size_of::<f64>() * 2;
        total += std::mem::size_of::<CachePriority>();

        total
    }

    /// Calculate cache value score (for eviction decisions)
    pub fn value_score(&self) -> f64 {
        let frequency_score = self.access_frequency * 0.4;
        let cost_score = (self.compute_cost_ms as f64 / 1000.0) * 0.3;
        let priority_score = (self.priority as i32 as f64) * 0.2;
        let size_penalty = (self.data_size as f64 / 1024.0 / 1024.0) * 0.1;

        frequency_score + cost_score + priority_score - size_penalty
    }

    /// Recorded visits
    pub fn record_access(&mut self) {
        self.last_accessed = Instant::now();
        self.access_count += 1;

        let elapsed_minutes = self.created_at.elapsed().as_secs_f64() / 60.0;
        if elapsed_minutes > 0.0 {
            self.access_frequency = self.access_count as f64 / elapsed_minutes;
        }

        self.reuse_probability = (self.reuse_probability * 0.7 + 0.3).min(0.95);
    }

    /// Get Cache Age
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Getting free time
    pub fn idle_time(&self) -> Duration {
        self.last_accessed.elapsed()
    }

    /// Calculate cache score (for LRU elimination decisions)
    /// The lower the score, the more likely you are to be eliminated
    pub fn cache_score(&self) -> f64 {
        let idle_factor = self.idle_time().as_secs_f64() / 60.0;
        let size_factor = (self.data_size as f64 / 1024.0 / 1024.0).max(0.1);
        let access_factor = (self.access_count as f64).sqrt().max(1.0);

        (idle_factor * size_factor) / (access_factor * self.reuse_probability)
    }
}

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
    fn predict_reuse_probability(&self, cte_definition: &str) -> f64 {
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
    fn compute_hash(cte_definition: &str) -> String {
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
                    sm.record_cache_hit(0, true);
                }
            }
            Some(entry.data.clone())
        } else {
            self.stats.counters.record_miss();
            if let Ok(ref sm_guard) = self.stats_manager.read() {
                if let Some(ref sm) = **sm_guard {
                    sm.record_cache_hit(0, false);
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
    pub fn get_stats(&self) -> super::stats::CteCacheStatsSnapshot {
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

/// CTE Cache Decision
///
/// Decide whether to use caching based on query characteristics
#[derive(Debug, Clone)]
pub struct CteCacheDecision {
    /// Should the cache be used?
    pub should_cache: bool,
    /// Reasons for decision-making
    pub reason: String,
    /// Estimated reuse probability
    pub reuse_probability: f64,
    /// Estimated caching gains
    pub estimated_benefit: f64,
    /// Suggested priority
    pub suggested_priority: CachePriority,
}

/// CTE cache decision maker
#[derive(Debug)]
pub struct CteCacheDecisionMaker {
    /// Cache Manager
    cache_manager: Arc<CteCacheManager>,
    /// Minimum Reuse Probability Threshold
    min_reuse_probability: f64,
    /// Minimum estimated gain
    min_benefit: f64,
}

impl CteCacheDecisionMaker {
    /// Create a new decision maker.
    pub fn new(cache_manager: Arc<CteCacheManager>) -> Self {
        Self {
            cache_manager,
            min_reuse_probability: 0.3,
            min_benefit: 1.0,
        }
    }

    /// Setting parameters
    pub fn with_params(mut self, min_reuse_probability: f64, min_benefit: f64) -> Self {
        self.min_reuse_probability = min_reuse_probability;
        self.min_benefit = min_benefit;
        self
    }

    /// Making a decision regarding caching
    pub fn decide(
        &self,
        cte_definition: &str,
        estimated_rows: u64,
        compute_cost: f64,
    ) -> CteCacheDecision {
        if !self.cache_manager.is_enabled() {
            return CteCacheDecision {
                should_cache: false,
                reason: "Cache disabled".to_string(),
                reuse_probability: 0.0,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        let config = self.cache_manager.config();

        if estimated_rows < config.min_row_count {
            return CteCacheDecision {
                should_cache: false,
                reason: format!(
                    "Row count {} below minimum {}",
                    estimated_rows, config.min_row_count
                ),
                reuse_probability: 0.0,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        if estimated_rows > config.max_row_count {
            return CteCacheDecision {
                should_cache: false,
                reason: format!(
                    "Row count {} above maximum {}",
                    estimated_rows, config.max_row_count
                ),
                reuse_probability: 0.0,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        let reuse_prob = self.cache_manager.predict_reuse_probability(cte_definition);

        if reuse_prob < self.min_reuse_probability {
            return CteCacheDecision {
                should_cache: false,
                reason: format!(
                    "Reuse probability {:.2} below threshold {:.2}",
                    reuse_prob, self.min_reuse_probability
                ),
                reuse_probability: reuse_prob,
                estimated_benefit: 0.0,
                suggested_priority: CachePriority::Low,
            };
        }

        let estimated_benefit = compute_cost * reuse_prob;

        let priority = if compute_cost > 1000.0 {
            CachePriority::High
        } else if compute_cost > 100.0 {
            CachePriority::Normal
        } else {
            CachePriority::Low
        };

        CteCacheDecision {
            should_cache: estimated_benefit >= self.min_benefit,
            reason: if estimated_benefit >= self.min_benefit {
                format!("Estimated benefit {:.2} meets threshold", estimated_benefit)
            } else {
                format!(
                    "Estimated benefit {:.2} below threshold {:.2}",
                    estimated_benefit, self.min_benefit
                )
            },
            reuse_probability: reuse_prob,
            estimated_benefit,
            suggested_priority: priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cte_cache_entry_creation() {
        let entry = CteCacheEntry::new(
            "hash123".to_string(),
            "SELECT * FROM users".to_string(),
            vec![1, 2, 3, 4],
            100,
            50,
        );

        assert_eq!(entry.row_count, 100);
        assert_eq!(entry.data_size, 4);
        assert_eq!(entry.compute_cost_ms, 50);
    }

    #[test]
    fn test_cte_cache_entry_memory_estimation() {
        let entry = CteCacheEntry::new(
            "hash".to_string(),
            "SELECT 1".to_string(),
            vec![0u8; 1024],
            10,
            10,
        );

        let memory = entry.estimate_memory();
        assert!(memory > 1024);
    }

    #[test]
    fn test_cte_cache_entry_value_score() {
        let mut entry = CteCacheEntry::new(
            "hash".to_string(),
            "SELECT 1".to_string(),
            vec![0u8; 1024],
            10,
            100,
        );

        entry.access_count = 10;
        entry.access_frequency = 5.0;

        let score = entry.value_score();
        assert!(score > 0.0);
    }

    #[test]
    fn test_cte_cache_manager_creation() {
        let manager = CteCacheManager::new();
        assert!(manager.is_enabled());
        assert_eq!(manager.entry_count(), 0);
    }

    #[test]
    fn test_cte_cache_manager_put_get() {
        let manager = CteCacheManager::new();

        let hash = manager.put("SELECT 1", vec![1, 2, 3], 1);
        assert!(hash.is_some());

        let data = manager.get("SELECT 1");
        assert!(data.is_some());
        assert_eq!(data.unwrap().as_ref(), &[1, 2, 3]);
    }

    #[test]
    fn test_cte_cache_manager_should_cache() {
        let manager = CteCacheManager::new();

        assert!(manager.should_cache("SELECT * FROM users", 500, true));
        assert!(!manager.should_cache("SELECT * FROM users", 10, true));
        assert!(!manager.should_cache("SELECT * FROM users", 500, false));
    }

    #[test]
    fn test_cte_cache_manager_invalidate() {
        let manager = CteCacheManager::new();

        manager.put("SELECT 1", vec![1, 2, 3], 1);
        assert!(manager.contains("SELECT 1"));

        assert!(manager.invalidate("SELECT 1"));
        assert!(!manager.contains("SELECT 1"));
    }

    #[test]
    fn test_cte_cache_decision_maker() {
        let manager = Arc::new(CteCacheManager::new());
        let decision_maker = CteCacheDecisionMaker::new(manager);

        let decision = decision_maker.decide("SELECT * FROM users", 500, 100.0);
        assert!(decision.should_cache);
    }

    #[test]
    fn test_compute_hash() {
        let hash1 = CteCacheManager::compute_hash("SELECT 1");
        let hash2 = CteCacheManager::compute_hash("SELECT 1");
        let hash3 = CteCacheManager::compute_hash("SELECT 2");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}

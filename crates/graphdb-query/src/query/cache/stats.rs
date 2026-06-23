//! Unified Cache Statistics Module
//!
//! Provides centralized statistics collection and reporting for all cache types.
//! Uses the `metrics` crate for production metrics export.

pub use crate::core::stats::utils::CacheStats;

use parking_lot::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Memory usage tracking
#[derive(Debug, Default)]
pub struct MemoryStats {
    current_bytes: AtomicUsize,
    max_bytes: AtomicUsize,
    entry_count: AtomicUsize,
}

impl MemoryStats {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            current_bytes: AtomicUsize::new(0),
            max_bytes: AtomicUsize::new(max_bytes),
            entry_count: AtomicUsize::new(0),
        }
    }

    pub fn update(&self, current_bytes: usize, entry_count: usize) {
        self.current_bytes.store(current_bytes, Ordering::Relaxed);
        self.entry_count.store(entry_count, Ordering::Relaxed);
    }

    pub fn set_max_bytes(&self, max_bytes: usize) {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
    }

    pub fn current_bytes(&self) -> usize {
        self.current_bytes.load(Ordering::Relaxed)
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes.load(Ordering::Relaxed)
    }

    pub fn entry_count(&self) -> usize {
        self.entry_count.load(Ordering::Relaxed)
    }

    pub fn usage_ratio(&self) -> f64 {
        let max = self.max_bytes();
        if max == 0 {
            0.0
        } else {
            self.current_bytes() as f64 / max as f64
        }
    }

    pub fn reset(&self) {
        self.current_bytes.store(0, Ordering::Relaxed);
        self.entry_count.store(0, Ordering::Relaxed);
    }
}

impl Clone for MemoryStats {
    fn clone(&self) -> Self {
        Self {
            current_bytes: AtomicUsize::new(self.current_bytes()),
            max_bytes: AtomicUsize::new(self.max_bytes()),
            entry_count: AtomicUsize::new(self.entry_count()),
        }
    }
}

/// Plan cache specific statistics
#[derive(Debug)]
pub struct PlanCacheStats {
    pub counters: CacheStats,
    pub memory: MemoryStats,
    pub avg_query_size: Arc<RwLock<usize>>,
    pub total_query_size: AtomicUsize,
}

impl Clone for PlanCacheStats {
    fn clone(&self) -> Self {
        Self {
            counters: self.counters.clone(),
            memory: self.memory.clone(),
            avg_query_size: Arc::new(RwLock::new(*self.avg_query_size.read())),
            total_query_size: AtomicUsize::new(self.total_query_size.load(Ordering::Relaxed)),
        }
    }
}

impl PlanCacheStats {
    pub fn new(memory_budget: usize) -> Self {
        Self {
            counters: CacheStats::new(),
            memory: MemoryStats::new(memory_budget),
            avg_query_size: Arc::new(RwLock::new(0)),
            total_query_size: AtomicUsize::new(0),
        }
    }

    pub fn record_query_size(&self, size: usize) {
        self.total_query_size.fetch_add(size, Ordering::Relaxed);
        let total = self.total_query_size.load(Ordering::Relaxed);
        let count = self.memory.entry_count();
        if let Some(avg) = total.checked_div(count) {
            *self.avg_query_size.write() = avg;
        }
    }

    pub fn avg_query_size(&self) -> usize {
        *self.avg_query_size.read()
    }

    pub fn estimated_memory(&self) -> usize {
        const PER_ENTRY_OVERHEAD: usize = 1024;
        let total_query = self.total_query_size.load(Ordering::Relaxed);
        let entries = self.memory.entry_count();
        total_query + (entries * PER_ENTRY_OVERHEAD)
    }

    pub fn hit_rate(&self) -> f64 {
        self.counters.hit_rate()
    }

    pub fn reset(&self) {
        self.counters.reset();
        self.memory.reset();
        self.total_query_size.store(0, Ordering::Relaxed);
        *self.avg_query_size.write() = 0;
    }

    pub fn snapshot(&self) -> PlanCacheStatsSnapshot {
        PlanCacheStatsSnapshot {
            hits: self.counters.hits(),
            misses: self.counters.misses(),
            evictions: self.counters.evictions(),
            expirations: self.counters.expirations(),
            entry_count: self.memory.entry_count(),
            current_memory: self.memory.current_bytes(),
            max_memory: self.memory.max_bytes(),
            hit_rate: self.hit_rate(),
            avg_query_size: self.avg_query_size(),
        }
    }
}

impl Default for PlanCacheStats {
    fn default() -> Self {
        Self::new(50 * 1024 * 1024)
    }
}

/// Plan cache statistics snapshot (for reporting)
#[derive(Debug, Clone, Default)]
pub struct PlanCacheStatsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub expirations: u64,
    pub entry_count: usize,
    pub current_memory: usize,
    pub max_memory: usize,
    pub hit_rate: f64,
    pub avg_query_size: usize,
}

/// CTE cache specific statistics
#[derive(Debug, Clone)]
pub struct CteCacheStats {
    pub counters: CacheStats,
    pub memory: MemoryStats,
}

impl CteCacheStats {
    pub fn new(max_size: usize) -> Self {
        Self {
            counters: CacheStats::new(),
            memory: MemoryStats::new(max_size),
        }
    }

    pub fn hit_rate(&self) -> f64 {
        self.counters.hit_rate()
    }

    pub fn memory_usage_ratio(&self) -> f64 {
        self.memory.usage_ratio()
    }

    pub fn reset(&self) {
        self.counters.reset();
        self.memory.reset();
    }

    pub fn snapshot(&self) -> CteCacheStatsSnapshot {
        CteCacheStatsSnapshot {
            hits: self.counters.hits(),
            misses: self.counters.misses(),
            evictions: self.counters.evictions(),
            rejections: self.counters.rejections(),
            entry_count: self.memory.entry_count(),
            current_memory: self.memory.current_bytes(),
            max_memory: self.memory.max_bytes(),
            hit_rate: self.hit_rate(),
            memory_usage_ratio: self.memory_usage_ratio(),
        }
    }
}

impl Default for CteCacheStats {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024)
    }
}

/// CTE cache statistics snapshot (for reporting)
#[derive(Debug, Clone, Default)]
pub struct CteCacheStatsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub rejections: u64,
    pub entry_count: usize,
    pub current_memory: usize,
    pub max_memory: usize,
    pub hit_rate: f64,
    pub memory_usage_ratio: f64,
}

/// Global cache statistics combining all cache types
#[derive(Debug, Clone, Default)]
pub struct GlobalCacheStatsSnapshot {
    pub plan_cache: PlanCacheStatsSnapshot,
    pub cte_cache: CteCacheStatsSnapshot,
    pub total_hits: u64,
    pub total_misses: u64,
    pub total_memory: usize,
    pub total_budget: usize,
    pub evictions: u64,
}

impl GlobalCacheStatsSnapshot {
    pub fn global_hit_rate(&self) -> f64 {
        let total = self.total_hits + self.total_misses;
        if total == 0 {
            0.0
        } else {
            self.total_hits as f64 / total as f64
        }
    }

    pub fn global_memory_usage_ratio(&self) -> f64 {
        if self.total_budget == 0 {
            0.0
        } else {
            self.total_memory as f64 / self.total_budget as f64
        }
    }

    pub fn format(&self) -> String {
        format!(
            "Global Cache Statistics:\n\
             - Hit Rate: {:.2}%\n\
             - Memory Usage: {:.2} MB / {:.2} MB ({:.1}%)\n\
             - Evictions: {}\n\
             - Plan Cache: {} entries, {:.2}% hit rate\n\
             - CTE Cache: {} entries, {:.2}% hit rate",
            self.global_hit_rate() * 100.0,
            self.total_memory as f64 / 1024.0 / 1024.0,
            self.total_budget as f64 / 1024.0 / 1024.0,
            self.global_memory_usage_ratio() * 100.0,
            self.evictions,
            self.plan_cache.entry_count,
            self.plan_cache.hit_rate * 100.0,
            self.cte_cache.entry_count,
            self.cte_cache.hit_rate * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_stats() {
        let stats = CacheStats::new();

        stats.record_hit();
        stats.record_hit();
        stats.record_miss();

        assert_eq!(stats.hits(), 2);
        assert_eq!(stats.misses(), 1);
        assert_eq!(stats.total_requests(), 3);
        assert!((stats.hit_rate() - 0.6666666666666666).abs() < 0.01);
    }

    #[test]
    fn test_cache_stats_reset() {
        let stats = CacheStats::new();
        stats.record_hit();
        stats.record_miss();
        stats.reset();

        assert_eq!(stats.hits(), 0);
        assert_eq!(stats.misses(), 0);
    }

    #[test]
    fn test_memory_stats() {
        let stats = MemoryStats::new(1000);
        stats.update(500, 10);

        assert_eq!(stats.current_bytes(), 500);
        assert_eq!(stats.entry_count(), 10);
        assert_eq!(stats.usage_ratio(), 0.5);
    }

    #[test]
    fn test_plan_cache_stats() {
        let stats = PlanCacheStats::new(1024 * 1024);

        stats.counters.record_hit();
        stats.counters.record_miss();
        stats.memory.update(512 * 1024, 5);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.hits, 1);
        assert_eq!(snapshot.misses, 1);
        assert_eq!(snapshot.entry_count, 5);
    }

    #[test]
    fn test_cte_cache_stats() {
        let stats = CteCacheStats::new(1024 * 1024);

        stats.counters.record_hit();
        stats.counters.record_hit();
        stats.memory.update(256 * 1024, 3);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.hits, 2);
        assert_eq!(snapshot.entry_count, 3);
        assert!((snapshot.hit_rate - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_global_stats_snapshot_format() {
        let snapshot = GlobalCacheStatsSnapshot {
            plan_cache: PlanCacheStatsSnapshot {
                hits: 600,
                misses: 100,
                entry_count: 100,
                hit_rate: 0.857,
                ..Default::default()
            },
            cte_cache: CteCacheStatsSnapshot {
                hits: 250,
                misses: 50,
                entry_count: 50,
                hit_rate: 0.833,
                ..Default::default()
            },
            total_hits: 850,
            total_misses: 150,
            total_memory: 50 * 1024 * 1024,
            total_budget: 100 * 1024 * 1024,
            evictions: 10,
        };

        let formatted = snapshot.format();
        assert!(formatted.contains("Hit Rate: 85.00%"));
        assert!(formatted.contains("Plan Cache: 100 entries"));
    }
}

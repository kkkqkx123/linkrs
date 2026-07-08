//! Cache Statistics Tests

use graphdb::query::cache::{
    CacheStats, CteCacheStats, CteCacheStatsSnapshot, GlobalCacheStatsSnapshot, MemoryStats,
    PlanCacheStats, PlanCacheStatsSnapshot,
};
use std::sync::Arc;

#[test]
fn test_cache_stats() {
    let stats = CacheStats::new();

    stats.record_hit();
    stats.record_hit();
    stats.record_miss();

    assert_eq!(stats.hits(), 2);
    assert_eq!(stats.misses(), 1);
    assert_eq!(stats.total_requests(), 3);
}

#[test]
fn test_cache_stats_hit_rate() {
    let stats = CacheStats::new();

    for _ in 0..8 {
        stats.record_hit();
    }
    for _ in 0..2 {
        stats.record_miss();
    }

    let hit_rate = stats.hit_rate();
    assert!((hit_rate - 0.8).abs() < 0.01);
}

#[test]
fn test_cache_stats_eviction() {
    let stats = CacheStats::new();

    stats.record_eviction();
    stats.record_eviction();
    stats.record_eviction();

    assert_eq!(stats.evictions(), 3);
}

#[test]
fn test_cache_stats_reset() {
    let stats = CacheStats::new();

    stats.record_hit();
    stats.record_miss();
    stats.record_eviction();

    stats.reset();

    assert_eq!(stats.hits(), 0);
    assert_eq!(stats.misses(), 0);
    assert_eq!(stats.evictions(), 0);
}

#[test]
fn test_memory_stats() {
    let stats = MemoryStats::new(1024 * 1024);

    stats.update(512 * 1024, 10);

    assert_eq!(stats.current_bytes(), 512 * 1024);
    assert_eq!(stats.max_bytes(), 1024 * 1024);
    assert_eq!(stats.entry_count(), 10);
}

#[test]
fn test_memory_stats_set_max() {
    let stats = MemoryStats::new(1024 * 1024);

    stats.set_max_bytes(2 * 1024 * 1024);

    assert_eq!(stats.max_bytes(), 2 * 1024 * 1024);
}

#[test]
fn test_plan_cache_stats() {
    let stats = Arc::new(PlanCacheStats::new(1024 * 1024));

    stats.counters.record_hit();
    stats.counters.record_miss();
    stats.memory.update(1024, 10);

    assert_eq!(stats.counters.hits(), 1);
    assert_eq!(stats.counters.misses(), 1);
    assert_eq!(stats.memory.entry_count(), 10);
    assert_eq!(stats.memory.current_bytes(), 1024);
}

#[test]
fn test_cte_cache_stats() {
    let stats = Arc::new(CteCacheStats::new(1024 * 1024));

    stats.counters.record_hit();
    stats.counters.record_hit();
    stats.counters.record_miss();
    stats.memory.update(2048, 5);

    assert_eq!(stats.counters.hits(), 2);
    assert_eq!(stats.counters.misses(), 1);
    assert_eq!(stats.memory.entry_count(), 5);
    assert_eq!(stats.memory.current_bytes(), 2048);
}

#[test]
fn test_plan_cache_stats_snapshot() {
    let stats = PlanCacheStats::new(1024 * 1024);
    stats.counters.record_hit();
    stats.counters.record_hit();
    stats.counters.record_miss();
    stats.memory.update(1024, 10);

    let snapshot = stats.snapshot();

    assert_eq!(snapshot.hits, 2);
    assert_eq!(snapshot.misses, 1);
    assert_eq!(snapshot.entry_count, 10);
    assert_eq!(snapshot.current_memory, 1024);
}

#[test]
fn test_global_cache_stats_snapshot() {
    let snapshot = GlobalCacheStatsSnapshot {
        plan_cache: PlanCacheStatsSnapshot {
            entry_count: 100,
            hit_rate: 0.9,
            ..Default::default()
        },
        cte_cache: CteCacheStatsSnapshot {
            entry_count: 50,
            hit_rate: 0.8,
            ..Default::default()
        },
        total_memory: 10 * 1024 * 1024,
        total_budget: 100 * 1024 * 1024,
        total_hits: 1000,
        total_misses: 100,
        evictions: 10,
    };

    assert_eq!(snapshot.plan_cache.entry_count, 100);
    assert_eq!(snapshot.cte_cache.entry_count, 50);
    assert_eq!(snapshot.total_memory, 10 * 1024 * 1024);
    assert_eq!(snapshot.total_hits, 1000);
}

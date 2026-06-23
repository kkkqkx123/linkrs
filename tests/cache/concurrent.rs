//! Cache Concurrency Tests

use graphdb::query::cache::{CacheManager, CacheStats, MemoryStats};
use std::sync::Arc;
use std::thread;

#[test]
fn test_cache_stats_concurrent_hits() {
    let stats = Arc::new(CacheStats::new());
    let mut handles = vec![];

    for _ in 0..10 {
        let stats_clone = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                stats_clone.record_hit();
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(stats.hits(), 1000);
}

#[test]
fn test_memory_stats_concurrent_update() {
    let stats = Arc::new(MemoryStats::new(1024 * 1024));
    let mut handles = vec![];

    for _ in 0..5 {
        let stats_clone = Arc::clone(&stats);
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                stats_clone.update(i * 1024, i);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(stats.current_bytes() <= 1024 * 1024);
}

#[test]
fn test_cache_manager_concurrent_access() {
    let manager = Arc::new(CacheManager::new());
    let mut handles = vec![];

    for _ in 0..10 {
        let manager_clone = Arc::clone(&manager);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let _ = manager_clone.stats();
                manager_clone.update_memory_usage();
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let stats = manager.stats();
    assert!(stats.total_budget > 0);
}

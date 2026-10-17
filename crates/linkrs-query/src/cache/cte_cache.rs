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
//!
//! The module was split from the original single `cte_cache.rs` into focused
//! submodules:
//! - `entry`: the [`CteCacheEntry`] cache-row type
//! - `manager`: the [`CteCacheManager`] store / eviction engine
//! - `decision`: the [`CteCacheDecisionMaker`] cache-policy evaluator

pub mod decision;
pub mod entry;
pub mod manager;

pub use decision::{CteCacheDecision, CteCacheDecisionMaker};
pub use entry::CteCacheEntry;
pub use manager::CteCacheManager;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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

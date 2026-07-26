//! Cache Module Tests
//!
//! Test coverage:
//! - basic: CacheManager and basic functionality
//! - config: Configuration validation and presets
//! - stats: Statistics collection and reporting
//! - plan_cache: Query plan cache functionality
//! - cte_cache: CTE result cache functionality
//! - invalidation: Cache invalidation strategies
//! - warmup: Cache warmup functionality
//! - concurrent: Concurrency and thread safety

#[path = "cache/basic.rs"]
pub mod basic;
#[path = "cache/concurrent.rs"]
pub mod concurrent;
#[path = "cache/config.rs"]
pub mod config;
#[path = "cache/cte_cache.rs"]
pub mod cte_cache;
#[path = "cache/invalidation.rs"]
pub mod invalidation;
#[path = "cache/plan_cache.rs"]
pub mod plan_cache;
#[path = "cache/stats.rs"]
pub mod stats;
#[path = "cache/warmup.rs"]
pub mod warmup;

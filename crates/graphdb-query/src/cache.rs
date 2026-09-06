//! Query Cache Module
//!
//! Provide a unified cache management function, including:
//! Query plan cache (Prepared Statement)
//! CTE result caching
//!
//! # Module Structure
//!
//! - `config`: Configuration for the plan and CTE caches
//! - `stats`: Statistics collection and reporting
//! - `plan_cache`: Query plan cache (Prepared Statement style)
//! - `cte_cache`: CTE result cache

// Submodules
pub mod config;
pub mod cte_cache;
pub mod plan_cache;
pub mod stats;

// Re-export config types
pub use config::{CachePriority, CteCacheConfig, PlanCacheConfig};

// Re-export stats types
pub use stats::{
    CacheStats, CteCacheStats, CteCacheStatsSnapshot, MemoryStats, PlanCacheStats,
    PlanCacheStatsSnapshot,
};

// Re-export the plan cache types
pub use plan_cache::{
    CachedPlan, ParamPosition, ParameterizedQueryHandler, PlanCacheContext, PlanCacheKey,
    QueryPlanCache,
};

// Re-export the CTE cache types
pub use cte_cache::{CteCacheDecision, CteCacheDecisionMaker, CteCacheEntry, CteCacheManager};

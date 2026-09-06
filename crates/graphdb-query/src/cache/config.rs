//! Unified Cache Configuration Module
//!
//! Provides centralized configuration for all cache types.

use serde::{Deserialize, Serialize};

/// Cache priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum CachePriority {
    Low = 0,
    #[default]
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// TTL configuration for cache entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtlConfig {
    /// Base TTL in seconds
    pub base_ttl_seconds: u64,
    /// Enable adaptive TTL based on access patterns
    pub adaptive: bool,
    /// Minimum TTL in seconds
    pub min_ttl_seconds: u64,
    /// Maximum TTL in seconds
    pub max_ttl_seconds: u64,
}

impl Default for TtlConfig {
    fn default() -> Self {
        Self {
            base_ttl_seconds: 3600,
            adaptive: true,
            min_ttl_seconds: 300,
            max_ttl_seconds: 86400,
        }
    }
}

/// Priority configuration for cache entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityConfig {
    /// Enable priority-based eviction
    pub enable_priority: bool,
    /// Track execution time for priority calculation
    pub track_execution_time: bool,
}

impl Default for PriorityConfig {
    fn default() -> Self {
        Self {
            enable_priority: true,
            track_execution_time: true,
        }
    }
}

/// Query Plan Cache Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanCacheConfig {
    /// Maximum number of cache entries
    pub max_entries: usize,
    /// Memory budget (bytes)
    pub memory_budget: usize,
    /// Maximum weight (bytes), takes precedence over max_entries
    /// If None, use memory_budget
    pub max_weight: Option<u64>,
    /// Whether to enable parameterized query support
    pub enable_parameterized: bool,
    /// TTL configuration
    pub ttl_config: TtlConfig,
    /// Priority configuration
    pub priority_config: PriorityConfig,
}

impl Default for PlanCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            memory_budget: 50 * 1024 * 1024,
            max_weight: None,
            enable_parameterized: true,
            ttl_config: TtlConfig::default(),
            priority_config: PriorityConfig::default(),
        }
    }
}

impl PlanCacheConfig {
    /// Create a minimal configuration for embedded environments
    pub fn minimal() -> Self {
        Self {
            max_entries: 200,
            memory_budget: 10 * 1024 * 1024,
            max_weight: None,
            enable_parameterized: true,
            ttl_config: TtlConfig {
                base_ttl_seconds: 1800,
                adaptive: true,
                min_ttl_seconds: 60,
                max_ttl_seconds: 3600,
            },
            priority_config: PriorityConfig::default(),
        }
    }

    /// Create a high-performance configuration for server environments
    pub fn high_performance() -> Self {
        Self {
            max_entries: 5000,
            memory_budget: 200 * 1024 * 1024,
            max_weight: None,
            enable_parameterized: true,
            ttl_config: TtlConfig {
                base_ttl_seconds: 7200,
                adaptive: true,
                min_ttl_seconds: 600,
                max_ttl_seconds: 86400,
            },
            priority_config: PriorityConfig::default(),
        }
    }

    /// Get effective max weight
    pub fn effective_max_weight(&self) -> u64 {
        self.max_weight.unwrap_or(self.memory_budget as u64)
    }
}

/// CTE Cache Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CteCacheConfig {
    /// Maximum cache size (bytes)
    pub max_size: usize,
    /// Maximum number of entries (optional)
    pub max_entries: Option<usize>,
    /// Maximum size of a single entry (bytes)
    pub max_entry_size: usize,
    /// Minimum number of rows to cache (less than this value is not cached)
    pub min_row_count: u64,
    /// Maximum number of rows to cache (greater than this value is not cached)
    pub max_row_count: u64,
    /// Entry expiration time (seconds)
    pub entry_ttl_seconds: u64,
    /// Enable caching
    pub enabled: bool,
    /// Whether to enable adaptive caching
    pub adaptive: bool,
    /// Whether to enable priority
    pub enable_priority: bool,
}

impl Default for CteCacheConfig {
    fn default() -> Self {
        Self {
            max_size: 64 * 1024 * 1024,
            max_entries: Some(10000),
            max_entry_size: 10 * 1024 * 1024,
            min_row_count: 100,
            max_row_count: 100_000,
            entry_ttl_seconds: 3600,
            enabled: true,
            adaptive: true,
            enable_priority: true,
        }
    }
}

impl CteCacheConfig {
    /// Create a small memory configuration
    pub fn low_memory() -> Self {
        Self {
            max_size: 16 * 1024 * 1024,
            max_entries: Some(5000),
            max_entry_size: 5 * 1024 * 1024,
            min_row_count: 50,
            max_row_count: 50_000,
            entry_ttl_seconds: 1800,
            enabled: true,
            adaptive: true,
            enable_priority: true,
        }
    }

    /// Create a large memory configuration
    pub fn high_memory() -> Self {
        Self {
            max_size: 256 * 1024 * 1024,
            max_entries: Some(20000),
            max_entry_size: 50 * 1024 * 1024,
            min_row_count: 100,
            max_row_count: 500_000,
            entry_ttl_seconds: 7200,
            enabled: true,
            adaptive: true,
            enable_priority: true,
        }
    }

    /// Disable caching
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ttl_config_default() {
        let config = TtlConfig::default();
        assert_eq!(config.base_ttl_seconds, 3600);
        assert!(config.adaptive);
    }

    #[test]
    fn test_plan_cache_config_default() {
        let config = PlanCacheConfig::default();
        assert_eq!(config.max_entries, 1000);
        assert_eq!(config.memory_budget, 50 * 1024 * 1024);
    }

    #[test]
    fn test_cte_cache_config_default() {
        let config = CteCacheConfig::default();
        assert_eq!(config.max_size, 64 * 1024 * 1024);
        assert!(config.enabled);
    }
}

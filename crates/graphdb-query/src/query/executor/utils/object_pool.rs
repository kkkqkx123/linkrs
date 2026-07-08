//! Object Pool Module
//!
//! Provide an executor object pool to reduce the frequent allocation and release of memory.
//! Improving the performance of query execution

use crate::query::executor::base::ExecutorEnum;
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::mem;
use std::sync::Arc;

/// Object pool configuration
#[derive(Debug, Clone)]
pub struct ObjectPoolConfig {
    /// Is the object pool enabled?
    pub enabled: bool,
    /// Default pool size for types without specific configuration
    pub default_pool_size: usize,
    /// Total memory budget in bytes
    pub memory_budget: usize,
    /// Whether to enable warmup
    pub enable_warmup: bool,
    /// Configuration per executor type
    pub type_configs: HashMap<String, TypePoolConfig>,
    /// Whether to enable adaptive adjustment
    pub enable_adaptive: bool,
    /// Adaptive adjustment interval in seconds
    pub adaptive_interval_secs: u64,
}

/// Pool priority for eviction decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PoolPriority {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3,
}

/// Configuration for a specific executor type
#[derive(Debug, Clone)]
pub struct TypePoolConfig {
    /// Maximum pool size for this type
    pub max_size: usize,
    /// Priority for eviction decisions
    pub priority: PoolPriority,
    /// Number of instances to warmup
    pub warmup_count: usize,
}

impl Default for ObjectPoolConfig {
    fn default() -> Self {
        let mut type_configs = HashMap::new();

        type_configs.insert(
            "FilterExecutor".to_string(),
            TypePoolConfig {
                max_size: 50,
                priority: PoolPriority::High,
                warmup_count: 10,
            },
        );
        type_configs.insert(
            "ProjectExecutor".to_string(),
            TypePoolConfig {
                max_size: 50,
                priority: PoolPriority::High,
                warmup_count: 10,
            },
        );
        type_configs.insert(
            "ScanVerticesExecutor".to_string(),
            TypePoolConfig {
                max_size: 20,
                priority: PoolPriority::Medium,
                warmup_count: 5,
            },
        );
        type_configs.insert(
            "GetNeighborsExecutor".to_string(),
            TypePoolConfig {
                max_size: 20,
                priority: PoolPriority::Medium,
                warmup_count: 5,
            },
        );
        type_configs.insert(
            "AggregateExecutor".to_string(),
            TypePoolConfig {
                max_size: 5,
                priority: PoolPriority::Low,
                warmup_count: 2,
            },
        );

        Self {
            enabled: true,
            default_pool_size: 10,
            memory_budget: 64 * 1024 * 1024,
            enable_warmup: true,
            type_configs,
            enable_adaptive: true,
            adaptive_interval_secs: 60,
        }
    }
}

impl ObjectPoolConfig {
    /// Minimal configuration for embedded/low-memory environments
    pub fn minimal() -> Self {
        let default_config = Self::default();
        let mut type_configs = default_config.type_configs.clone();

        for type_config in type_configs.values_mut() {
            type_config.max_size /= 2;
            type_config.warmup_count = 0;
        }

        Self {
            enabled: true,
            default_pool_size: 5,
            memory_budget: 16 * 1024 * 1024,
            enable_warmup: false,
            type_configs,
            enable_adaptive: false,
            adaptive_interval_secs: default_config.adaptive_interval_secs,
        }
    }

    /// High concurrency configuration for high-performance servers
    pub fn high_concurrency() -> Self {
        let mut type_configs = HashMap::new();

        type_configs.insert(
            "FilterExecutor".to_string(),
            TypePoolConfig {
                max_size: 100,
                priority: PoolPriority::High,
                warmup_count: 20,
            },
        );
        type_configs.insert(
            "ProjectExecutor".to_string(),
            TypePoolConfig {
                max_size: 100,
                priority: PoolPriority::High,
                warmup_count: 20,
            },
        );
        type_configs.insert(
            "ScanVerticesExecutor".to_string(),
            TypePoolConfig {
                max_size: 20,
                priority: PoolPriority::Medium,
                warmup_count: 5,
            },
        );
        type_configs.insert(
            "GetNeighborsExecutor".to_string(),
            TypePoolConfig {
                max_size: 20,
                priority: PoolPriority::Medium,
                warmup_count: 5,
            },
        );
        type_configs.insert(
            "AggregateExecutor".to_string(),
            TypePoolConfig {
                max_size: 5,
                priority: PoolPriority::Low,
                warmup_count: 2,
            },
        );

        Self {
            enabled: true,
            default_pool_size: 20,
            memory_budget: 256 * 1024 * 1024,
            enable_warmup: true,
            type_configs,
            enable_adaptive: true,
            adaptive_interval_secs: 30,
        }
    }

    /// Disable object pool
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Validate configuration and return errors if invalid
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled {
            if self.memory_budget == 0 {
                return Err("memory_budget must be greater than 0 when pool is enabled".to_string());
            }
            if self.default_pool_size == 0 {
                return Err(
                    "default_pool_size must be greater than 0 when pool is enabled".to_string(),
                );
            }

            for (type_name, type_config) in &self.type_configs {
                if type_config.max_size == 0 {
                    return Err(format!("max_size for {} must be greater than 0", type_name));
                }
            }
        }

        Ok(())
    }
}

/// Object pool: A cache for executor instances
///
/// Reuse executor instances by using the object pool pattern to reduce the overhead associated with memory allocation.
pub struct ExecutorObjectPool<S: StorageClient + 'static> {
    config: ObjectPoolConfig,
    pools: HashMap<String, Vec<ExecutorEnum<S>>>,
    stats: PoolStats,
    current_memory: usize,
    type_sizes: HashMap<String, usize>,
}

/// Statistics for a specific executor type pool
#[derive(Debug, Clone, Default)]
pub struct TypePoolStats {
    pub current_size: usize,
    pub max_size: usize,
    pub hits: usize,
    pub misses: usize,
    pub estimated_memory: usize,
}

/// Object pool statistics
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    /// Total number of acquisitions
    pub total_acquires: usize,
    /// Total number of releases
    pub total_releases: usize,
    /// Number of cache hits
    pub cache_hits: usize,
    /// Number of cache misses
    pub cache_misses: usize,
    /// Number of discarded executors due to memory limit
    pub memory_discarded: usize,
    /// Number of discarded executors due to pool full
    pub pool_full_discarded: usize,
    /// Number of evicted executors
    pub evicted: usize,
    /// Current memory usage in bytes
    pub current_memory: usize,
    /// Memory budget in bytes
    pub memory_budget: usize,
    /// Statistics per executor type
    pub type_stats: HashMap<String, TypePoolStats>,
}

impl PoolStats {
    pub fn hit_rate(&self) -> f64 {
        if self.total_acquires == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_acquires as f64
        }
    }
}

impl<S: StorageClient + 'static> ExecutorObjectPool<S> {
    /// Create a new object pool.
    pub fn new(config: ObjectPoolConfig) -> Self {
        if let Err(e) = config.validate() {
            log::warn!("Invalid object pool config: {}", e);
        }

        let memory_budget = config.memory_budget;
        Self {
            config,
            pools: HashMap::new(),
            stats: PoolStats {
                memory_budget,
                ..Default::default()
            },
            current_memory: 0,
            type_sizes: HashMap::new(),
        }
    }

    /// Create an object pool with default configuration.
    pub fn default_pool() -> Self {
        Self::new(ObjectPoolConfig::default())
    }

    /// Get maximum pool size for a specific executor type
    fn get_max_size(&self, executor_type: &str) -> usize {
        self.config
            .type_configs
            .get(executor_type)
            .map(|cfg| cfg.max_size)
            .unwrap_or(self.config.default_pool_size)
    }

    /// Get priority for a specific executor type
    fn get_priority(&self, executor_type: &str) -> PoolPriority {
        self.config
            .type_configs
            .get(executor_type)
            .map(|cfg| cfg.priority)
            .unwrap_or(PoolPriority::Medium)
    }

    /// Estimate size of an executor
    fn estimate_size(&self, executor: &ExecutorEnum<S>) -> usize {
        mem::size_of_val(executor)
    }

    /// Evict executors to free memory
    fn evict_for_memory(&mut self, required_size: usize) -> bool {
        let mut freed = 0;

        let mut types: Vec<_> = self.pools.keys().cloned().collect();
        types.sort_by_key(|t| self.get_priority(t));

        for type_name in types {
            if freed >= required_size {
                break;
            }

            if let Some(pool) = self.pools.get_mut(&type_name) {
                while let Some(executor) = pool.pop() {
                    let size = mem::size_of_val(&executor);
                    freed += size;
                    self.stats.evicted += 1;

                    if let Some(type_stats) = self.stats.type_stats.get_mut(&type_name) {
                        type_stats.current_size = pool.len();
                        type_stats.estimated_memory =
                            type_stats.estimated_memory.saturating_sub(size);
                    }

                    if freed >= required_size {
                        break;
                    }
                }
            }
        }

        self.current_memory = self.current_memory.saturating_sub(freed);
        self.stats.current_memory = self.current_memory;
        freed >= required_size
    }

    /// Obtain an executor from the object pool.
    ///
    /// If there are available executors in the pool, the cached instance is returned.
    /// Otherwise, return `None`. The caller will need to create a new instance.
    pub fn acquire(&mut self, executor_type: &str) -> Option<ExecutorEnum<S>> {
        if !self.config.enabled {
            return None;
        }

        self.stats.total_acquires += 1;

        if let Some(executors) = self.pools.get_mut(executor_type) {
            if let Some(executor) = executors.pop() {
                self.stats.cache_hits += 1;

                let size = self.type_sizes.get(executor_type).copied().unwrap_or(0);
                self.current_memory = self.current_memory.saturating_sub(size);
                self.stats.current_memory = self.current_memory;

                if let Some(type_stats) = self.stats.type_stats.get_mut(executor_type) {
                    type_stats.current_size = executors.len();
                    type_stats.hits += 1;
                    type_stats.estimated_memory = type_stats.estimated_memory.saturating_sub(size);
                }

                return Some(executor);
            }
        }

        self.stats.cache_misses += 1;

        if let Some(type_stats) = self.stats.type_stats.get_mut(executor_type) {
            type_stats.misses += 1;
        }

        None
    }

    /// Release the executor back to the object pool.
    ///
    /// If the pool is not full, the executor will be returned to the pool.
    /// Otherwise, discard the executor.
    pub fn release(&mut self, executor_type: &str, executor: ExecutorEnum<S>) {
        if !self.config.enabled {
            return;
        }

        self.stats.total_releases += 1;

        let size = self.estimate_size(&executor);

        if self.current_memory + size > self.config.memory_budget && !self.evict_for_memory(size) {
            self.stats.memory_discarded += 1;
            return;
        }

        let max_size = self.get_max_size(executor_type);
        let pool = self.pools.entry(executor_type.to_string()).or_default();

        if pool.len() < max_size {
            pool.push(executor);
            self.current_memory += size;
            self.stats.current_memory = self.current_memory;
            self.type_sizes.insert(executor_type.to_string(), size);

            let type_stats = self
                .stats
                .type_stats
                .entry(executor_type.to_string())
                .or_insert_with(|| TypePoolStats {
                    max_size,
                    ..Default::default()
                });
            type_stats.current_size = pool.len();
            type_stats.estimated_memory += size;
        } else {
            self.stats.pool_full_discarded += 1;
        }
    }

    /// Clear the object pool.
    pub fn clear(&mut self) {
        self.pools.clear();
        self.current_memory = 0;
        self.stats.current_memory = 0;
        self.stats.type_stats.clear();
    }

    /// Obtain object pool statistics information
    pub fn stats(&self) -> &PoolStats {
        &self.stats
    }

    /// Obtaining the object pool configuration
    pub fn config(&self) -> &ObjectPoolConfig {
        &self.config
    }

    /// Update the object pool configuration.
    pub fn set_config(&mut self, config: ObjectPoolConfig) {
        self.config = config;
    }

    /// Obtain the pool size of the specified type.
    pub fn pool_size(&self, executor_type: &str) -> usize {
        self.pools
            .get(executor_type)
            .map(|pool| pool.len())
            .unwrap_or(0)
    }

    /// Obtain the total size of the pool.
    pub fn total_size(&self) -> usize {
        self.pools.values().map(|pool| pool.len()).sum()
    }
}

/// Object pool wrapper – Provides a thread-safe object pool.
pub struct ThreadSafeExecutorPool<S: StorageClient + 'static> {
    inner: Arc<RwLock<ExecutorObjectPool<S>>>,
}

impl<S: StorageClient + 'static> ThreadSafeExecutorPool<S> {
    /// Create a new thread-safe object pool.
    pub fn new(config: ObjectPoolConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ExecutorObjectPool::new(config))),
        }
    }

    /// Create a thread-safe object pool with default configuration.
    pub fn default_pool() -> Self {
        Self::new(ObjectPoolConfig::default())
    }

    /// Get the executor from the object pool.
    pub fn acquire(&self, executor_type: &str) -> Option<ExecutorEnum<S>> {
        let mut pool = self.inner.write();
        pool.acquire(executor_type)
    }

    /// Release the executor back to the object pool.
    pub fn release(&self, executor_type: &str, executor: ExecutorEnum<S>) {
        let mut pool = self.inner.write();
        pool.release(executor_type, executor);
    }

    /// Clear the object pool.
    pub fn clear(&self) {
        let mut pool = self.inner.write();
        pool.clear();
    }

    /// Obtain object pool statistics information
    pub fn stats(&self) -> PoolStats {
        let pool = self.inner.write();
        pool.stats().clone()
    }

    /// Obtain object pool configuration
    pub fn config(&self) -> ObjectPoolConfig {
        let pool = self.inner.write();
        pool.config().clone()
    }

    /// Update the object pool configuration.
    pub fn set_config(&self, config: ObjectPoolConfig) {
        let mut pool = self.inner.write();
        pool.set_config(config);
    }

    /// Get the pool size of the specified type
    pub fn pool_size(&self, executor_type: &str) -> usize {
        let pool = self.inner.write();
        pool.pool_size(executor_type)
    }

    /// Get the total pool size
    pub fn total_size(&self) -> usize {
        let pool = self.inner.write();
        pool.total_size()
    }
}

impl<S: StorageClient + 'static> Clone for ThreadSafeExecutorPool<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MockStorage;

    #[test]
    fn test_object_pool_config_default() {
        let config = ObjectPoolConfig::default();
        assert!(config.enabled);
        assert_eq!(config.default_pool_size, 10);
        assert_eq!(config.memory_budget, 64 * 1024 * 1024);
        assert!(config.enable_warmup);
        assert!(config.enable_adaptive);
    }

    #[test]
    fn test_object_pool_config_minimal() {
        let config = ObjectPoolConfig::minimal();
        assert!(config.enabled);
        assert_eq!(config.default_pool_size, 5);
        assert_eq!(config.memory_budget, 16 * 1024 * 1024);
        assert!(!config.enable_warmup);
        assert!(!config.enable_adaptive);
    }

    #[test]
    fn test_object_pool_config_high_concurrency() {
        let config = ObjectPoolConfig::high_concurrency();
        assert!(config.enabled);
        assert_eq!(config.default_pool_size, 20);
        assert_eq!(config.memory_budget, 256 * 1024 * 1024);
        assert!(config.enable_adaptive);
        assert_eq!(config.adaptive_interval_secs, 30);
    }

    #[test]
    fn test_object_pool_config_disabled() {
        let config = ObjectPoolConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_pool_priority_ordering() {
        assert!(PoolPriority::Low < PoolPriority::Medium);
        assert!(PoolPriority::Medium < PoolPriority::High);
        assert!(PoolPriority::High < PoolPriority::Critical);
    }

    #[test]
    fn test_object_pool_creation() {
        let pool = ExecutorObjectPool::<MockStorage>::default_pool();
        assert_eq!(pool.total_size(), 0);
        assert_eq!(pool.stats().current_memory, 0);
    }

    #[test]
    fn test_object_pool_acquire_empty() {
        let mut pool = ExecutorObjectPool::<MockStorage>::default_pool();
        let executor = pool.acquire("TestExecutor");
        assert!(executor.is_none());
        assert_eq!(pool.stats().cache_misses, 1);
    }

    #[test]
    fn test_object_pool_release_and_acquire() {
        let pool = ExecutorObjectPool::<MockStorage>::default_pool();
        assert_eq!(pool.pool_size("TestExecutor"), 0);
    }

    #[test]
    fn test_thread_safe_pool() {
        let pool = ThreadSafeExecutorPool::<MockStorage>::default_pool();
        assert_eq!(pool.total_size(), 0);

        let executor = pool.acquire("TestExecutor");
        assert!(executor.is_none());
    }

    #[test]
    fn test_pool_stats() {
        let mut pool = ExecutorObjectPool::<MockStorage>::default_pool();
        pool.acquire("TestExecutor");
        pool.acquire("TestExecutor");

        assert_eq!(pool.stats().total_acquires, 2);
        assert_eq!(pool.stats().cache_misses, 2);
        assert_eq!(pool.stats().cache_hits, 0);
        assert_eq!(pool.stats().hit_rate(), 0.0);
    }

    #[test]
    fn test_pool_clear() {
        let mut pool = ExecutorObjectPool::<MockStorage>::default_pool();
        pool.acquire("TestExecutor");
        pool.clear();
        assert_eq!(pool.total_size(), 0);
        assert_eq!(pool.stats().current_memory, 0);
    }

    #[test]
    fn test_type_pool_config() {
        let config = ObjectPoolConfig::default();
        let filter_config = config.type_configs.get("FilterExecutor");
        assert!(filter_config.is_some());
        assert_eq!(
            filter_config
                .expect("FilterExecutor config should exist")
                .max_size,
            50
        );
        assert_eq!(
            filter_config
                .expect("FilterExecutor config should exist")
                .priority,
            PoolPriority::High
        );
    }
}

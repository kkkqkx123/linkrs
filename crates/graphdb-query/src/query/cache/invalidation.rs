//! Cache Invalidation Strategy Module
//!
//! Provides cache invalidation mechanisms to maintain data consistency
//! when underlying data changes.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Type alias for table names
pub type TableName = String;

/// Type alias for cache keys
pub type CacheKey = String;

/// Data change event types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataChangeType {
    /// Row inserted
    Insert,
    /// Row updated
    Update,
    /// Row deleted
    Delete,
    /// Schema changed (DDL)
    SchemaChange,
    /// Bulk data load
    BulkLoad,
    /// Truncate table
    Truncate,
}

/// Data change event
#[derive(Debug, Clone)]
pub struct DataChangeEvent {
    /// Table that was modified
    pub table_name: TableName,
    /// Type of change
    pub change_type: DataChangeType,
    /// Optional: affected row IDs (for selective invalidation)
    pub affected_ids: Option<Vec<String>>,
    /// Timestamp of the change
    pub timestamp: std::time::Instant,
}

impl DataChangeEvent {
    pub fn new(table_name: impl Into<String>, change_type: DataChangeType) -> Self {
        Self {
            table_name: table_name.into(),
            change_type,
            affected_ids: None,
            timestamp: std::time::Instant::now(),
        }
    }

    pub fn with_affected_ids(mut self, ids: Vec<String>) -> Self {
        self.affected_ids = Some(ids);
        self
    }

    pub fn insert(table_name: impl Into<String>) -> Self {
        Self::new(table_name, DataChangeType::Insert)
    }

    pub fn update(table_name: impl Into<String>) -> Self {
        Self::new(table_name, DataChangeType::Update)
    }

    pub fn delete(table_name: impl Into<String>) -> Self {
        Self::new(table_name, DataChangeType::Delete)
    }

    pub fn schema_change(table_name: impl Into<String>) -> Self {
        Self::new(table_name, DataChangeType::SchemaChange)
    }

    pub fn truncate(table_name: impl Into<String>) -> Self {
        Self::new(table_name, DataChangeType::Truncate)
    }
}

/// Dependency tracker for cache entries
///
/// Tracks which tables a cache entry depends on, enabling selective invalidation.
#[derive(Debug, Default)]
pub struct DependencyTracker {
    /// Map from table name to set of cache keys that depend on it
    table_to_keys: RwLock<HashMap<TableName, HashSet<CacheKey>>>,
    /// Map from cache key to set of tables it depends on
    key_to_tables: RwLock<HashMap<CacheKey, HashSet<TableName>>>,
}

impl DependencyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a dependency: cache key depends on a table
    pub fn register_dependency(&self, cache_key: CacheKey, table_name: TableName) {
        self.table_to_keys
            .write()
            .entry(table_name.clone())
            .or_default()
            .insert(cache_key.clone());

        self.key_to_tables
            .write()
            .entry(cache_key)
            .or_default()
            .insert(table_name);
    }

    /// Register multiple dependencies for a cache key
    pub fn register_dependencies(&self, cache_key: CacheKey, tables: Vec<TableName>) {
        for table in tables {
            self.register_dependency(cache_key.clone(), table);
        }
    }

    /// Get all cache keys that depend on a table
    pub fn get_dependent_keys(&self, table_name: &str) -> HashSet<CacheKey> {
        self.table_to_keys
            .read()
            .get(table_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Get all tables a cache key depends on
    pub fn get_dependencies(&self, cache_key: &str) -> HashSet<TableName> {
        self.key_to_tables
            .read()
            .get(cache_key)
            .cloned()
            .unwrap_or_default()
    }

    /// Remove a cache key from tracking
    pub fn remove_key(&self, cache_key: &str) {
        if let Some(tables) = self.key_to_tables.write().remove(cache_key) {
            let mut table_to_keys = self.table_to_keys.write();
            for table in tables {
                if let Some(keys) = table_to_keys.get_mut(&table) {
                    keys.remove(cache_key);
                    if keys.is_empty() {
                        table_to_keys.remove(&table);
                    }
                }
            }
        }
    }

    /// Clear all dependencies
    pub fn clear(&self) {
        self.table_to_keys.write().clear();
        self.key_to_tables.write().clear();
    }

    /// Get total number of tracked cache keys
    pub fn key_count(&self) -> usize {
        self.key_to_tables.read().len()
    }

    /// Get total number of tracked tables
    pub fn table_count(&self) -> usize {
        self.table_to_keys.read().len()
    }
}

/// Invalidation strategy trait
pub trait InvalidationStrategy: Send + Sync {
    /// Determine if a cache entry should be invalidated based on the change event
    fn should_invalidate(&self, event: &DataChangeEvent, cache_key: &str) -> bool;

    /// Get the strategy name for logging/debugging
    fn name(&self) -> &'static str;
}

/// Simple table-based invalidation strategy
///
/// Invalidates all cache entries that depend on a modified table.
#[derive(Debug, Default)]
pub struct TableBasedInvalidation {
    tracker: DependencyTracker,
}

impl TableBasedInvalidation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tracker(&self) -> &DependencyTracker {
        &self.tracker
    }

    pub fn register_dependency(&self, cache_key: CacheKey, table_name: TableName) {
        self.tracker.register_dependency(cache_key, table_name);
    }

    pub fn get_keys_to_invalidate(&self, event: &DataChangeEvent) -> HashSet<CacheKey> {
        self.tracker.get_dependent_keys(&event.table_name)
    }
}

impl InvalidationStrategy for TableBasedInvalidation {
    fn should_invalidate(&self, event: &DataChangeEvent, cache_key: &str) -> bool {
        self.tracker
            .get_dependent_keys(&event.table_name)
            .contains(cache_key)
    }

    fn name(&self) -> &'static str {
        "table_based"
    }
}

/// Time-based invalidation strategy
///
/// Invalidates cache entries based on time-to-live (TTL).
/// This is typically handled by the cache implementation itself.
#[derive(Debug)]
pub struct TimeBasedInvalidation {
    default_ttl_seconds: u64,
}

impl TimeBasedInvalidation {
    pub fn new(default_ttl_seconds: u64) -> Self {
        Self {
            default_ttl_seconds,
        }
    }

    pub fn default_ttl(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.default_ttl_seconds)
    }
}

impl Default for TimeBasedInvalidation {
    fn default() -> Self {
        Self::new(3600)
    }
}

impl InvalidationStrategy for TimeBasedInvalidation {
    fn should_invalidate(&self, _event: &DataChangeEvent, _cache_key: &str) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "time_based"
    }
}

/// Composite invalidation strategy
///
/// Combines multiple strategies, invalidating if any strategy says to invalidate.
pub struct CompositeInvalidation {
    strategies: Vec<Box<dyn InvalidationStrategy>>,
}

impl std::fmt::Debug for CompositeInvalidation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeInvalidation")
            .field("strategies_count", &self.strategies.len())
            .finish()
    }
}

impl CompositeInvalidation {
    pub fn new() -> Self {
        Self {
            strategies: Vec::new(),
        }
    }

    pub fn with_strategy(mut self, strategy: Box<dyn InvalidationStrategy>) -> Self {
        self.strategies.push(strategy);
        self
    }

    pub fn add_strategy(&mut self, strategy: Box<dyn InvalidationStrategy>) {
        self.strategies.push(strategy);
    }
}

impl Default for CompositeInvalidation {
    fn default() -> Self {
        Self::new()
    }
}

impl InvalidationStrategy for CompositeInvalidation {
    fn should_invalidate(&self, event: &DataChangeEvent, cache_key: &str) -> bool {
        self.strategies
            .iter()
            .any(|s| s.should_invalidate(event, cache_key))
    }

    fn name(&self) -> &'static str {
        "composite"
    }
}

/// Cache invalidator trait for cache implementations
pub trait CacheInvalidator: Send + Sync {
    /// Invalidate a specific cache entry by key
    fn invalidate(&self, key: &str) -> bool;

    /// Invalidate multiple cache entries
    fn invalidate_many(&self, keys: &[String]) -> usize {
        keys.iter().filter(|k| self.invalidate(k)).count()
    }

    /// Invalidate all cache entries
    fn invalidate_all(&self);

    /// Get the number of entries invalidated
    fn invalidated_count(&self) -> u64;
}

/// Invalidation manager that coordinates cache invalidation
pub struct InvalidationManager {
    /// Table-based invalidation strategy
    table_strategy: TableBasedInvalidation,
    /// Registered invalidators for different cache types
    invalidators: RwLock<Vec<Arc<dyn CacheInvalidator>>>,
    /// Statistics
    stats: RwLock<InvalidationStats>,
}

impl std::fmt::Debug for InvalidationManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvalidationManager")
            .field("table_strategy", &self.table_strategy)
            .field("invalidators_count", &self.invalidators.read().len())
            .field("stats", &*self.stats.read())
            .finish()
    }
}

#[derive(Debug, Clone, Default)]
pub struct InvalidationStats {
    pub total_invalidations: u64,
    pub table_invalidations: u64,
    pub full_invalidations: u64,
}

impl InvalidationManager {
    pub fn new() -> Self {
        Self {
            table_strategy: TableBasedInvalidation::new(),
            invalidators: RwLock::new(Vec::new()),
            stats: RwLock::new(InvalidationStats::default()),
        }
    }

    /// Register a cache invalidator
    pub fn register_invalidator(&self, invalidator: Arc<dyn CacheInvalidator>) {
        self.invalidators.write().push(invalidator);
    }

    /// Register a table dependency for a cache key
    pub fn register_dependency(&self, cache_key: CacheKey, table_name: TableName) {
        self.table_strategy
            .register_dependency(cache_key, table_name);
    }

    /// Register multiple table dependencies for a cache key
    pub fn register_dependencies(&self, cache_key: CacheKey, tables: Vec<TableName>) {
        self.table_strategy
            .tracker()
            .register_dependencies(cache_key, tables);
    }

    /// Handle a data change event
    pub fn on_data_change(&self, event: &DataChangeEvent) {
        log::debug!(
            "Processing data change event: table={}, type={:?}",
            event.table_name,
            event.change_type
        );

        match event.change_type {
            DataChangeType::SchemaChange | DataChangeType::Truncate => {
                self.invalidate_all_caches();
                let mut stats = self.stats.write();
                stats.full_invalidations += 1;
                stats.total_invalidations += 1;
            }
            _ => {
                let keys = self.table_strategy.get_keys_to_invalidate(event);
                if !keys.is_empty() {
                    self.invalidate_keys(&keys);
                    let mut stats = self.stats.write();
                    stats.table_invalidations += 1;
                    stats.total_invalidations += keys.len() as u64;
                }
            }
        }
    }

    /// Invalidate specific cache keys across all registered caches
    fn invalidate_keys(&self, keys: &HashSet<CacheKey>) {
        let invalidators = self.invalidators.read();
        for invalidator in invalidators.iter() {
            for key in keys {
                invalidator.invalidate(key);
            }
        }

        for key in keys {
            self.table_strategy.tracker().remove_key(key);
        }
    }

    /// Invalidate all caches
    fn invalidate_all_caches(&self) {
        let invalidators = self.invalidators.read();
        for invalidator in invalidators.iter() {
            invalidator.invalidate_all();
        }

        self.table_strategy.tracker().clear();
    }

    /// Get current statistics
    pub fn stats(&self) -> InvalidationStats {
        self.stats.read().clone()
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        *self.stats.write() = InvalidationStats::default();
    }

    /// Get the dependency tracker
    pub fn dependency_tracker(&self) -> &DependencyTracker {
        self.table_strategy.tracker()
    }
}

impl Default for InvalidationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_change_event() {
        let event = DataChangeEvent::insert("users");
        assert_eq!(event.table_name, "users");
        assert_eq!(event.change_type, DataChangeType::Insert);
        assert!(event.affected_ids.is_none());
    }

    #[test]
    fn test_data_change_event_with_ids() {
        let event =
            DataChangeEvent::update("users").with_affected_ids(vec!["1".into(), "2".into()]);
        assert_eq!(event.table_name, "users");
        assert_eq!(event.change_type, DataChangeType::Update);
        assert_eq!(event.affected_ids.as_ref().map(|v| v.len()), Some(2));
    }

    #[test]
    fn test_dependency_tracker() {
        let tracker = DependencyTracker::new();

        tracker.register_dependency("key1".into(), "users".into());
        tracker.register_dependency("key2".into(), "users".into());
        tracker.register_dependency("key1".into(), "posts".into());

        let users_deps = tracker.get_dependent_keys("users");
        assert_eq!(users_deps.len(), 2);
        assert!(users_deps.contains("key1"));
        assert!(users_deps.contains("key2"));

        let key1_tables = tracker.get_dependencies("key1");
        assert_eq!(key1_tables.len(), 2);
        assert!(key1_tables.contains("users"));
        assert!(key1_tables.contains("posts"));
    }

    #[test]
    fn test_dependency_tracker_remove() {
        let tracker = DependencyTracker::new();
        tracker.register_dependency("key1".into(), "users".into());
        tracker.remove_key("key1");

        assert!(tracker.get_dependencies("key1").is_empty());
        assert!(tracker.get_dependent_keys("users").is_empty());
    }

    #[test]
    fn test_table_based_invalidation() {
        let strategy = TableBasedInvalidation::new();
        strategy.register_dependency("key1".into(), "users".into());

        let event = DataChangeEvent::update("users");
        assert!(strategy.should_invalidate(&event, "key1"));
        assert!(!strategy.should_invalidate(&event, "key2"));

        let keys = strategy.get_keys_to_invalidate(&event);
        assert!(keys.contains("key1"));
    }

    #[test]
    fn test_invalidation_manager() {
        let manager = InvalidationManager::new();
        manager.register_dependency("key1".into(), "users".into());

        let event = DataChangeEvent::update("users");
        manager.on_data_change(&event);

        let stats = manager.stats();
        assert_eq!(stats.table_invalidations, 1);
    }

    #[test]
    fn test_invalidation_manager_schema_change() {
        let manager = InvalidationManager::new();
        manager.register_dependency("key1".into(), "users".into());

        let event = DataChangeEvent::schema_change("users");
        manager.on_data_change(&event);

        let stats = manager.stats();
        assert_eq!(stats.full_invalidations, 1);
    }
}

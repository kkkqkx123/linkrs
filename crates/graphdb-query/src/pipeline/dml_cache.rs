use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::executor::streaming::plan::PhysicalPlan;
use crate::planning::dml_shape::DML_PARAM_PREFIX;
use graphdb_core::types::DataType;
use moka::sync::Cache;

/// Key for the DML shape cache.
///
/// Combines the query text hash, parameter type signature, schema version,
/// and space name to uniquely identify a cacheable DML plan.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DmlCacheKey {
    pub query_hash: u64,
    pub param_types: Vec<DataType>,
    pub schema_version: u64,
    pub space_name: String,
}

impl DmlCacheKey {
    pub fn new(
        query_text: &str,
        params: &std::collections::HashMap<String, graphdb_core::Value>,
        schema_version: u64,
        space_name: Option<&str>,
    ) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        query_text.hash(&mut hasher);
        let query_hash = hasher.finish();

        let mut param_types: Vec<DataType> = params
            .iter()
            .filter(|(name, _)| name.starts_with(DML_PARAM_PREFIX))
            .map(|(_, value)| value.data_type())
            .collect();
        param_types.sort_by_key(|dt| format!("{:?}", dt));

        DmlCacheKey {
            query_hash,
            param_types,
            schema_version,
            space_name: space_name.unwrap_or("default").to_string(),
        }
    }
}

/// A cached DML plan entry.
#[derive(Debug, Clone)]
pub struct CachedDmlPlan {
    pub plan: Arc<PhysicalPlan>,
    pub param_sig: u64,
    pub access_count: u64,
}

/// Statistics for the DML shape cache.
#[derive(Debug, Default)]
pub struct DmlCacheStats {
    pub hot_hits: AtomicU64,
    pub cold_hits: AtomicU64,
    pub misses: AtomicU64,
    pub evictions: AtomicU64,
}

impl DmlCacheStats {
    pub fn total_hits(&self) -> u64 {
        self.hot_hits.load(Ordering::Relaxed) + self.cold_hits.load(Ordering::Relaxed)
    }

    pub fn hit_rate(&self) -> f64 {
        let hits = self.total_hits();
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

/// Multi-entry DML shape cache with hot/cold tiers.
///
/// The hot cache holds frequently accessed plans, while the cold cache
/// holds recently evicted plans for quick re-promotion.
pub struct DmlShapeCache {
    hot_cache: Cache<DmlCacheKey, CachedDmlPlan>,
    cold_cache: Cache<DmlCacheKey, CachedDmlPlan>,
    stats: DmlCacheStats,
}

impl DmlShapeCache {
    /// Create a new DML shape cache.
    ///
    /// `hot_capacity` is the number of entries in the hot cache.
    /// `cold_capacity` is the number of entries in the cold cache.
    pub fn new(hot_capacity: usize, cold_capacity: usize) -> Self {
        let hot = Cache::builder()
            .max_capacity(hot_capacity.max(1) as u64)
            .build();
        let cold = Cache::builder()
            .max_capacity(cold_capacity.max(1) as u64)
            .build();

        Self {
            hot_cache: hot,
            cold_cache: cold,
            stats: DmlCacheStats::default(),
        }
    }

    /// Create a DML shape cache with default capacities.
    pub fn with_defaults() -> Self {
        Self::new(64, 128)
    }

    /// Look up a cached DML plan.
    ///
    /// Returns the cached plan if found in hot or cold cache.
    /// Promotes cold cache hits back to the hot cache.
    pub fn get(&self, key: &DmlCacheKey) -> Option<Arc<PhysicalPlan>> {
        // Check hot cache first
        if let Some(entry) = self.hot_cache.get(key) {
            self.stats.hot_hits.fetch_add(1, Ordering::Relaxed);
            return Some(entry.plan.clone());
        }

        // Check cold cache (promote to hot on hit)
        if let Some(mut entry) = self.cold_cache.remove(key) {
            self.stats.cold_hits.fetch_add(1, Ordering::Relaxed);
            entry.access_count += 1;
            let plan = entry.plan.clone();
            self.hot_cache.insert(key.clone(), entry);
            return Some(plan);
        }

        self.stats.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert a DML plan into the hot cache.
    pub fn insert(&self, key: DmlCacheKey, plan: Arc<PhysicalPlan>, param_sig: u64) {
        let entry = CachedDmlPlan {
            plan,
            param_sig,
            access_count: 1,
        };
        self.hot_cache.insert(key, entry);
    }

    /// Evict an entry from the hot cache to the cold cache.
    pub fn evict_to_cold(&self, key: &DmlCacheKey) {
        if let Some(entry) = self.hot_cache.remove(key) {
            self.cold_cache.insert(key.clone(), entry);
            self.stats.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Invalidate all entries for a given space.
    pub fn invalidate_space(&self, space_name: &str) -> usize {
        let mut removed = 0;

        let hot_keys: Vec<DmlCacheKey> = self
            .hot_cache
            .iter()
            .filter(|(k, _)| k.space_name == space_name)
            .map(|(k, _)| (*k).clone())
            .collect();
        for key in hot_keys {
            self.hot_cache.remove(&key);
            removed += 1;
        }

        let cold_keys: Vec<DmlCacheKey> = self
            .cold_cache
            .iter()
            .filter(|(k, _)| k.space_name == space_name)
            .map(|(k, _)| (*k).clone())
            .collect();
        for key in cold_keys {
            self.cold_cache.remove(&key);
            removed += 1;
        }

        removed
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.hot_cache.invalidate_all();
        self.cold_cache.invalidate_all();
    }

    /// Get cache statistics.
    pub fn stats(&self) -> &DmlCacheStats {
        &self.stats
    }

    /// Current hot cache size.
    pub fn hot_len(&self) -> usize {
        self.hot_cache.iter().count()
    }

    /// Current cold cache size.
    pub fn cold_len(&self) -> usize {
        self.cold_cache.iter().count()
    }
}

impl Default for DmlShapeCache {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dml_cache_key_creation() {
        let mut params = std::collections::HashMap::new();
        params.insert(
            format!("{}0", DML_PARAM_PREFIX),
            graphdb_core::Value::String("test".to_string().into()),
        );

        let key1 = DmlCacheKey::new("INSERT VERTEX", &params, 1, Some("test_space"));
        let key2 = DmlCacheKey::new("INSERT VERTEX", &params, 1, Some("test_space"));
        let key3 = DmlCacheKey::new("INSERT VERTEX", &params, 2, Some("test_space"));

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_dml_shape_cache_invalidate_space() {
        let cache = DmlShapeCache::new(4, 4);
        let removed = cache.invalidate_space("test_space");
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_dml_shape_cache_clear() {
        let cache = DmlShapeCache::new(4, 4);
        cache.clear();
        assert_eq!(cache.hot_len(), 0);
        assert_eq!(cache.cold_len(), 0);
    }

    #[test]
    fn test_dml_shape_cache_stats() {
        let cache = DmlShapeCache::new(4, 4);
        let stats = cache.stats();
        assert_eq!(stats.total_hits(), 0);
        assert_eq!(stats.hit_rate(), 0.0);
    }
}

use std::sync::Arc;

use moka::sync::Cache;
use parking_lot::Mutex;

use crate::core::stats::CacheStats;
use crate::core::types::Timestamp;

use super::config::*;
use super::types::*;

/// Record cache for vertex data and ID index mappings.
///
/// Backed by two independent Moka caches with weight-based eviction.
/// Moka's `max_capacity` is set at build time and cannot be changed at runtime.
///
/// ## Timestamp validation
/// Both `get_vertex` and `get_id_index` accept a `query_ts` parameter.
/// Entries with `cached_at_ts > query_ts` are treated as misses to prevent
/// serving data from the future in MVCC time-travel queries.
pub struct RecordCache {
    vertex_cache: Cache<VertexCacheKey, CachedVertex>,
    id_index_cache: Cache<IdIndexCacheKey, IdIndexCacheValue>,
    config: RecordCacheConfig,
    vertex_stats: Arc<CacheStats>,
    id_index_stats: Arc<CacheStats>,
    eviction_callback: Arc<Mutex<Option<EvictionCallback>>>,
}

impl std::fmt::Debug for RecordCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordCache")
            .field("config", &self.config)
            .field("vertex_count", &self.vertex_cache.entry_count())
            .field("id_index_count", &self.id_index_cache.entry_count())
            .field("vertex_stats", &self.vertex_stats)
            .field("id_index_stats", &self.id_index_stats)
            .finish()
    }
}

impl RecordCache {
    pub fn new() -> Self {
        Self::with_config(RecordCacheConfig::default())
    }

    pub fn with_config(config: RecordCacheConfig) -> Self {
        let max_memory = config.max_memory as u64;
        let total_ratio = config.memory_ratio.0 + config.memory_ratio.1;

        let base_vertex_memory = max_memory * config.memory_ratio.0 as u64 / total_ratio as u64;
        let base_id_index_memory = max_memory * config.memory_ratio.1 as u64 / total_ratio as u64;

        let high_priority_extra = if config.high_priority_ratio > 0.0 {
            (max_memory as f64 * config.high_priority_ratio as f64) as u64
        } else {
            0
        };

        let vertex_memory = base_vertex_memory;
        let id_index_memory = base_id_index_memory + high_priority_extra;

        let vertex_stats = Arc::new(CacheStats::new());
        let id_index_stats = Arc::new(CacheStats::new());

        let eviction_callback = Arc::new(Mutex::new(None::<EvictionCallback>));

        let vertex_cache = Self::build_vertex_cache(
            vertex_memory,
            vertex_stats.clone(),
            eviction_callback.clone(),
            config.ttl,
            config.tti,
        );

        let id_index_cache = Self::build_id_index_cache(
            id_index_memory,
            id_index_stats.clone(),
            eviction_callback.clone(),
            config.ttl,
            config.tti,
        );

        Self {
            vertex_cache,
            id_index_cache,
            config,
            vertex_stats,
            id_index_stats,
            eviction_callback,
        }
    }

    fn build_vertex_cache(
        max_capacity: u64,
        stats: Arc<CacheStats>,
        eviction_callback: Arc<Mutex<Option<EvictionCallback>>>,
        ttl: Option<std::time::Duration>,
        tti: Option<std::time::Duration>,
    ) -> Cache<VertexCacheKey, CachedVertex> {
        let mut builder = Cache::builder()
            .max_capacity(max_capacity)
            .weigher(|_key: &VertexCacheKey, value: &CachedVertex| {
                let key_size = std::mem::size_of::<VertexCacheKey>() as u32;
                let value_size = value.estimated_size();
                key_size.saturating_add(value_size)
            })
            .support_invalidation_closures()
            .eviction_listener(move |_key, _value, cause| {
                stats.record_eviction();
                let cause = EvictionCause::from(cause);
                if let Some(guard) = eviction_callback.try_lock() {
                    if let Some(ref callback) = *guard {
                        callback("vertex", cause);
                    }
                }
            });

        if let Some(duration) = ttl {
            builder = builder.time_to_live(duration);
        }
        if let Some(duration) = tti {
            builder = builder.time_to_idle(duration);
        }

        builder.build()
    }

    fn build_id_index_cache(
        max_capacity: u64,
        stats: Arc<CacheStats>,
        eviction_callback: Arc<Mutex<Option<EvictionCallback>>>,
        ttl: Option<std::time::Duration>,
        tti: Option<std::time::Duration>,
    ) -> Cache<IdIndexCacheKey, IdIndexCacheValue> {
        let mut builder = Cache::builder()
            .max_capacity(max_capacity)
            .weigher(|key: &IdIndexCacheKey, _value: &IdIndexCacheValue| {
                let key_size = std::mem::size_of::<IdIndexCacheKey>() as u32
                    + key.external_id.capacity() as u32;
                let value_size = std::mem::size_of::<IdIndexCacheValue>() as u32;
                key_size.saturating_add(value_size)
            })
            .support_invalidation_closures()
            .eviction_listener(move |_key, _value, cause| {
                stats.record_eviction();
                let cause = EvictionCause::from(cause);
                if let Some(guard) = eviction_callback.try_lock() {
                    if let Some(ref callback) = *guard {
                        callback("id_index", cause);
                    }
                }
            });

        if let Some(duration) = ttl {
            builder = builder.time_to_live(duration);
        }
        if let Some(duration) = tti {
            builder = builder.time_to_idle(duration);
        }

        builder.build()
    }

    fn notify_eviction(&self, cache_type: &str, cause: EvictionCause) {
        if let Some(ref callback) = *self.eviction_callback.lock() {
            callback(cache_type, cause);
        }
    }

    // ==================== ID Index Operations ====================

    pub fn get_id_index(
        &self,
        label_id: u32,
        external_id: &str,
        query_ts: Timestamp,
    ) -> Option<u32> {
        let key = IdIndexCacheKey::new(label_id, external_id.to_string());
        match self.id_index_cache.get(&key) {
            Some(cached) if cached.cached_at_ts <= query_ts => {
                self.id_index_stats.record_hit();
                Some(cached.internal_id)
            }
            Some(_) => {
                self.id_index_stats.record_miss();
                None
            }
            None => {
                self.id_index_stats.record_miss();
                None
            }
        }
    }

    pub fn insert_id_index(
        &self,
        label_id: u32,
        external_id: &str,
        internal_id: u32,
        ts: Timestamp,
    ) {
        let key = IdIndexCacheKey::new(label_id, external_id.to_string());
        self.id_index_cache.insert(
            key,
            IdIndexCacheValue {
                internal_id,
                cached_at_ts: ts,
            },
        );
    }

    pub fn remove_id_index(&self, label_id: u32, external_id: &str) {
        let key = IdIndexCacheKey::new(label_id, external_id.to_string());
        if self.id_index_cache.remove(&key).is_some() {
            self.notify_eviction("id_index", EvictionCause::Explicit);
        }
    }

    // ==================== Vertex Operations ====================

    pub fn get_vertex(&self, key: &VertexCacheKey, query_ts: Timestamp) -> Option<CachedVertex> {
        match self.vertex_cache.get(key) {
            Some(vertex) if vertex.cached_at_ts <= query_ts => {
                self.vertex_stats.record_hit();
                Some(vertex)
            }
            Some(_) => {
                self.vertex_stats.record_miss();
                None
            }
            None => {
                self.vertex_stats.record_miss();
                None
            }
        }
    }

    pub fn insert_vertex(&self, key: VertexCacheKey, vertex: CachedVertex) {
        self.vertex_cache.insert(key, vertex);
    }

    pub fn remove_vertex(&self, key: &VertexCacheKey) {
        if self.vertex_cache.remove(key).is_some() {
            self.notify_eviction("vertex", EvictionCause::Explicit);
        }
    }

    // ==================== Invalidation ====================

    /// Invalidate all vertex entries for a given label.
    ///
    /// Note: Moka does not expose which entries were actually removed,
    /// so this cannot be rolled back at the cache level.
    pub fn invalidate_vertices_by_label(&self, label_id: u32) {
        let _ = self
            .vertex_cache
            .invalidate_entries_if(move |k, _| k.label_id == label_id);
        self.vertex_cache.run_pending_tasks();
    }

    /// Invalidate all ID index entries for a given label.
    pub fn invalidate_id_indexes_by_label(&self, label_id: u32) {
        let _ = self
            .id_index_cache
            .invalidate_entries_if(move |k, _| k.label_id == label_id);
        self.id_index_cache.run_pending_tasks();
    }

    pub fn clear(&self) {
        self.vertex_cache.invalidate_all();
        self.id_index_cache.invalidate_all();
        self.vertex_cache.run_pending_tasks();
        self.id_index_cache.run_pending_tasks();
    }
}

impl Default for RecordCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared record cache type alias
pub type SharedRecordCache = Arc<RecordCache>;

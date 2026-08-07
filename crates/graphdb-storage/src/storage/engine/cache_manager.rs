//! Cache Manager
//!
//! Manages record cache and memory tracking for the storage engine.

use crate::core::types::{LabelId, Timestamp};
use crate::storage::cache::{
    CachedVertex, RecordCache, RecordCacheConfig, RecordCacheStats, SharedRecordCache,
    VertexCacheKey,
};
use crate::storage::engine::config::ResourceConfig;
use crate::storage::engine::resource_budget::{MemoryAccounting, MemoryCategory};
use std::sync::Arc;

/// Manager for storage caches
pub struct CacheManager {
    pub record_cache: Option<SharedRecordCache>,
    accounting: Arc<MemoryAccounting>,
}

impl CacheManager {
    pub fn new(
        enable_cache: bool,
        cache_memory: usize,
        resources: &ResourceConfig,
        accounting: Arc<MemoryAccounting>,
    ) -> Self {
        let record_cache = if enable_cache {
            let config = RecordCacheConfig {
                max_memory: cache_memory,
                ttl: resources.cache_ttl,
                tti: resources.cache_tti,
                ..Default::default()
            };
            let cache = SharedRecordCache::new(RecordCache::with_config(config));

            if resources.cache_eviction_sync {
                cache.set_memory_accounting(Some(accounting.clone()));
            }

            Some(cache)
        } else {
            None
        };

        Self {
            record_cache,
            accounting,
        }
    }

    pub fn refresh_memory_usage(&self) -> Option<RecordCacheStats> {
        let stats = self.record_cache.as_ref().map(|cache| cache.stats())?;
        let bytes = stats
            .vertex_weighted_size
            .saturating_add(stats.id_index_weighted_size);
        self.accounting.report_usage(MemoryCategory::Cache, bytes);
        Some(stats)
    }

    pub fn clear_cache(&self) {
        if let Some(ref record_cache) = self.record_cache {
            record_cache.clear();
        }
    }

    /// Halve the cache capacity under memory pressure, evicting cold entries.
    pub fn shrink_cache(&self) {
        if let Some(ref record_cache) = self.record_cache {
            let stats = record_cache.stats();
            let current = stats
                .vertex_weighted_size
                .saturating_add(stats.id_index_weighted_size);
            record_cache.set_capacity(current / 2);
        }
    }

    // ==================== ID Index Cache Operations ====================

    pub fn get_cached_vertex_id(
        &self,
        label: LabelId,
        external_id: &str,
        ts: Timestamp,
    ) -> Option<u32> {
        self.record_cache
            .as_ref()
            .and_then(|rc| rc.get_id_index(label, external_id, ts))
    }

    pub fn cache_vertex_id(
        &self,
        label: LabelId,
        external_id: &str,
        internal_id: u32,
        ts: Timestamp,
    ) {
        if let Some(ref rc) = self.record_cache {
            rc.insert_id_index(label, external_id, internal_id, ts);
        }
    }

    pub fn remove_cached_vertex_id(&self, label: LabelId, external_id: &str) {
        if let Some(ref rc) = self.record_cache {
            rc.remove_id_index(label, external_id);
        }
    }

    // ==================== Vertex Cache Operations ====================

    pub fn get_cached_vertex(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<CachedVertex> {
        self.record_cache.as_ref().and_then(|rc| {
            let key = VertexCacheKey::new(label, internal_id);
            rc.get_vertex(&key, ts)
        })
    }

    pub fn cache_vertex(
        &self,
        label: LabelId,
        internal_id: u32,
        external_id: String,
        properties: Vec<(String, crate::core::Value)>,
        ts: Timestamp,
    ) {
        if let Some(ref rc) = self.record_cache {
            let key = VertexCacheKey::new(label, internal_id);
            let cached = CachedVertex {
                internal_id,
                external_id,
                properties,
                cached_at_ts: ts,
                generation: 0,
            };
            rc.insert_vertex(key, cached);
        }
    }

    pub fn remove_cached_vertex(&self, label: LabelId, internal_id: u32) {
        if let Some(ref rc) = self.record_cache {
            let key = VertexCacheKey::new(label, internal_id);
            rc.remove_vertex(&key);
        }
    }

    // ==================== Cache Invalidation ====================

    pub fn invalidate_vertices_by_label(&self, label: LabelId) {
        if let Some(ref rc) = self.record_cache {
            rc.invalidate_vertices_by_label(label);
            rc.invalidate_id_indexes_by_label(label);
        }
    }
}

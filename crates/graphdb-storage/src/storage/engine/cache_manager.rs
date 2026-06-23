//! Cache Manager
//!
//! Manages record cache and memory tracking for the storage engine.

use crate::core::types::{LabelId, Timestamp};
use crate::storage::cache::{
    CachedVertex, RecordCache, RecordCacheConfig, SharedRecordCache, VertexCacheKey,
};

/// Manager for storage caches
pub struct CacheManager {
    pub record_cache: Option<SharedRecordCache>,
}

impl CacheManager {
    pub fn new(enable_cache: bool, cache_memory: usize) -> Self {
        let record_cache = if enable_cache {
            let config = RecordCacheConfig {
                max_memory: cache_memory,
                ..Default::default()
            };
            Some(SharedRecordCache::new(RecordCache::with_config(config)))
        } else {
            None
        };

        Self { record_cache }
    }

    pub fn clear_cache(&self) {
        if let Some(ref record_cache) = self.record_cache {
            record_cache.clear();
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

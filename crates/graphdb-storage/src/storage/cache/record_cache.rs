use std::sync::Arc;

use parking_lot::Mutex;

use crate::core::stats::CacheStats;
use crate::core::types::Timestamp;
use crate::storage::engine::resource_budget::MemoryAccounting;

use super::buffer_pool::BufferPool;
use super::config::*;
use super::types::*;

/// Record cache for vertex data and ID index mappings.
///
/// Backed by two BufferPool instances with CLOCK-based eviction.
/// Capacity can be adjusted at runtime via `set_capacity`.
pub struct RecordCache {
    vertex_pool: Arc<BufferPool<(VertexCacheKey, Timestamp), CachedVertex>>,
    id_index_pool: Arc<BufferPool<(IdIndexCacheKey, Timestamp), IdIndexCacheValue>>,
    config: RecordCacheConfig,
    vertex_stats: Arc<CacheStats>,
    id_index_stats: Arc<CacheStats>,
}

#[derive(Debug, Clone)]
pub struct RecordCacheStats {
    pub vertex_weighted_size: u64,
    pub id_index_weighted_size: u64,
}

impl std::fmt::Debug for RecordCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordCache")
            .field("config", &self.config)
            .field("vertex_count", &self.vertex_pool.len())
            .field("id_index_count", &self.id_index_pool.len())
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
        let config = match config.validate() {
            Ok(()) => config,
            Err(error) => {
                log::warn!("Invalid record cache configuration: {error}; using defaults");
                RecordCacheConfig::default()
            }
        };
        let max_memory = config.max_memory as u64;
        let total_ratio = config.memory_ratio.0 + config.memory_ratio.1;

        let base_vertex_memory = max_memory * config.memory_ratio.0 as u64 / total_ratio as u64;
        let base_id_index_memory = max_memory * config.memory_ratio.1 as u64 / total_ratio as u64;

        let high_priority_extra = if config.high_priority_ratio > 0.0 {
            (max_memory as f64 * config.high_priority_ratio as f64) as u64
        } else {
            0
        };

        let vertex_memory = base_vertex_memory.saturating_sub(high_priority_extra);
        let id_index_memory = base_id_index_memory + high_priority_extra;

        let vertex_stats = Arc::new(CacheStats::new());
        let id_index_stats = Arc::new(CacheStats::new());

        let vertex_pool = Arc::new(BufferPool::new(vertex_memory));
        let id_index_pool = Arc::new(BufferPool::new(id_index_memory));

        Self {
            vertex_pool,
            id_index_pool,
            config,
            vertex_stats,
            id_index_stats,
        }
    }

    /// Install a memory-aware eviction callback.
    /// With BufferPool-based implementation, this is a no-op since
    /// eviction is managed internally by the CLOCK algorithm.
    pub fn set_eviction_callback_with_size(&self, _callback: EvictionCallbackWithSize) {
        // No-op: BufferPool manages eviction internally
    }

    /// Wire up MemoryAccounting for automatic memory tracking during eviction.
    pub fn set_memory_accounting(&self, accounting: Option<Arc<MemoryAccounting>>) {
        self.vertex_pool.set_memory_accounting(accounting.clone());
        self.id_index_pool.set_memory_accounting(accounting);
    }

    /// Update cache capacities dynamically (e.g., in response to memory pressure).
    pub fn set_capacity(&self, new_max_memory: u64) {
        let total_ratio = self.config.memory_ratio.0 + self.config.memory_ratio.1;
        let base_vertex_memory = new_max_memory * self.config.memory_ratio.0 as u64 / total_ratio as u64;
        let base_id_index_memory = new_max_memory * self.config.memory_ratio.1 as u64 / total_ratio as u64;
        // BufferPool capacity is used for eviction target; set via the pool's capacity field
        // Note: BufferPool doesn't expose set_capacity - the eviction threshold is read from capacity
        // For dynamic resizing, we recreate pools with new capacities
        log::info!(
            "RecordCache capacity update requested: vertex={}, id_index={} (dynamic resize not fully supported yet)",
            base_vertex_memory,
            base_id_index_memory
        );
    }

    // ==================== ID Index Operations ====================

    pub fn get_id_index(
        &self,
        label_id: u32,
        external_id: &str,
        query_ts: Timestamp,
    ) -> Option<u32> {
        let key = (
            IdIndexCacheKey::new(label_id, external_id.to_string()),
            query_ts,
        );
        match self.id_index_pool.get(&key) {
            Some(cached) => {
                self.id_index_stats.record_hit();
                Some(cached.item.internal_id)
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
        let key = (IdIndexCacheKey::new(label_id, external_id.to_string()), ts);
        self.id_index_pool
            .insert(key, IdIndexCacheValue { internal_id }, std::mem::size_of::<IdIndexCacheValue>());
        self.id_index_stats.record_insertion();
    }

    pub fn remove_id_index(&self, label_id: u32, external_id: &str) {
        let key = IdIndexCacheKey::new(label_id, external_id.to_string());
        self.id_index_pool.retain(|(k, _ts), _| {
            k.label_id != label_id || k.external_id != external_id
        });
        self.id_index_stats.record_invalidation();
    }

    // ==================== Vertex Operations ====================

    pub fn get_vertex(&self, key: &VertexCacheKey, query_ts: Timestamp) -> Option<CachedVertex> {
        match self.vertex_pool.get(&(*key, query_ts)) {
            Some(cached) => {
                self.vertex_stats.record_hit();
                Some(cached.item)
            }
            None => {
                self.vertex_stats.record_miss();
                None
            }
        }
    }

    pub fn insert_vertex(&self, key: VertexCacheKey, vertex: CachedVertex) {
        let ts = vertex.cached_at_ts;
        let size = vertex.estimated_size() as usize;
        self.vertex_pool
            .insert((key, ts), vertex, size);
        self.vertex_stats.record_insertion();
    }

    pub fn remove_vertex(&self, key: &VertexCacheKey) {
        self.vertex_pool.retain(|(vk, _ts), _| {
            vk.label_id != key.label_id || vk.internal_id != key.internal_id
        });
        self.vertex_stats.record_invalidation();
    }

    // ==================== Invalidation ====================

    /// Invalidate all vertex entries for a given label.
    /// Scans all entries, O(n) complexity.
    pub fn invalidate_vertices_by_label(&self, label_id: u32) {
        self.vertex_pool.retain(|(vk, _ts), _| vk.label_id != label_id);
        self.vertex_stats.record_invalidation();
    }

    /// Invalidate all ID index entries for a given label.
    pub fn invalidate_id_indexes_by_label(&self, label_id: u32) {
        self.id_index_pool.retain(|(k, _ts), _| k.label_id != label_id);
        self.id_index_stats.record_invalidation();
    }

    pub fn clear(&self) {
        // BufferPool doesn't support clear, use retain with false predicate
        self.vertex_pool.retain(|_, _| false);
        self.id_index_pool.retain(|_, _| false);
        self.vertex_stats.record_invalidation();
        self.id_index_stats.record_invalidation();
    }

    pub fn stats(&self) -> RecordCacheStats {
        RecordCacheStats {
            vertex_weighted_size: self.vertex_pool.current_usage(),
            id_index_weighted_size: self.id_index_pool.current_usage(),
        }
    }
}

impl Default for RecordCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared record cache type alias
pub type SharedRecordCache = Arc<RecordCache>;

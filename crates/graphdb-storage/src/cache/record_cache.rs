use std::collections::HashMap;
use std::sync::Arc;

use graphdb_core::stats::CacheStats;
use graphdb_core::types::Timestamp;
use crate::engine::resource_budget::MemoryAccounting;

use super::buffer_pool::BufferPool;
use super::config::*;
use super::types::*;

/// Record cache for vertex data and ID index mappings.
///
/// Backed by two sharded BufferPool instances with CLOCK-based eviction.
/// Keys carry no snapshot timestamp: cached entries keep the timestamp they
/// were loaded at, and a hit is only served for the exact snapshot. Per-label
/// invalidation generations let stale entries be marked invalid in O(1).
/// Capacity can be adjusted at runtime via `set_capacity`.
pub struct RecordCache {
    vertex_pool: Arc<BufferPool<VertexCacheKey, CachedVertex>>,
    id_index_pool: Arc<BufferPool<IdIndexCacheKey, IdIndexCacheValue>>,
    config: RecordCacheConfig,
    vertex_stats: Arc<CacheStats>,
    id_index_stats: Arc<CacheStats>,
    label_generations: parking_lot::RwLock<HashMap<u32, u32>>,
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
            label_generations: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    fn label_generation(&self, label_id: u32) -> u32 {
        self.label_generations
            .read()
            .get(&label_id)
            .copied()
            .unwrap_or(0)
    }

    fn bump_label_generation(&self, label_id: u32) {
        let mut generations = self.label_generations.write();
        let entry = generations.entry(label_id).or_insert(0);
        *entry = entry.wrapping_add(1);
    }

    /// Wire up MemoryAccounting for automatic memory tracking during eviction.
    pub fn set_memory_accounting(&self, accounting: Option<Arc<MemoryAccounting>>) {
        self.vertex_pool.set_memory_accounting(accounting.clone());
        self.id_index_pool.set_memory_accounting(accounting);
    }

    /// Update cache capacities dynamically (e.g., in response to memory pressure).
    pub fn set_capacity(&self, new_max_memory: u64) {
        let total_ratio = self.config.memory_ratio.0 + self.config.memory_ratio.1;
        let base_vertex_memory =
            new_max_memory * self.config.memory_ratio.0 as u64 / total_ratio as u64;
        let base_id_index_memory =
            new_max_memory * self.config.memory_ratio.1 as u64 / total_ratio as u64;
        self.vertex_pool.set_capacity(base_vertex_memory);
        self.id_index_pool.set_capacity(base_id_index_memory);
    }

    // ==================== ID Index Operations ====================

    pub fn get_id_index(
        &self,
        label_id: u32,
        external_id: &str,
        query_ts: Timestamp,
    ) -> Option<u32> {
        let key = IdIndexCacheKey::new(label_id, external_id.to_string());
        match self.id_index_pool.get(&key) {
            Some(cached)
                if cached.item.cached_at_ts == query_ts
                    && cached.item.generation == self.label_generation(label_id) =>
            {
                self.id_index_stats.record_hit();
                Some(cached.item.internal_id)
            }
            _ => {
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
        let value = IdIndexCacheValue {
            internal_id,
            cached_at_ts: ts,
            generation: self.label_generation(label_id),
        };
        self.id_index_pool
            .insert(key, value, std::mem::size_of::<IdIndexCacheValue>());
        self.id_index_stats.record_insertion();
    }

    pub fn remove_id_index(&self, label_id: u32, external_id: &str) {
        self.id_index_pool
            .retain(|k, _| k.label_id != label_id || k.external_id != external_id);
        self.id_index_stats.record_invalidation();
    }

    // ==================== Vertex Operations ====================

    pub fn get_vertex(&self, key: &VertexCacheKey, query_ts: Timestamp) -> Option<CachedVertex> {
        match self.vertex_pool.get(key) {
            Some(cached)
                if cached.item.cached_at_ts == query_ts
                    && cached.item.generation == self.label_generation(key.label_id) =>
            {
                self.vertex_stats.record_hit();
                Some(cached.item.clone())
            }
            _ => {
                self.vertex_stats.record_miss();
                None
            }
        }
    }

    pub fn insert_vertex(&self, key: VertexCacheKey, vertex: CachedVertex) {
        let mut vertex = vertex;
        vertex.generation = self.label_generation(key.label_id);
        let size = vertex.estimated_size() as usize;
        self.vertex_pool.insert(key, vertex, size);
        self.vertex_stats.record_insertion();
    }

    pub fn remove_vertex(&self, key: &VertexCacheKey) {
        self.vertex_pool
            .retain(|vk, _| vk.label_id != key.label_id || vk.internal_id != key.internal_id);
        self.vertex_stats.record_invalidation();
    }

    // ==================== Invalidation ====================

    /// Invalidate all vertex entries for a given label.
    /// O(1): bumps the label generation; stale entries are rejected on read
    /// and reclaimed lazily by capacity eviction.
    pub fn invalidate_vertices_by_label(&self, label_id: u32) {
        self.bump_label_generation(label_id);
        self.vertex_stats.record_invalidation();
    }

    /// Invalidate all ID index entries for a given label.
    /// O(1): bumps the label generation; stale entries are rejected on read
    /// and reclaimed lazily by capacity eviction.
    pub fn invalidate_id_indexes_by_label(&self, label_id: u32) {
        self.bump_label_generation(label_id);
        self.id_index_stats.record_invalidation();
    }

    pub fn clear(&self) {
        // BufferPool doesn't support clear, use retain with false predicate
        self.vertex_pool.retain(|_, _| false);
        self.id_index_pool.retain(|_, _| false);
        self.label_generations.write().clear();
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

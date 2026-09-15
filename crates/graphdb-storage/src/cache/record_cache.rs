use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::resource_budget::MemoryAccounting;
use graphdb_core::types::Timestamp;

use super::buffer_pool::BufferPool;
use super::config::*;
use super::types::*;

/// Record cache for vertex data and ID index mappings.
///
/// Backed by two sharded BufferPool instances with CLOCK-based eviction.
/// Keys carry no snapshot timestamp: each key maps to at most one entry.
/// A cached entry records the timestamp it was loaded at (`cached_at_ts`)
/// and is served to any reader at or past that timestamp
/// (`cached_at_ts <= query_ts`); older snapshots miss and fall back to a
/// version-chain read. Correctness rests on two contracts, not one:
/// every vertex/edge write path removes (O(1)) or generation-bumps the
/// affected entries, AND callers only consult or populate the vertex cache
/// for snapshots at the committed frontier (see `record_cache_eligible`).
/// A historical snapshot read must never populate the cache: its value is
/// not current, and a single entry per key cannot serve two snapshots that
/// observe different versions. Per-label invalidation generations mark
/// stale entries invalid in O(1). Capacity can be adjusted at runtime via
/// `set_capacity`.
pub struct RecordCache {
    vertex_pool: Arc<BufferPool<VertexCacheKey, CachedVertex>>,
    id_index_pool: Arc<BufferPool<IdIndexCacheKey, IdIndexCacheValue>>,
    config: RecordCacheConfig,
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

        let vertex_pool = Arc::new(BufferPool::new(vertex_memory));
        let id_index_pool = Arc::new(BufferPool::new(id_index_memory));
        vertex_pool.set_ttl(config.ttl);
        vertex_pool.set_tti(config.tti);
        id_index_pool.set_ttl(config.ttl);
        id_index_pool.set_tti(config.tti);

        Self {
            vertex_pool,
            id_index_pool,
            config,
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
            // Forward-compatible hit: the mapping was loaded at or before
            // the reader's snapshot and no write has invalidated it since.
            // Older snapshots (query_ts < cached_at_ts) miss so they fall
            // back to a version-aware lookup instead of seeing newer data.
            Some(cached)
                if cached.item.cached_at_ts <= query_ts
                    && cached.item.generation == self.label_generation(label_id) =>
            {
                Some(cached.item.internal_id)
            }
            _ => None,
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
    }

    pub fn remove_id_index(&self, label_id: u32, external_id: &str) {
        let key = IdIndexCacheKey::new(label_id, external_id.to_string());
        // O(1) point invalidation: each key maps to at most one entry.
        self.id_index_pool.remove(&key);
    }

    // ==================== Vertex Operations ====================

    pub fn get_vertex(&self, key: &VertexCacheKey, query_ts: Timestamp) -> Option<CachedVertex> {
        match self.vertex_pool.get(key) {
            // Forward-compatible hit: the record was loaded at or before
            // the reader's snapshot and no write has invalidated it since,
            // so it still reflects the current value. Older snapshots
            // (query_ts < cached_at_ts) miss so they fall back to a
            // version-chain read instead of seeing newer data.
            Some(cached)
                if cached.item.cached_at_ts <= query_ts
                    && cached.item.generation == self.label_generation(key.label_id) =>
            {
                Some(cached.item.clone())
            }
            _ => None,
        }
    }

    pub fn insert_vertex(&self, key: VertexCacheKey, vertex: CachedVertex) {
        let mut vertex = vertex;
        vertex.generation = self.label_generation(key.label_id);
        let size = vertex.estimated_size() as usize;
        self.vertex_pool.insert(key, vertex, size);
    }

    pub fn remove_vertex(&self, key: &VertexCacheKey) {
        // O(1) point invalidation: each key maps to at most one entry.
        self.vertex_pool.remove(key);
    }

    // ==================== Invalidation ====================

    /// Invalidate all vertex entries for a given label.
    /// O(1): bumps the label generation; stale entries are rejected on read
    /// and reclaimed lazily by capacity eviction.
    pub fn invalidate_vertices_by_label(&self, label_id: u32) {
        self.bump_label_generation(label_id);
    }

    /// Invalidate all ID index entries for a given label.
    /// O(1): bumps the label generation; stale entries are rejected on read
    /// and reclaimed lazily by capacity eviction.
    pub fn invalidate_id_indexes_by_label(&self, label_id: u32) {
        self.bump_label_generation(label_id);
    }

    pub fn clear(&self) {
        self.vertex_pool.clear();
        self.id_index_pool.clear();
        self.label_generations.write().clear();
    }

    pub fn stats(&self) -> RecordCacheStats {
        RecordCacheStats {
            vertex_weighted_size: self.vertex_pool.current_usage(),
            id_index_weighted_size: self.id_index_pool.current_usage(),
        }
    }

    /// Drop all entries that have exceeded their TTL/TTI.
    /// Returns the number of entries removed.
    pub fn prune_expired(&self) -> usize {
        self.vertex_pool.prune_expired() + self.id_index_pool.prune_expired()
    }
}

impl Default for RecordCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared record cache type alias
pub type SharedRecordCache = Arc<RecordCache>;

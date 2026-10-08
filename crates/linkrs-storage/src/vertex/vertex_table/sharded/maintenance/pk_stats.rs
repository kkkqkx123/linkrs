//! Primary-key index observability aggregates across shards.

use super::super::ShardedVertexTable;

impl ShardedVertexTable {
    /// Aggregate primary-key reuse across shards for observability.
    ///
    /// Returns `(cumulative_reuses, free_depth_total)`: how many inserts
    /// recycled a deleted slot versus growing the id space, and how many
    /// holes await reuse. A growing high-water mark beside a flat reuse
    /// count means deletes are not being absorbed and compaction pressure
    /// builds instead.
    pub fn pk_reuse_stats(&self) -> (u64, usize) {
        let mut reuses = 0u64;
        let mut free_depth = 0usize;
        for shard in &self.shards {
            let table = shard.read();
            reuses = reuses.saturating_add(table.id_indexer.reuse_count());
            free_depth += table.id_indexer.free_depth();
        }
        (reuses, free_depth)
    }

    /// Worst index-level hole ratio across shards (see
    /// [`crate::vertex::id_indexer::IdManager::hole_ratio`]): fast
    /// pre-check beside the reuse counters so patrol logs show whether
    /// ordered reuse keeps holes tail-adjacent instead of scattering them.
    pub fn index_hole_ratio(&self) -> f64 {
        let mut worst = 0.0f64;
        for shard in &self.shards {
            worst = worst.max(shard.read().id_indexer.hole_ratio());
        }
        worst
    }

    /// Aggregate primary-key index heap across shards.
    ///
    /// Returns `(total_bytes, max_shard_bytes)`: the whole-table resident
    /// cost and the hottest shard.
    pub fn pk_memory_stats(&self) -> (usize, usize) {
        let mut total = 0usize;
        let mut max_shard = 0usize;
        for shard in &self.shards {
            let bytes = shard.read().id_indexer.memory_breakdown().total_bytes;
            total += bytes;
            max_shard = max_shard.max(bytes);
        }
        (total, max_shard)
    }

    /// Aggregate per-component primary-key memory accounting across shards.
    ///
    /// Sums every breakdown field so patrol logs show which structure (key
    /// heap, map, live set, delta log) dominates.
    pub fn pk_memory_breakdown(&self) -> crate::vertex::id_indexer::IdIndexMemoryBreakdown {
        let mut total = crate::vertex::id_indexer::IdIndexMemoryBreakdown::default();
        for shard in &self.shards {
            let breakdown = shard.read().id_indexer.memory_breakdown();
            total.slot_count += breakdown.slot_count;
            total.live_count += breakdown.live_count;
            total.hole_count += breakdown.hole_count;
            total.hole_bytes += breakdown.hole_bytes;
            total.free_depth += breakdown.free_depth;
            total.delta_entries += breakdown.delta_entries;
            total.delta_heap_bytes += breakdown.delta_heap_bytes;
            total.keys_heap_bytes += breakdown.keys_heap_bytes;
            total.map_bytes += breakdown.map_bytes;
            total.set_bytes += breakdown.set_bytes;
            total.free_bytes += breakdown.free_bytes;
            total.total_bytes += breakdown.total_bytes;
        }
        total
    }
}

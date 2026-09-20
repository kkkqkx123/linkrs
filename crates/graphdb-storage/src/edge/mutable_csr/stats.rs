use super::super::{ColdStamps, HotNbr};
use super::MutableCsr;
use crate::edge::FragmentationStats;

impl MutableCsr {
    pub fn overflow_chunk_allocs(&self) -> u64 {
        self.overflow_chunk_allocs
    }

    pub fn primary_block_allocs(&self) -> u64 {
        self.primary_block_allocs
    }

    pub fn repack_count(&self) -> u64 {
        self.repack_count
    }

    pub fn tombstone_reuse_count(&self) -> u64 {
        self.tombstone_reuse_count
    }

    pub fn live_set_rebuild_count(&self) -> u64 {
        self.live_set_rebuild_count
    }

    pub fn vertex_expansion_count(&self) -> u64 {
        self.vertex_expansion_count
    }

    pub fn reset_baseline_counters(&mut self) {
        self.overflow_chunk_allocs = 0;
        self.primary_block_allocs = 0;
        self.repack_count = 0;
        self.tombstone_reuse_count = 0;
        self.live_set_rebuild_count = 0;
        self.vertex_expansion_count = 0;
    }
}

impl MutableCsr {
    /// Get used memory size, counting reserved topology plus indexes.
    ///
    /// Covers the primary neighbor list, the offset/degree/capacity arrays,
    /// reserved overflow chunk buffers, the dedup live sets and this struct.
    /// Tombstone authority memory is accounted by the table layer through
    /// the shared tombstone estimate so both stay on one caliber.
    pub fn used_memory_size(&self) -> usize {
        let slot_bytes = std::mem::size_of::<HotNbr>() + std::mem::size_of::<ColdStamps>();
        let arrays = self.hot_list.capacity() * std::mem::size_of::<HotNbr>()
            + self.cold_list.capacity() * std::mem::size_of::<ColdStamps>()
            + self.rows.adj_offsets.capacity() * std::mem::size_of::<u32>()
            + self.rows.degrees.capacity() * std::mem::size_of::<u32>()
            + self.rows.primary_capacities.capacity() * std::mem::size_of::<u32>();
        let overflow_reserved: usize = self
            .overflow_chunks
            .iter()
            .map(|(_, chunks)| chunks.iter().map(|chunk| chunk.capacity()).sum::<usize>())
            .sum();
        let live_heap: usize = self.live_sets.heap_bytes_total();
        let live_sets = self.live_sets.index_bytes() + live_heap;
        arrays
            + overflow_reserved * slot_bytes
            + self.overflow_chunks.index_bytes()
            + live_sets
            + std::mem::size_of::<Self>()
    }

    /// Compute fragmentation ratio as wasted share of reserved capacity.
    ///
    /// Single caliber shared with `FragmentationStats`: `wasted / total`,
    /// ranging from 0.0 (perfect packing) to below 1.0. A ratio above 0.5
    /// marks a waste-dominated table worth a group merge; the write path
    /// triggers on per-vertex reclaimable counts instead of this ratio.
    pub fn fragmentation_ratio(&self) -> f32 {
        if self.total_edge_capacity == 0 {
            return 0.0;
        }
        let active_edges = self.edge_count as usize;
        self.total_edge_capacity.saturating_sub(active_edges) as f32
            / self.total_edge_capacity as f32
    }

    /// Estimate wasted memory due to fragmentation (in bytes)
    pub(crate) fn wasted_bytes_estimate(&self) -> usize {
        let active_edges = self.edge_count as usize;
        let slot_bytes = std::mem::size_of::<HotNbr>() + std::mem::size_of::<ColdStamps>();
        self.total_edge_capacity.saturating_sub(active_edges) * slot_bytes
    }

    /// Get detailed fragmentation statistics.
    ///
    /// Both counters derive from the live structures: dead entries are the
    /// physically stored entries minus live edges (primary tombstones plus
    /// overflow dead entries), and wasted capacity is the reserved capacity
    /// minus live edges (row gaps plus tombstone slots).
    pub fn get_fragmentation_stats(&self) -> FragmentationStats {
        let live_edges = self.edge_count as usize;

        let mut physical_entries = 0usize;
        for vid in 0..self.vertex_capacity() {
            physical_entries += self.rows.degrees[vid] as usize;
        }
        for (_, chunks) in self.overflow_chunks.iter() {
            for chunk in chunks {
                physical_entries += chunk.len();
            }
        }

        let dead_entries = physical_entries.saturating_sub(live_edges);
        let wasted_capacity = self.total_edge_capacity.saturating_sub(live_edges);

        FragmentationStats::with_dead_info(
            self.total_edge_capacity,
            live_edges,
            dead_entries,
            wasted_capacity,
        )
    }

    /// Fraction of overflow rows that hold exactly one chunk.
    ///
    /// Skewed rows that stay single-block after merge are the common
    /// case; a ratio near 1.0 confirms the merge threshold and
    /// vertex-level expansion are working.  Returns 0.0 when no vertex
    /// carries overflow.
    pub fn single_block_overflow_ratio(&self) -> f32 {
        let mut total_overflow_rows = 0usize;
        let mut single_block_rows = 0usize;
        for (_, chunks) in self.overflow_chunks.iter() {
            if !chunks.is_empty() {
                total_overflow_rows += 1;
                if chunks.len() == 1 {
                    single_block_rows += 1;
                }
            }
        }
        if total_overflow_rows == 0 {
            return 0.0;
        }
        single_block_rows as f32 / total_overflow_rows as f32
    }
}

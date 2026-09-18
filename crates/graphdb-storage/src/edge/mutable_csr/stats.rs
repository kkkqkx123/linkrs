use super::super::Nbr;
use super::MutableCsr;
use crate::edge::FragmentationStats;

impl MutableCsr {
    /// Get used memory size, counting reserved topology plus indexes.
    ///
    /// Covers the primary neighbor list, the offset/degree/capacity arrays,
    /// reserved overflow chunk buffers, the dedup live sets and this struct.
    /// Tombstone authority memory is accounted by the table layer through
    /// the shared tombstone estimate so both stay on one caliber.
    pub fn used_memory_size(&self) -> usize {
        let arrays = self.nbr_list.capacity() * std::mem::size_of::<Nbr>()
            + self.adj_offsets.capacity() * std::mem::size_of::<u32>()
            + self.degrees.capacity() * std::mem::size_of::<u32>()
            + self.primary_capacities.capacity() * std::mem::size_of::<u32>();
        let overflow_reserved: usize = self
            .overflow_chunks
            .iter()
            .map(|(_, chunks)| chunks.iter().map(Vec::capacity).sum::<usize>())
            .sum();
        let live_heap: usize = self.live_sets.heap_bytes_total();
        let live_sets = self.live_sets.index_bytes() + live_heap;
        arrays
            + overflow_reserved * std::mem::size_of::<Nbr>()
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
        self.total_edge_capacity.saturating_sub(active_edges) * std::mem::size_of::<Nbr>()
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
            physical_entries += self.degrees[vid] as usize;
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
}

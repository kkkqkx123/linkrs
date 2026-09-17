use std::sync::atomic::Ordering;

use super::MutableCsr;
use super::super::Nbr;
use crate::edge::FragmentationStats;

impl MutableCsr {
    /// Get used memory size (active edges only)
    pub fn used_memory_size(&self) -> usize {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        active_edges * std::mem::size_of::<Nbr>() + std::mem::size_of::<Self>()
    }

    /// Compute fragmentation ratio: reserved capacity over live edges.
    ///
    /// A ratio > 1.5 indicates moderate fragmentation; > 2.0 suggests
    /// collection. Returns 0.0 if no live edges. This whole-table ratio is
    /// an observation metric; the write path triggers on per-vertex
    /// reclaimable counts instead.
    pub fn fragmentation_ratio(&self) -> f32 {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        if active_edges == 0 {
            return 0.0;
        }
        self.total_edge_capacity as f32 / active_edges as f32
    }

    /// Estimate wasted memory due to fragmentation (in bytes)
    pub(crate) fn wasted_bytes_estimate(&self) -> usize {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        self.total_edge_capacity.saturating_sub(active_edges) * std::mem::size_of::<Nbr>()
    }

    /// Get detailed fragmentation statistics.
    ///
    /// Both counters derive from the live structures: dead entries are the
    /// physically stored entries minus live edges (primary tombstones plus
    /// overflow dead entries), and wasted capacity is the reserved capacity
    /// minus live edges (row gaps plus tombstone slots).
    pub fn get_fragmentation_stats(&self) -> FragmentationStats {
        let live_edges = self.edge_count.load(Ordering::Relaxed) as usize;

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

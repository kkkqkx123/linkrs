//! Fragmentation statistics and observability for CSR structures.
//!
//! Tracks waste from row gaps and tombstoned entries:
//! - Primary rows reserve gaps for everyday writes; the unused slots are waste.
//! - Deleted entries stay physically present until the collection cutoff
//!   passes; those tombstone slots are waste as well.
//! - Overflow chunks are appended per vertex and repacked per vertex; empty
//!   chunks are detached immediately, so no unreachable-block accounting
//!   remains.
//!
//! # Collection
//!
//! Per-vertex counts drive incremental collection: a vertex is a candidate
//! exactly when it holds entries eligible at the current cutoff. Whole-table
//! ratios below are observation metrics, not collection triggers.

#[derive(Debug, Clone, Copy)]
pub struct FragmentationStats {
    /// Total reserved capacity (primary rows plus overflow chunks).
    pub total_capacity: usize,
    /// Number of live edges.
    pub reachable_edges: usize,
    /// Number of dead entries awaiting collection (primary tombstones plus
    /// overflow dead entries). The historical field name is retained; no
    /// unreachable overflow blocks exist anymore.
    pub zombie_blocks: usize,
    /// Reserved capacity minus live edges (row gaps plus tombstone slots).
    pub wasted_capacity: usize,
}

impl FragmentationStats {
    /// Create basic stats without zombie block information.
    /// Use `compute()` for full analysis including zombie blocks.
    pub fn new(total_capacity: usize, reachable_edges: usize) -> Self {
        Self {
            total_capacity,
            reachable_edges,
            zombie_blocks: 0,
            wasted_capacity: 0,
        }
    }

    /// Compute detailed fragmentation stats from edge counts.
    pub fn with_zombie_info(
        total_capacity: usize,
        reachable_edges: usize,
        zombie_blocks: usize,
        wasted_capacity: usize,
    ) -> Self {
        Self {
            total_capacity,
            reachable_edges,
            zombie_blocks,
            wasted_capacity,
        }
    }

    /// Fragmentation ratio: wasted_capacity / total_capacity.
    ///
    /// - 0.0 = no fragmentation (perfect packing)
    /// - 0.5 = 50% wasted space
    /// - 1.0 = all reserved capacity is waste
    ///
    /// Observation metric for dashboards; collection triggers use the
    /// per-vertex reclaimable counts instead.
    pub fn fragmentation_ratio(&self) -> f32 {
        if self.total_capacity == 0 {
            0.0
        } else {
            self.wasted_capacity as f32 / self.total_capacity as f32
        }
    }

    /// Unused capacity in currently allocated nbr_list
    pub fn unused_capacity(&self) -> usize {
        self.total_capacity.saturating_sub(self.reachable_edges)
    }

    /// Space efficiency: reachable_edges / total_capacity
    ///
    /// - 1.0 = perfect efficiency (no fragmentation)
    /// - 0.5 = 50% efficiency (half the space is unused)
    pub fn space_efficiency(&self) -> f32 {
        if self.total_capacity == 0 {
            1.0
        } else {
            self.reachable_edges as f32 / self.total_capacity as f32
        }
    }

    /// Estimated space reclamation if compacted
    pub fn reclamation_potential(&self) -> usize {
        self.unused_capacity()
    }

    /// Check if compaction is recommended
    ///
    /// Threshold applies to the waste ratio above.
    pub fn should_compact(&self, threshold: f32) -> bool {
        self.fragmentation_ratio() >= threshold
    }
}

/// Per-vertex fragmentation view driving incremental collection.
///
/// A vertex is a collection candidate exactly when `reclaimable` is
/// nonzero: it holds tombstones the current cutoff already covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexFragmentation {
    /// Row under observation.
    pub vertex: u32,
    /// Live entries of the row.
    pub live_edges: usize,
    /// Tombstoned entries of the row, regardless of eligibility.
    pub dead_entries: usize,
    /// Reserved row capacity (primary slots plus overflow chunks).
    pub capacity: usize,
    /// Entries of the row eligible at the current cutoff.
    pub reclaimable: usize,
}

impl VertexFragmentation {
    /// Reserved slots minus live entries (gaps plus tombstone slots).
    pub fn waste(&self) -> usize {
        self.capacity.saturating_sub(self.live_edges)
    }

    /// Live entries per unit of reserved capacity.
    pub fn density(&self) -> f32 {
        if self.capacity == 0 {
            1.0
        } else {
            self.live_edges as f32 / self.capacity as f32
        }
    }

    /// Whether incremental collection should visit this row.
    pub fn needs_compact(&self) -> bool {
        self.reclaimable > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fragmentation_stats_no_fragmentation() {
        let stats = FragmentationStats {
            total_capacity: 100,
            reachable_edges: 100,
            zombie_blocks: 0,
            wasted_capacity: 0,
        };

        assert_eq!(stats.fragmentation_ratio(), 0.0);
        assert_eq!(stats.space_efficiency(), 1.0);
        assert_eq!(stats.unused_capacity(), 0);
        assert!(!stats.should_compact(2.0));
    }

    #[test]
    fn test_fragmentation_stats_with_fragmentation() {
        let stats = FragmentationStats {
            total_capacity: 100,
            reachable_edges: 30,
            zombie_blocks: 2,
            wasted_capacity: 50,
        };

        assert_eq!(stats.fragmentation_ratio(), 0.5);
        assert_eq!(stats.space_efficiency(), 0.3);
        assert_eq!(stats.unused_capacity(), 70);
        assert!(!stats.should_compact(2.0));
    }

    #[test]
    fn test_fragmentation_stats_severe_fragmentation() {
        let stats = FragmentationStats {
            total_capacity: 100,
            reachable_edges: 20,
            zombie_blocks: 3,
            wasted_capacity: 250, // 250% overhead
        };

        assert_eq!(stats.fragmentation_ratio(), 2.5);
        assert_eq!(stats.space_efficiency(), 0.2);
        assert_eq!(stats.unused_capacity(), 80);
        assert!(stats.should_compact(2.0));
    }

    #[test]
    fn test_fragmentation_stats_empty() {
        let stats = FragmentationStats::new(0, 0);

        assert_eq!(stats.fragmentation_ratio(), 0.0);
        assert_eq!(stats.space_efficiency(), 1.0);
        assert_eq!(stats.unused_capacity(), 0);
    }

    #[test]
    fn test_should_compact_threshold() {
        let stats = FragmentationStats {
            total_capacity: 100,
            reachable_edges: 50,
            zombie_blocks: 1,
            wasted_capacity: 150,
        };

        assert!(stats.should_compact(1.0)); // ratio = 1.5, threshold = 1.0
        assert!(!stats.should_compact(2.0)); // ratio = 1.5, threshold = 2.0
    }

    #[test]
    fn test_vertex_fragmentation_view() {
        let view = VertexFragmentation {
            vertex: 3,
            live_edges: 4,
            dead_entries: 1,
            capacity: 8,
            reclaimable: 1,
        };
        assert_eq!(view.waste(), 4);
        assert_eq!(view.density(), 0.5);
        assert!(view.needs_compact());

        let clean = VertexFragmentation {
            vertex: 4,
            live_edges: 4,
            dead_entries: 0,
            capacity: 8,
            reclaimable: 0,
        };
        assert!(!clean.needs_compact());
    }
}

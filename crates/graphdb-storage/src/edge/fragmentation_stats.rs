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

/// Group merge gate on the wasted-share caliber: a group whose wasted share
/// reaches this level is worth a group-scope merge. Row and region scopes
/// trigger on per-vertex reclaimable counts alone.
pub const GROUP_FRAGMENTATION_THRESHOLD: f32 = 0.5;

#[derive(Debug, Clone, Copy)]
pub struct FragmentationStats {
    /// Total reserved capacity (primary rows plus overflow chunks).
    pub total_capacity: usize,
    /// Number of live edges.
    pub reachable_edges: usize,
    /// Number of dead entries awaiting collection (primary tombstones plus
    /// overflow dead entries).
    pub dead_entries: usize,
    /// Reserved capacity minus live edges (row gaps plus tombstone slots).
    pub wasted_capacity: usize,
}

impl FragmentationStats {
    /// Compute detailed fragmentation stats from edge counts.
    pub fn with_dead_info(
        total_capacity: usize,
        reachable_edges: usize,
        dead_entries: usize,
        wasted_capacity: usize,
    ) -> Self {
        Self {
            total_capacity,
            reachable_edges,
            dead_entries,
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

    /// Measured bytes per edge with no fallback.
    ///
    /// Empty tables report zero; degenerate zero measurements report zero.
    /// Callers handle zero explicitly instead of relying on estimates.
    pub fn measured_bytes_per_edge(used_bytes: usize, edges: u64) -> usize {
        if edges == 0 {
            return 0;
        }
        let bpe = used_bytes / edges as usize;
        if bpe == 0 {
            log::debug!(
                "bytes_per_edge: measured zero ({} bytes / {} edges), reporting zero",
                used_bytes,
                edges,
            );
        }
        bpe
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
            dead_entries: 0,
            wasted_capacity: 0,
        };

        assert_eq!(stats.fragmentation_ratio(), 0.0);
        assert!(stats.fragmentation_ratio() < GROUP_FRAGMENTATION_THRESHOLD);
    }

    #[test]
    fn test_fragmentation_stats_with_fragmentation() {
        let stats = FragmentationStats {
            total_capacity: 100,
            reachable_edges: 30,
            dead_entries: 2,
            wasted_capacity: 50,
        };

        assert_eq!(stats.fragmentation_ratio(), 0.5);
        assert!(stats.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD);
    }

    #[test]
    fn test_fragmentation_stats_severe_fragmentation() {
        let stats = FragmentationStats {
            total_capacity: 100,
            reachable_edges: 20,
            dead_entries: 3,
            wasted_capacity: 80,
        };

        assert_eq!(stats.fragmentation_ratio(), 0.8);
        assert!(stats.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD);
    }

    #[test]
    fn test_fragmentation_stats_empty() {
        let stats = FragmentationStats::with_dead_info(0, 0, 0, 0);

        assert_eq!(stats.fragmentation_ratio(), 0.0);
    }

    #[test]
    fn test_fragmentation_ratio_threshold() {
        let stats = FragmentationStats {
            total_capacity: 100,
            reachable_edges: 50,
            dead_entries: 1,
            wasted_capacity: 60,
        };

        assert!(stats.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD);
        assert!(stats.fragmentation_ratio() <= 1.0);
    }

    #[test]
    fn test_measured_bytes_per_edge() {
        assert_eq!(FragmentationStats::measured_bytes_per_edge(0, 0), 0);
        assert_eq!(FragmentationStats::measured_bytes_per_edge(100, 0), 0);
        assert_eq!(FragmentationStats::measured_bytes_per_edge(100, 10), 10);
        assert_eq!(FragmentationStats::measured_bytes_per_edge(5, 10), 0);
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

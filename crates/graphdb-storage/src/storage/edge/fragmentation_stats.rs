//! Fragmentation statistics and observability for CSR structures.
//!
//! Tracks internal fragmentation caused by the two-level overflow design:
//! - Primary blocks remain fixed in size
//! - Overflow blocks are appended to nbr_list when vertices expand
//! - Old overflow blocks become unreachable but still occupy space ("zombie blocks")
//!
//! # Fragmentation Sources
//!
//! When a vertex's overflow block expands multiple times via `expand_vertex_capacity()`:
//! 1. New data is appended to the end of `nbr_list`
//! 2. Old overflow block becomes unreachable (zombie)
//! 3. Repeated expansions accumulate zombie blocks
//!
//! # Recovery Strategy
//!
//! Call `compact_with_ts()` to:
//! - Merge primary + overflow into flat CSR layout
//! - Reclaim all zombie block space
//! - Remove logically deleted edges

#[derive(Debug, Clone, Copy)]
pub struct FragmentationStats {
    /// Total capacity allocated in nbr_list
    pub total_capacity: usize,
    /// Number of actively reachable edges (primary + current overflow)
    pub reachable_edges: usize,
    /// Number of zombie blocks (unreachable overflow blocks)
    pub zombie_blocks: usize,
    /// Approximate wasted capacity in zombie blocks
    pub wasted_capacity: usize,
}

impl FragmentationStats {
    pub fn new(total_capacity: usize, reachable_edges: usize) -> Self {
        Self {
            total_capacity,
            reachable_edges,
            zombie_blocks: 0,
            wasted_capacity: 0,
        }
    }

    /// Fragmentation ratio: wasted_capacity / total_capacity.
    ///
    /// - 0.0 = no fragmentation (perfect packing)
    /// - 0.5 = 50% wasted space
    /// - 1.0 = 100% wasted space (all capacity is zombie blocks)
    ///
    /// Typical threshold for compaction: ratio > 2.0 (200% overhead)
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
    /// Default threshold: fragmentation_ratio >= 2.0
    pub fn should_compact(&self, threshold: f32) -> bool {
        self.fragmentation_ratio() >= threshold
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
}

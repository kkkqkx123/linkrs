//! Observability views: per-group stats and whole-set memory and
//! fragmentation metrics summed across existing groups.

use graphdb_core::types::EdgeStrategy;

use super::super::{CsrBase, FragmentationStats, MutableCsrTrait};
use super::{group_base, CsrShardSet};

/// Observability view of one group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeGroupStats {
    pub group: usize,
    pub base: u32,
    pub rows: usize,
    pub live_edges: u64,
    pub capacity: usize,
    pub density: f32,
    pub dirty: super::GroupDirty,
}

impl CsrShardSet {
    /// Observability view of one group.
    pub fn group_stats(&self, gid: usize) -> Option<NodeGroupStats> {
        let shard = self.shards.get(&gid)?;
        let live = shard.variant.edge_count();
        let capacity = shard
            .variant
            .fragmentation_stats()
            .map(|stats| stats.total_capacity)
            .unwrap_or_else(|| shard.variant.vertex_capacity());
        let density = if capacity == 0 {
            1.0
        } else {
            live as f32 / capacity as f32
        };
        Some(NodeGroupStats {
            group: gid,
            base: group_base(gid, self.group_bits),
            rows: self.group_size(),
            live_edges: live,
            capacity,
            density,
            dirty: shard.dirty,
        })
    }

    pub fn all_group_stats(&self) -> Vec<NodeGroupStats> {
        self.existing_group_ids()
            .into_iter()
            .filter_map(|gid| self.group_stats(gid))
            .collect()
    }

    /// Live edge count of one group for segment statistics.
    pub fn group_live_count(&self, gid: usize) -> u64 {
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.edge_count())
            .unwrap_or(0)
    }

    /// Minimum and maximum neighbor endpoints stored in one group.
    ///
    /// Sort-column bounds for segment statistics: a single pass over the
    /// group entries without materializing them. Missing groups report no
    /// bounds.
    pub fn group_endpoint_bounds(&self, gid: usize) -> (Option<u32>, Option<u32>) {
        let Some(shard) = self.shards.get(&gid) else {
            return (None, None);
        };
        let mut min: Option<u32> = None;
        let mut max: Option<u32> = None;
        for (_, nbr) in shard.variant.iter_all() {
            min = Some(min.map_or(nbr.endpoint, |current: u32| current.min(nbr.endpoint)));
            max = Some(max.map_or(nbr.endpoint, |current: u32| current.max(nbr.endpoint)));
        }
        (min, max)
    }

    /// Average bytes per edge based on actual memory usage.
    /// Measured value only, with no fallback: empty tables report zero, and
    /// a degenerate zero measurement on a non-empty table reports zero with
    /// a debug line. Callers handle zero explicitly.
    pub fn bytes_per_edge(&self) -> usize {
        FragmentationStats::measured_bytes_per_edge(self.used_memory_size(), self.edge_count())
    }

    /// Whole-set fragmentation statistics, summed across groups.
    /// Observation metric only; collection triggers use per-vertex counts.
    pub fn fragmentation_stats(&self) -> Option<FragmentationStats> {
        if self.strategy != EdgeStrategy::Multiple {
            return None;
        }
        let mut total_capacity = 0usize;
        let mut reachable_edges = 0usize;
        let mut dead_entries = 0usize;
        let mut wasted_capacity = 0usize;
        for shard in self.shards.values() {
            if let Some(stats) = shard.variant.fragmentation_stats() {
                total_capacity += stats.total_capacity;
                reachable_edges += stats.reachable_edges;
                dead_entries += stats.dead_entries;
                wasted_capacity += stats.wasted_capacity;
            }
        }
        Some(FragmentationStats::with_dead_info(
            total_capacity,
            reachable_edges,
            dead_entries,
            wasted_capacity,
        ))
    }

    /// Whole-set fragmentation ratio, summed across groups.
    /// Observation metric only; collection triggers use per-vertex counts.
    /// Shares the single wasted-share caliber through the summed snapshot
    /// below instead of a second aggregation loop.
    pub fn fragmentation_ratio(&self) -> f32 {
        self.fragmentation_stats()
            .map(|stats| stats.fragmentation_ratio())
            .unwrap_or(0.0)
    }

    /// Estimate wasted bytes due to fragmentation, summed across groups.
    pub fn wasted_bytes_estimate(&self) -> usize {
        self.shards
            .values()
            .map(|shard| shard.variant.wasted_bytes_estimate())
            .sum()
    }

    /// Approximate memory usage in bytes, summed across existing groups.
    /// Missing groups consume nothing, so sparse tables stay proportional to
    /// materialized groups rather than the endpoint span.
    pub fn used_memory_size(&self) -> usize {
        if self.strategy == EdgeStrategy::None {
            return std::mem::size_of::<Self>();
        }
        self.shards
            .values()
            .map(|shard| shard.variant.used_memory_size())
            .sum::<usize>()
            + std::mem::size_of::<Self>()
    }
}

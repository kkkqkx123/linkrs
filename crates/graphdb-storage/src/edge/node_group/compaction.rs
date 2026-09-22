//! Reclaim and compaction surface: per-region physical observation, merge
//! scope selection, and the compaction passes driven by the reclaim hint.
//!
//! The trigger is always the per-row reclaimable count; density only widens
//! the merge scope, so sparse spans never drag clean rows along.

use graphdb_core::types::{EdgeId, Timestamp};

use super::super::MutableCsrTrait;
use super::{group_id_for, region_local_range, CsrShardSet};

/// Region density at or above which a dirty region merges as a whole.
/// Below it only rows holding reclaimable entries are visited.
///
/// Source: 0.4 keeps region-wide merges for regions where at least two in
/// five slots are live, so mostly-dead regions compact row by row instead of
/// rewriting clean rows. Recommended range 0.3..=0.5. Retuning requires a
/// region-compaction benchmark proving the wider scope pays for itself.
pub const REGION_MERGE_MIN_DENSITY: f32 = 0.4;
/// Group density at or above which a multi-region dirty span merges at
/// group scope. Below it merges stay region-scoped.
///
/// Source: 0.65 reserves group-wide merges for spans that are nearly all
/// live, where per-region passes would revisit the same dense rows.
/// Recommended range 0.5..=0.8. Retuning requires a group-compaction
/// benchmark proving the wider scope pays for itself.
pub const GROUP_MERGE_MIN_DENSITY: f32 = 0.65;

/// Merge scope selected for a dirty region. The trigger is always the
/// per-row reclaimable count; density only widens the scope. Larger scopes
/// require higher density so sparse spans never drag clean rows along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionMergeScope {
    /// Only rows holding reclaimable entries are visited.
    Row,
    /// The whole dirty region is compacted row by row.
    Region,
    /// The dirty span merges at group scope.
    Group,
}

impl CsrShardSet {
    /// Set the reclaim hint for the group owning `vid`.
    ///
    /// Used by remap to preserve the hint explicitly instead of relying on
    /// delete side effects.
    pub fn mark_reclaim_hint_for(&mut self, vid: u32) {
        let gid = group_id_for(vid, self.group_bits);
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.reclaim_hint = true;
        }
    }

    /// Whether a group may hold tombstones worth a reclaim scan.
    pub fn group_needs_reclaim_scan(&self, gid: usize) -> bool {
        self.shards
            .get(&gid)
            .map(|shard| shard.reclaim_hint)
            .unwrap_or(false)
    }

    /// Clear the reclaim hint after a pass visited the whole group.
    pub fn clear_reclaim_hint(&mut self, gid: usize) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.reclaim_hint = false;
        }
    }

    /// Physical census of one leaf region: `(live, dead, capacity)` summed
    /// over its rows in global-vid order.
    pub fn region_census(&self, gid: usize, region: usize) -> (usize, usize, usize) {
        let Some(shard) = self.shards.get(&gid) else {
            return (0, 0, 0);
        };
        let (start, end) = region_local_range(region, self.group_size());
        let mut live = 0usize;
        let mut dead = 0usize;
        let mut capacity = 0usize;
        for local in start..end {
            let (l, d, c) = shard.variant.vertex_census(local);
            live += l;
            dead += d;
            capacity += c;
        }
        (live, dead, capacity)
    }

    /// Live entries per unit of reserved capacity of one leaf region.
    pub fn region_density(&self, gid: usize, region: usize) -> f32 {
        let (live, _, capacity) = self.region_census(gid, region);
        if capacity == 0 {
            1.0
        } else {
            live as f32 / capacity as f32
        }
    }

    /// Live entries of one leaf region summed over its rows.
    pub fn region_live(&self, gid: usize, region: usize) -> usize {
        self.region_census(gid, region).0
    }

    /// Reserved capacity minus live entries of one leaf region.
    ///
    /// Steady-state write room under the current per-row reservation; the
    /// region-tail gap model derives the same room from the live count alone.
    pub fn region_gap(&self, gid: usize, region: usize) -> usize {
        let (live, _, capacity) = self.region_census(gid, region);
        capacity.saturating_sub(live)
    }

    /// Entries of one leaf region reclaimable at `cutoff`.
    pub fn region_reclaimable_count(&self, gid: usize, region: usize, cutoff: Timestamp) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let Some(shard) = self.shards.get(&gid) else {
            return 0;
        };
        let (start, end) = region_local_range(region, self.group_size());
        (start..end)
            .map(|local| shard.variant.reclaimable_count(local, cutoff))
            .sum()
    }

    /// Merge scope for one leaf region. The trigger is always the per-row
    /// reclaimable count; density only widens the scope, and larger scopes
    /// require higher density. Returns `None` when the region holds nothing
    /// reclaimable and carries no delete dirt. Thresholds come from the
    /// table configuration so measured write amplification can guide them;
    /// the module constants back the configuration defaults.
    pub fn select_merge_scope(
        &self,
        gid: usize,
        region: usize,
        cutoff: Timestamp,
        region_min_density: f32,
        group_min_density: f32,
    ) -> Option<RegionMergeScope> {
        if self.region_reclaimable_count(gid, region, cutoff) == 0
            && !self.region_needs_rebalance(gid, region)
        {
            return None;
        }
        let region_density = self.region_density(gid, region);
        if region_density < region_min_density {
            return Some(RegionMergeScope::Row);
        }
        let group_live = self
            .group_stats(gid)
            .map(|stats| {
                if stats.capacity == 0 {
                    1.0
                } else {
                    stats.live_edges as f32 / stats.capacity as f32
                }
            })
            .unwrap_or(1.0);
        if self.dirty_region_ids(gid).len() > 1 && group_live >= group_min_density {
            return Some(RegionMergeScope::Group);
        }
        Some(RegionMergeScope::Region)
    }

    /// Compact one leaf region in place, reporting removals. Only rows in
    /// the region window are visited; other regions never move. Marks the
    /// region deleted when entries were dropped so the next checkpoint
    /// merges the rebuilt rows. Rows holding overflow are rebalanced into
    /// primary gaps on the same pass. Frozen groups bypass the per-row walk
    /// and merge once at group scope so one region request never pays one
    /// trailing memmove per row.
    pub fn compact_region_with_reporting(
        &mut self,
        gid: usize,
        region: usize,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if self.is_frozen(gid) {
            return self.compact_group_with_reporting(gid, cutoff, 0.0, on_edge_removed);
        }
        let group_size = self.group_size();
        let (start, end) = region_local_range(region, group_size);
        let Some(shard) = self.shards.get_mut(&gid) else {
            return 0;
        };
        let mut removed = 0usize;
        for local in start..end {
            removed += shard
                .variant
                .compact_vertex_with_reporting(local, cutoff, on_edge_removed);
            if shard.variant.row_gap(local) == 0 {
                shard.variant.rebalance_row(local);
            }
        }
        if removed > 0 {
            shard.dirty.deleted = true;
            if let Some(slot) = shard.regions.get_mut(region) {
                slot.deleted = true;
            }
        }
        removed
    }

    /// Reclaim a listed local-row set of one frozen group in one pass.
    ///
    /// Collect-then-merge entry for frozen groups: callers gather reclaimable
    /// rows across regions and merge once instead of paying one trailing
    /// memmove per row. Mutable groups report zero; their rows use the
    /// regular per-row path. Marks the group deleted when entries drop so
    /// the next checkpoint persists the rebuilt base.
    pub fn compact_frozen_rows_batched(
        &mut self,
        gid: usize,
        locals: &[u32],
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let Some(shard) = self.shards.get_mut(&gid) else {
            return 0;
        };
        let removed = shard
            .variant
            .compact_frozen_rows_batched(locals, cutoff, on_edge_removed);
        if removed > 0 {
            shard.dirty.deleted = true;
            for region in shard.regions.iter_mut() {
                region.deleted = true;
            }
        }
        removed
    }

    /// Compact one group in place, reporting removals. Marks the group
    /// dirty so the next checkpoint persists the rebuilt rows.
    pub fn compact_group_with_reporting(
        &mut self,
        gid: usize,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let Some(shard) = self.shards.get_mut(&gid) else {
            return 0;
        };
        let removed =
            shard
                .variant
                .compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed);
        if removed > 0 {
            shard.dirty.deleted = true;
            for region in shard.regions.iter_mut() {
                region.deleted = true;
            }
        }
        removed
    }

    /// Compact with per-edge removal reporting across all groups.
    /// Only groups that actually dropped entries are marked dirty so clean
    /// groups are never dragged into the next checkpoint.
    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let mut removed = 0usize;
        for shard in self.shards.values_mut() {
            let before = removed;
            removed +=
                shard
                    .variant
                    .compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed);
            if removed > before {
                shard.dirty.deleted = true;
                for region in shard.regions.iter_mut() {
                    region.deleted = true;
                }
            }
        }
        removed
    }
}

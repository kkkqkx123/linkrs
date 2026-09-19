//! Write-path dirt model and the set-level dirt surface.
//!
//! Leaf regions carry the three-kind dirt; group dirt is the OR over its
//! regions. Checkpoints clear dirt at group or region granularity so clean
//! scopes are never rewritten. `column_updated` traces property-only writes
//! for precise property flushing without forcing topology work.

use super::{group_id_for, local_vid, region_id_for_local, CsrShardSet};

/// Write-path dirt of one leaf region. Cleared for regions a checkpoint
/// persisted. Topology checkpoints rewrite or append-persist a region exactly
/// when `inserted` or `deleted` is set; `column_updated` traces
/// property-only writes for observability without forcing topology work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegionDirty {
    pub inserted: bool,
    pub deleted: bool,
    pub column_updated: bool,
}

impl RegionDirty {
    pub fn is_dirty(self) -> bool {
        self.inserted || self.deleted
    }

    pub fn is_column_dirty(self) -> bool {
        self.column_updated
    }
}

/// Write-path dirt of one group: the OR over its leaf regions. Cleared for
/// groups a checkpoint persisted.
///
/// Topology checkpoints rewrite a group exactly when `inserted` or `deleted`
/// is set; `column_updated` precisely traces property-only writes to their
/// owning groups without forcing a topology rewrite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupDirty {
    pub inserted: bool,
    pub deleted: bool,
    pub column_updated: bool,
}

impl GroupDirty {
    pub fn is_dirty(self) -> bool {
        self.inserted || self.deleted
    }

    /// Sampled column-dirt caliber, not a correctness signal. Topology
    /// checkpoints never consult it; property flushing consult the
    /// table-level flag.
    pub fn is_column_dirty(self) -> bool {
        self.column_updated
    }
}

/// Checkpoint class derived from group dirt before it is cleared.
///
/// Any delete dirt (including the full-dirty mark left by topology-wide
/// rebuilds) makes the checkpoint a rebalance; insert-only or
/// column-only dirt flushes the memory append layer alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeCheckpointKind {
    AppendOnly,
    Rebalance,
}

impl CsrShardSet {
    /// Dirt of one group; missing groups report clean.
    pub fn group_dirty(&self, gid: usize) -> GroupDirty {
        self.shards
            .get(&gid)
            .map(|shard| shard.dirty)
            .unwrap_or_default()
    }

    /// Ids of groups holding uncheckpointed writes.
    pub fn dirty_group_ids(&self) -> Vec<usize> {
        self.shards
            .iter()
            .filter_map(|(gid, shard)| shard.dirty.is_dirty().then_some(*gid))
            .collect()
    }

    /// Ids of groups holding property-only write traces.
    ///
    /// Precise write-time marking: every property write marks its owning
    /// group, so the flush can limit property shards to traced owners. The
    /// table-level property flag remains as a correctness insurance that
    /// rewrites all owners when no trace exists, but the regular path always
    /// carries a trace.
    pub fn column_dirty_group_ids(&self) -> Vec<usize> {
        self.shards
            .iter()
            .filter_map(|(gid, shard)| shard.dirty.is_column_dirty().then_some(*gid))
            .collect()
    }

    /// Checkpoint class for the current dirt without clearing it.
    pub fn checkpoint_kind(&self) -> EdgeCheckpointKind {
        let rebalance = self.shards.values().any(|shard| shard.dirty.deleted);
        if rebalance {
            EdgeCheckpointKind::Rebalance
        } else {
            EdgeCheckpointKind::AppendOnly
        }
    }

    /// Trace a property-only write to the group owning `vid`.
    ///
    /// Precise write-time marking: the owning group is materialized when
    /// missing (unless the direction stores nothing) so every property write
    /// leaves a group trace. The table-level property flag remains as a
    /// correctness insurance, but the regular path never needs it.
    pub fn mark_column_updated_for(&mut self, vid: u32) {
        let gid = group_id_for(vid, self.group_bits);
        let rid = region_id_for_local(local_vid(vid, self.group_bits));
        if !self.shards.contains_key(&gid) {
            let _ = self.ensure_group_id(gid);
        }
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.dirty.column_updated = true;
            if let Some(region) = shard.regions.get_mut(rid) {
                region.column_updated = true;
            }
        }
    }

    pub fn clear_group_dirty(&mut self, gid: usize) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.dirty = GroupDirty::default();
            for region in shard.regions.iter_mut() {
                *region = RegionDirty::default();
            }
        }
    }

    pub fn clear_all_dirty(&mut self) {
        for shard in self.shards.values_mut() {
            shard.dirty = GroupDirty::default();
            for region in shard.regions.iter_mut() {
                *region = RegionDirty::default();
            }
        }
    }

    /// Clear precise column-only traces after a flush. Insert and delete
    /// dirt is owned by the per-group flush above and never touched here.
    pub fn clear_all_column_dirty(&mut self) {
        for shard in self.shards.values_mut() {
            shard.dirty.column_updated = false;
            for region in shard.regions.iter_mut() {
                region.column_updated = false;
            }
        }
    }

    /// Mark every group dirty so the next checkpoint rewrites the full
    /// direction. Used after topology-wide rebuilds such as vertex remapping,
    /// where clean-group skipping would otherwise persist stale group files.
    pub fn mark_all_dirty(&mut self) {
        for shard in self.shards.values_mut() {
            shard.dirty = GroupDirty {
                inserted: true,
                deleted: true,
                column_updated: true,
            };
            for region in shard.regions.iter_mut() {
                *region = RegionDirty {
                    inserted: true,
                    deleted: true,
                    column_updated: true,
                };
            }
        }
    }

    /// Whether a group holds uncheckpointed writes.
    pub fn needs_checkpoint(&self, gid: usize) -> bool {
        self.group_dirty(gid).is_dirty()
    }

    /// Whether one group carries delete dirt and needs a base merge rather
    /// than an append-only persist.
    pub fn group_needs_rebalance(&self, gid: usize) -> bool {
        self.group_dirty(gid).deleted
    }

    /// Dirt of one leaf region; missing groups or regions report clean.
    pub fn region_dirty(&self, gid: usize, region: usize) -> RegionDirty {
        self.shards
            .get(&gid)
            .and_then(|shard| shard.regions.get(region))
            .copied()
            .unwrap_or_default()
    }

    /// Ids of dirty leaf regions inside one group.
    pub fn dirty_region_ids(&self, gid: usize) -> Vec<usize> {
        self.shards
            .get(&gid)
            .map(|shard| {
                shard
                    .regions
                    .iter()
                    .enumerate()
                    .filter_map(|(rid, region)| region.is_dirty().then_some(rid))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether one leaf region holds uncheckpointed topology writes.
    pub fn region_needs_checkpoint(&self, gid: usize, region: usize) -> bool {
        self.region_dirty(gid, region).is_dirty()
    }

    /// Whether one leaf region carries delete dirt and needs a base merge
    /// rather than an append-only persist.
    pub fn region_needs_rebalance(&self, gid: usize, region: usize) -> bool {
        self.region_dirty(gid, region).deleted
    }

    pub fn clear_region_dirty(&mut self, gid: usize, region: usize) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            if let Some(slot) = shard.regions.get_mut(region) {
                *slot = RegionDirty::default();
            }
            if shard.regions.iter().all(|r| !r.is_dirty()) {
                shard.dirty.inserted = false;
                shard.dirty.deleted = false;
            } else {
                shard.dirty.inserted = shard.regions.iter().any(|r| r.inserted);
                shard.dirty.deleted = shard.regions.iter().any(|r| r.deleted);
            }
        }
    }

    pub(super) fn mark_region_insert(&mut self, gid: usize, local: u32) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.dirty.inserted = true;
            let rid = region_id_for_local(local);
            if let Some(region) = shard.regions.get_mut(rid) {
                region.inserted = true;
            }
        }
    }

    pub(super) fn mark_region_delete(&mut self, gid: usize, local: u32) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.dirty.deleted = true;
            let rid = region_id_for_local(local);
            if let Some(region) = shard.regions.get_mut(rid) {
                region.deleted = true;
            }
        }
    }
}

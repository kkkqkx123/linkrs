//! Container lifecycle, group materialization, routing and bulk insert.
//!
//! The set holds one `Shard` per existing group in a sparse `BTreeMap`:
//! reads route through `route` and never create groups, writes create
//! missing groups on demand. Group-space resets are construction/load
//! paths only.

use graphdb_core::types::{EdgeId, EdgeStrategy, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};
use std::collections::BTreeMap;

use super::super::{CsrBase, CsrVariant, MutableCsrTrait, Nbr};
use super::{
    group_id_for, group_size, local_vid, regions_per_group, validate_group_bits, CsrShardSet,
    GroupDirty, RegionDirty, Shard, ShardAppendLog,
};

impl CsrShardSet {
    pub fn new(
        strategy: EdgeStrategy,
        group_bits: u32,
        overflow_chunk_edges: usize,
    ) -> StorageResult<Self> {
        validate_group_bits(group_bits)?;
        if overflow_chunk_edges == 0 {
            return Err(StorageError::invalid_operation(
                "overflow_chunk_edges must be greater than zero",
            ));
        }
        let mut set = Self {
            strategy,
            group_bits,
            overflow_chunk_edges,
            tombstone_reuse_cutoff: Timestamp::MAX,
            shards: BTreeMap::new(),
        };
        if strategy != EdgeStrategy::None {
            set.shards.insert(
                0,
                Shard {
                    variant: set.fresh_variant()?,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(set.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                },
            );
        }
        Ok(set)
    }

    pub fn strategy(&self) -> EdgeStrategy {
        self.strategy
    }

    pub fn group_bits(&self) -> u32 {
        self.group_bits
    }

    pub fn group_size(&self) -> usize {
        group_size(self.group_bits)
    }

    pub fn group_count(&self) -> usize {
        self.shards.len()
    }

    /// Sorted existing group ids. Sparse holes are absent: they read as
    /// empty, never consume memory and never produce files.
    pub fn existing_group_ids(&self) -> Vec<usize> {
        self.shards.keys().copied().collect()
    }

    /// One past the largest materialized group, or zero when empty. Only for
    /// diagnostics; flush and reclaim paths iterate existing ids alone so a
    /// wide sparse span never drags holes along.
    pub fn group_span(&self) -> usize {
        self.shards.keys().next_back().map_or(0, |max| max + 1)
    }

    /// Address-span row count: highest group upper bound minus lowest group
    /// lower bound, holes included.
    ///
    /// This is the true address upper bound for preallocation and range
    /// validation. `vertex_capacity` instead reports materialized rows only
    /// (memory-proportional); callers must pick by purpose and never mix the
    /// two calibers.
    pub fn address_span_rows(&self) -> usize {
        let (Some(min), Some(max)) = (self.shards.keys().next(), self.shards.keys().next_back())
        else {
            return 0;
        };
        (max - min + 1) * self.group_size()
    }

    fn fresh_variant(&self) -> StorageResult<CsrVariant> {
        let mut variant = CsrVariant::from_strategy_with_overflow(
            self.strategy,
            self.group_size(),
            0,
            self.overflow_chunk_edges,
        )?;
        variant.set_tombstone_reuse_cutoff(self.tombstone_reuse_cutoff);
        Ok(variant)
    }

    /// Refresh the hot-path tombstone reuse cutoff on every group. The
    /// table maintenance pass calls this with its watermark-derived bound;
    /// the sentinel disables reuse.
    pub fn set_tombstone_reuse_cutoff(&mut self, cutoff: Timestamp) {
        self.tombstone_reuse_cutoff = cutoff;
        for shard in self.shards.values_mut() {
            shard.variant.set_tombstone_reuse_cutoff(cutoff);
        }
    }

    pub(super) fn ensure_group_for(&mut self, vid: u32) -> StorageResult<usize> {
        if self.strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            ));
        }
        let gid = group_id_for(vid, self.group_bits);
        if !self.shards.contains_key(&gid) {
            self.shards.insert(
                gid,
                Shard {
                    variant: self.fresh_variant()?,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                },
            );
        }
        Ok(gid)
    }

    /// Ensure one group exists without routing a vertex id. Used when loading
    /// an explicit existing-group list and when orphan timestamp shards fall
    /// back to group zero.
    pub fn ensure_group_id(&mut self, gid: usize) -> StorageResult<()> {
        if self.strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            ));
        }
        if !self.shards.contains_key(&gid) {
            self.shards.insert(
                gid,
                Shard {
                    variant: self.fresh_variant()?,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                },
            );
        }
        Ok(())
    }

    pub(super) fn route(&self, vid: u32) -> Option<(usize, u32)> {
        let gid = group_id_for(vid, self.group_bits);
        self.shards
            .get(&gid)
            .map(|_| (gid, local_vid(vid, self.group_bits)))
    }

    /// Bulk insert pre-grouped edges with one reservation per touched row.
    ///
    /// Input is `(src, dst, edge_id)` triples at global addresses; rows are
    /// grouped by shard, each `Multiple` group is written through its bulk
    /// path (single reservation, single live-set rebuild per row), and
    /// `Single` groups fall back to per-edge inserts. Dirt and append-log
    /// entries are recorded per inserted edge exactly like the single-edge
    /// path. Duplicate keys are rejected before any write when
    /// `check_duplicates` is set.
    pub fn batch_put_edges(
        &mut self,
        edges: &[(u32, VertexId, EdgeId)],
        ts: Timestamp,
        check_duplicates: bool,
    ) -> StorageResult<usize> {
        if self.strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            ));
        }
        let mut by_group: BTreeMap<usize, Vec<(u32, VertexId, EdgeId)>> = BTreeMap::new();
        for (src, dst, edge_id) in edges {
            let gid = group_id_for(*src, self.group_bits);
            by_group
                .entry(gid)
                .or_default()
                .push((*src, *dst, *edge_id));
        }
        let mut inserted = 0usize;
        for (gid, group_edges) in by_group {
            let group_bits = self.group_bits;
            self.ensure_group_id(gid)?;
            let is_multiple = matches!(
                self.shards.get(&gid).map(|shard| &shard.variant),
                Some(CsrVariant::Multiple(_))
            );
            if is_multiple {
                let batch: Vec<(u32, Vec<(u32, i64, EdgeId, Timestamp)>)> = {
                    let mut rows: BTreeMap<u32, Vec<(u32, i64, EdgeId, Timestamp)>> =
                        BTreeMap::new();
                    for (src, dst, edge_id) in &group_edges {
                        let local = local_vid(*src, group_bits);
                        let (vid, rank) = dst.decode_edge_endpoint();
                        let endpoint = vid.as_u64().unwrap_or(0) as u32;
                        rows.entry(local)
                            .or_default()
                            .push((endpoint, rank, *edge_id, ts));
                    }
                    rows.into_iter().collect()
                };
                {
                    let shard = self.shards.get_mut(&gid).ok_or_else(|| {
                        StorageError::invalid_operation(format!("missing group {} on insert", gid))
                    })?;
                    let CsrVariant::Multiple(csr) = &mut shard.variant else {
                        return Err(StorageError::invalid_operation(format!(
                            "missing group {} on insert",
                            gid
                        )));
                    };
                    inserted += csr.batch_put_edges(&batch, check_duplicates)?;
                }
                for (src, dst, edge_id) in &group_edges {
                    let local = local_vid(*src, group_bits);
                    let (vid, rank) = dst.decode_edge_endpoint();
                    let endpoint = vid.as_u64().unwrap_or(0) as u32;
                    let nbr = Nbr::with_create_ts(endpoint, rank, *edge_id, ts);
                    self.mark_region_insert(gid, local);
                    self.record_append_insert(gid, local, nbr);
                }
            } else {
                for (src, dst, edge_id) in group_edges {
                    let local = local_vid(src, group_bits);
                    {
                        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
                            StorageError::invalid_operation(format!(
                                "missing group {} on insert",
                                gid
                            ))
                        })?;
                        shard.variant.insert_edge(local, dst, edge_id, ts)?;
                    }
                    let (vid, rank) = dst.decode_edge_endpoint();
                    let endpoint = vid.as_u64().unwrap_or(0) as u32;
                    let nbr = Nbr::with_create_ts(endpoint, rank, edge_id, ts);
                    self.mark_region_insert(gid, local);
                    self.record_append_insert(gid, local, nbr);
                    inserted += 1;
                }
            }
        }
        Ok(inserted)
    }

    /// Borrow one persisted group for checkpoint writes.
    pub fn group_variant(&self, gid: usize) -> Option<&CsrVariant> {
        self.shards.get(&gid).map(|shard| &shard.variant)
    }

    /// Mutably borrow one group for checkpoint loads.
    pub fn group_variant_mut(&mut self, gid: usize) -> Option<&mut CsrVariant> {
        self.shards.get_mut(&gid).map(|shard| &mut shard.variant)
    }

    /// Resize the group space for construction only: grows with fresh
    /// variants, shrinks by dropping trailing groups. Construction and load
    /// paths only; normal writes grow through the routed insert path and
    /// never reset whole groups. Refuses to run on a table holding physical
    /// rows so a mistaken call cannot silently discard data.
    pub(crate) fn resize_groups(&mut self, count: usize) -> StorageResult<()> {
        if self.has_physical_rows() {
            return Err(StorageError::invalid_operation(
                "refusing to reset groups of a non-empty table; use group migration instead"
                    .to_string(),
            ));
        }
        if self.strategy == EdgeStrategy::None {
            if count != 0 {
                return Err(StorageError::deserialize_error(format!(
                    "group count {} does not match no-edge strategy",
                    count
                )));
            }
            self.shards.clear();
            return Ok(());
        }
        let ids: Vec<u32> = (0..count).map(|gid| gid as u32).collect();
        self.set_groups(&ids)
    }

    /// Whether any shard holds a physical row, live or tombstoned.
    fn has_physical_rows(&self) -> bool {
        self.shards
            .values()
            .any(|shard| shard.variant.iter_all().next().is_some())
    }

    /// Materialize exactly the listed groups for loading a sparse manifest.
    /// Missing groups stay absent: they read as empty and never produce
    /// files. Unlisted materialized groups are dropped. Construction and
    /// load paths only; callers must go through the normal group migration
    /// path instead of resetting a non-empty table.
    pub(crate) fn set_groups(&mut self, ids: &[u32]) -> StorageResult<()> {
        if self.has_physical_rows() {
            return Err(StorageError::invalid_operation(
                "refusing to reset groups of a non-empty table; use group migration instead"
                    .to_string(),
            ));
        }
        if self.strategy == EdgeStrategy::None {
            if !ids.is_empty() {
                return Err(StorageError::deserialize_error(format!(
                    "group list {:?} does not match no-edge strategy",
                    ids
                )));
            }
            self.shards.clear();
            return Ok(());
        }
        let mut wanted: Vec<usize> = ids.iter().map(|id| *id as usize).collect();
        wanted.sort_unstable();
        wanted.dedup();
        let mut fresh = BTreeMap::new();
        for gid in wanted {
            fresh.insert(
                gid,
                Shard {
                    variant: self.fresh_variant()?,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                },
            );
        }
        if fresh.is_empty() {
            fresh.insert(
                0,
                Shard {
                    variant: self.fresh_variant()?,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                },
            );
        }
        self.shards = fresh;
        self.clear_all_dirty();
        Ok(())
    }

    /// Load one group payload without marking it dirty. The reclaim hint
    /// stays set: loaded groups may hold tombstones the next reclaim pass
    /// must inspect once.
    pub fn load_group(&mut self, gid: usize, data: &[u8]) -> StorageResult<()> {
        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
            StorageError::deserialize_error(format!("group {} out of range on load", gid))
        })?;
        shard.variant.load(data)?;
        shard.dirty = GroupDirty::default();
        for region in shard.regions.iter_mut() {
            *region = RegionDirty::default();
        }
        shard.append.clear();
        shard.reclaim_hint = true;
        Ok(())
    }

    /// Drop every group holding no physical entries. Tombstone-bearing groups
    /// are retained so snapshot history before the cutoff survives. Non-empty
    /// strategies keep at least group zero so an empty table stays
    /// addressable. Intermediate holes are never materialized, so only
    /// existing empty groups are dropped and no empty files are produced.
    pub fn truncate_trailing_empty_groups(&mut self) {
        let empty: Vec<usize> = self
            .shards
            .iter()
            .filter_map(|(gid, shard)| shard.variant.iter_all().next().is_none().then_some(*gid))
            .collect();
        for gid in empty {
            if self.shards.len() <= 1 {
                break;
            }
            self.shards.remove(&gid);
        }
        if self.strategy != EdgeStrategy::None && self.shards.is_empty() {
            if let Ok(variant) = self.fresh_variant() {
                self.shards.insert(
                    0,
                    Shard {
                        variant,
                        dirty: GroupDirty::default(),
                        regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                        append: ShardAppendLog::default(),
                        reclaim_hint: false,
                    },
                );
            }
        }
    }

    /// Clear all edges, keeping the group space. Marks surviving groups
    /// dirty so the next checkpoint persists the cleared state.
    pub fn clear(&mut self) {
        for shard in self.shards.values_mut() {
            shard.variant.clear();
            shard.dirty = GroupDirty {
                inserted: false,
                deleted: true,
                column_updated: false,
            };
            for region in shard.regions.iter_mut() {
                *region = RegionDirty {
                    inserted: false,
                    deleted: true,
                    column_updated: false,
                };
            }
            shard.append.clear();
            shard.reclaim_hint = false;
        }
    }
}

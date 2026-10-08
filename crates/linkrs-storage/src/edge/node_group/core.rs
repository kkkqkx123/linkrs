//! Container lifecycle, group materialization, routing and bulk insert.
//!
//! The set holds one `Shard` per existing group in a sparse `BTreeMap`:
//! reads route through `route` and never create groups, writes create
//! missing groups on demand. Group-space resets are construction/load
//! paths only.

use linkrs_core::types::{EdgeStrategy, Timestamp};
use linkrs_core::{StorageError, StorageResult};
use std::collections::BTreeMap;

use super::super::{CsrBase, CsrVariant, MappedFrozen, RecordForm, NO_EDGES_STORED_MSG};
use super::{
    group_id_for, group_size, local_vid, regions_per_group, validate_group_bits, CsrShardSet,
    GroupDirty, RegionDirty, Shard, ShardAppendLog,
};

mod batch;
mod bundled_values;

impl CsrShardSet {
    /// Routing-cache invalidation family for cache coherence.
    ///
    /// Group drops, load replacement and per-group updates converge on the
    /// per-group entry; group-space resets use the all-routes entry. The
    /// cache stays an accelerator only: misses never populate for missing
    /// groups, so absent rows read as empty and no invalidation path can
    /// leave a stale triple behind.
    pub(crate) fn invalidate_group_route(&self, gid: usize) {
        self.route_cache.invalidate_gid(gid);
    }

    /// Invalidate every cached route after a group-space reset.
    pub(crate) fn invalidate_all_routes(&self) {
        self.route_cache.clear();
    }

    /// Routing-cache hit rate for observability. Capacity stays fixed;
    /// this only reports so operators can tell whether hot lookups hit.
    pub fn route_hit_rate(&self) -> f32 {
        self.route_cache.hit_rate()
    }

    /// Routing-cache collision count since creation. Collisions share the
    /// miss counter; this separates true empty misses from evictions so
    /// operators can tell whether 128 slots still cover the working set.
    pub fn route_collision_count(&self) -> usize {
        self.route_cache.collision_count()
    }

    pub fn new(
        strategy: EdgeStrategy,
        group_bits: u32,
        overflow_chunk_edges: usize,
        record_form: RecordForm,
    ) -> StorageResult<Self> {
        validate_group_bits(group_bits)?;
        if overflow_chunk_edges == 0 {
            return Err(StorageError::invalid_operation(
                "overflow_chunk_edges must be greater than zero",
            ));
        }
        // Central single-plus-inline rule lives with the schema helpers.
        super::super::validate_strategy_form(strategy, record_form)?;
        let mut set = Self {
            strategy,
            group_bits,
            overflow_chunk_edges,
            record_form,
            tombstone_reuse_cutoff: Timestamp::MAX,
            shards: BTreeMap::new(),
            route_cache: super::RouteCache::new(),
        };
        if strategy != EdgeStrategy::None {
            set.shards.insert(
                0,
                Shard {
                    variant: set.fresh_variant()?,
                    mapped: None,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(set.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                    dead_entries: 0,
                },
            );
        }
        Ok(set)
    }

    pub fn strategy(&self) -> EdgeStrategy {
        self.strategy
    }

    pub fn record_form(&self) -> RecordForm {
        self.record_form
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

    pub(crate) fn fresh_variant(&self) -> StorageResult<CsrVariant> {
        // Local guard for the constructor invariant through the shared helper.
        super::super::validate_strategy_form(self.strategy, self.record_form)?;
        // Only the columnar multi-edge store reuses primary tombstones, so
        // only that branch seeds the reuse cutoff. Pure and bundled groups
        // hold no reuse state and intentionally ignore the hint.
        let variant = match self.record_form {
            RecordForm::Pure => CsrVariant::Pure(Box::new(
                super::super::pure_csr::PureTopologyCsr::with_overflow_chunk_edges(
                    self.group_size(),
                    0,
                    self.overflow_chunk_edges,
                ),
            )),
            RecordForm::Bundled => CsrVariant::Bundled(Box::new(
                super::super::bundled_csr::BundledCsr::with_overflow_chunk_edges(
                    self.group_size(),
                    0,
                    self.overflow_chunk_edges,
                ),
            )),
            RecordForm::Columnar => {
                let mut v = CsrVariant::from_strategy_with_overflow(
                    self.strategy,
                    self.group_size(),
                    0,
                    self.overflow_chunk_edges,
                )?;
                v.set_tombstone_reuse_cutoff(self.tombstone_reuse_cutoff);
                return Ok(v);
            }
        };
        Ok(variant)
    }

    /// Refresh the hot-path tombstone reuse cutoff on every group.
    ///
    /// Only watermark-derived bounds refresh; the sentinel disables reuse.
    /// See [`MutableCsr::set_tombstone_reuse_cutoff`](super::super::MutableCsr::set_tombstone_reuse_cutoff)
    /// for the single freshness contract.
    pub fn set_tombstone_reuse_cutoff(&mut self, cutoff: Timestamp) {
        self.tombstone_reuse_cutoff = self.tombstone_reuse_cutoff.min(cutoff);
        for shard in self.shards.values_mut() {
            shard.variant.set_tombstone_reuse_cutoff(cutoff);
        }
    }

    /// Fresh watermark refresh allowing widening on every group.
    pub fn refresh_tombstone_reuse_cutoff(&mut self, fresh: Timestamp) {
        self.tombstone_reuse_cutoff = fresh;
        for shard in self.shards.values_mut() {
            shard.variant.refresh_tombstone_reuse_cutoff(fresh);
        }
    }

    /// Drop the reuse hint back to the disabled sentinel on every group.
    ///
    /// Stale path of the same contract: no fresh watermark means no reuse.
    pub fn clear_tombstone_reuse_cutoff(&mut self) {
        self.tombstone_reuse_cutoff = Timestamp::MAX;
        for shard in self.shards.values_mut() {
            shard.variant.clear_tombstone_reuse_cutoff();
        }
    }

    /// Current reuse cutoff for observability and tests. `Timestamp::MAX`
    /// means reuse is disabled.
    pub fn tombstone_reuse_cutoff(&self) -> Timestamp {
        self.tombstone_reuse_cutoff
    }

    pub(super) fn ensure_group_for(&mut self, vid: u32) -> StorageResult<usize> {
        if self.strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                NO_EDGES_STORED_MSG.to_string(),
            ));
        }
        let gid = group_id_for(vid, self.group_bits);
        if !self.shards.contains_key(&gid) {
            self.shards.insert(
                gid,
                Shard {
                    variant: self.fresh_variant()?,
                    mapped: None,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                    dead_entries: 0,
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
                NO_EDGES_STORED_MSG.to_string(),
            ));
        }
        if !self.shards.contains_key(&gid) {
            self.shards.insert(
                gid,
                Shard {
                    variant: self.fresh_variant()?,
                    mapped: None,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                    dead_entries: 0,
                },
            );
        }
        Ok(())
    }

    pub(super) fn route(&self, vid: u32) -> Option<(usize, u32)> {
        // Hot-row fast path: repeats skip the group map search. Cached
        // triples are only stored for existing groups and removals
        // invalidate, so a hit is authoritative without rechecking the map.
        if let Some(hit) = self.route_cache.lookup(vid) {
            // Debug-only coherence check: cached triples are only stored for
            // existing groups and removals invalidate first, so a hit must
            // always agree with the map. A failure here means a removal path
            // forgot to invalidate, never a benign race.
            debug_assert!(
                self.shards.contains_key(&hit.0),
                "stale route cache entry for vertex {vid}: group {} is gone",
                hit.0,
            );
            return Some(hit);
        }
        let gid = group_id_for(vid, self.group_bits);
        let local = local_vid(vid, self.group_bits);
        if self.shards.contains_key(&gid) {
            self.route_cache.insert(vid, gid, local);
            Some((gid, local))
        } else {
            None
        }
    }

    /// Borrow one persisted group for checkpoint writes.
    pub fn group_variant(&self, gid: usize) -> Option<&CsrVariant> {
        self.shards.get(&gid).map(|shard| &shard.variant)
    }

    /// Mutably borrow one group for checkpoint loads.
    pub fn group_variant_mut(&mut self, gid: usize) -> Option<&mut CsrVariant> {
        self.shards.get_mut(&gid).map(|shard| &mut shard.variant)
    }

    /// Derived serving cache of one group, if the checkpoint load attached one.
    ///
    /// Clones share one mapping through the reference count, so snapshotting
    /// the handle for a reader is cheap. Only frozen groups ever carry one.
    pub fn group_mapped(&self, gid: usize) -> Option<MappedFrozen> {
        self.shards.get(&gid).and_then(|shard| shard.mapped.clone())
    }

    /// Whether one group serves hot reads from its derived mapping.
    pub fn group_has_mapped(&self, gid: usize) -> bool {
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.mapped.is_some())
    }

    /// Attach a derived serving cache to a frozen group. Non-frozen groups
    /// reject the handle so residency never shadows a writable heap.
    pub fn set_group_mapped(&mut self, gid: usize, mapped: MappedFrozen) -> StorageResult<()> {
        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
            StorageError::deserialize_error(format!("group {} missing on mapped attach", gid))
        })?;
        if !matches!(shard.variant, CsrVariant::Frozen(_)) {
            return Err(StorageError::invalid_operation(format!(
                "group {} is not frozen, refusing a mapped serving cache",
                gid
            )));
        }
        shard.mapped = Some(mapped);
        Ok(())
    }

    /// Drop one group serving cache, falling back to heap serving.
    pub fn clear_group_mapped(&mut self, gid: usize) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.mapped = None;
        }
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
            self.invalidate_all_routes();
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
            self.invalidate_all_routes();
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
                    mapped: None,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                    dead_entries: 0,
                },
            );
        }
        if fresh.is_empty() {
            fresh.insert(
                0,
                Shard {
                    variant: self.fresh_variant()?,
                    mapped: None,
                    dirty: GroupDirty::default(),
                    regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                    append: ShardAppendLog::default(),
                    reclaim_hint: false,
                    dead_entries: 0,
                },
            );
        }
        self.shards = fresh;
        self.invalidate_all_routes();
        self.clear_all_dirty();
        Ok(())
    }

    /// Load one group payload without marking it dirty. The tombstone
    /// counter is derived from the loaded payload in one pass, and the
    /// reclaim hint follows it, so clean groups skip reclaim scans without
    /// paying one audit walk after every load.
    pub fn load_group(&mut self, gid: usize, data: &[u8]) -> StorageResult<()> {
        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
            StorageError::deserialize_error(format!("group {} out of range on load", gid))
        })?;
        shard.variant.load(data)?;
        shard.mapped = None;
        shard.dirty = GroupDirty::default();
        for region in shard.regions.iter_mut() {
            *region = RegionDirty::default();
        }
        shard.append.clear();
        let dead = shard
            .variant
            .iter_all()
            .filter(|(_, nbr)| {
                nbr.edge_id != super::super::INVALID_EDGE_ID && nbr.delete_ts != Timestamp::MAX
            })
            .count();
        shard.dead_entries = dead;
        shard.reclaim_hint = dead > 0;
        self.invalidate_group_route(gid);
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
        let mut removed: Vec<usize> = Vec::new();
        for gid in empty {
            if self.shards.len() <= 1 {
                break;
            }
            self.shards.remove(&gid);
            removed.push(gid);
        }
        if self.strategy != EdgeStrategy::None && self.shards.is_empty() {
            if let Ok(variant) = self.fresh_variant() {
                self.shards.insert(
                    0,
                    Shard {
                        variant,
                        mapped: None,
                        dirty: GroupDirty::default(),
                        regions: vec![RegionDirty::default(); regions_per_group(self.group_size())],
                        append: ShardAppendLog::default(),
                        reclaim_hint: false,
                        dead_entries: 0,
                    },
                );
            }
        }
        // Dropped groups change routing: invalidate their cached rows so
        // later reads route to missing groups as empty instead of hitting
        // stale triples.
        for gid in removed {
            self.invalidate_group_route(gid);
        }
    }

    /// Clear all edges, keeping the group space. Marks surviving groups
    /// dirty so the next checkpoint persists the cleared state.
    ///
    /// Clearing also drops every derived serving cache: the heap form is
    /// emptied and the mapping is outside the heap, so the group falls back
    /// to heap serving. Missing-group reads stay empty and no group is
    /// dropped here.
    pub fn clear(&mut self) {
        for shard in self.shards.values_mut() {
            shard.variant.clear();
            shard.mapped = None;
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
            shard.dead_entries = 0;
        }
    }
}

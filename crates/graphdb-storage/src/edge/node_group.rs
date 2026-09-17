//! Node-group sharding for edge topology.
//!
//! Each edge direction is partitioned by bound-endpoint interval:
//! `group = vid >> group_bits`, `local = vid & (group_size - 1)`.
//! Every group owns one `CsrVariant` holding only the rows of its interval,
//! plus insert/delete/column-update dirt markers driving incremental
//! checkpoints. Neighbor keys keep global endpoint values; only row
//! addressing is local.
//!
//! Properties and visibility stay global by edge id and are not sharded.
//! Groups report leaf-region densities over fixed row windows for collection
//! observability; the checkpoint input-output unit stays the group.

use graphdb_core::types::{EdgeId, EdgeStrategy, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::csr_variant::CsrIterator;
use super::mutable_csr::VertexEdgesIter;
use super::{CsrBase, CsrVariant, FragmentationStats, MutableCsrTrait, Nbr};

/// Default address bits per node group: 12 bits cover 4096 rows.
pub const DEFAULT_NODE_GROUP_BITS: u32 = 12;
/// Rows per leaf region inside a group, for density observability.
/// Density floor below which a group is reported as sparse.
/// Mirrors the packed-row density target of the underlying CSR.
/// Container serialization version for a sharded direction.
pub const SHARD_SET_FORMAT_VERSION: u32 = 1;
/// Manifest version for the per-table group layout file. Old single-file
/// layouts carry no manifest and are rejected, never converted.
pub const GROUP_MANIFEST_VERSION: u32 = 2;

/// Group index for a global vertex id.
pub fn group_id_for(vid: u32, group_bits: u32) -> usize {
    (vid >> group_bits) as usize
}

/// First global vertex id covered by a group.
pub fn group_base(group: usize, group_bits: u32) -> u32 {
    (group as u32) << group_bits
}

/// Rows covered by one group.
pub fn group_size(group_bits: u32) -> usize {
    1usize << group_bits
}

/// Row address inside its group.
pub fn local_vid(vid: u32, group_bits: u32) -> u32 {
    vid & ((group_size(group_bits) as u32).wrapping_sub(1))
}

/// Validate the configured address width.
pub fn validate_group_bits(group_bits: u32) -> StorageResult<()> {
    if group_bits == 0 || group_bits > 20 {
        return Err(StorageError::invalid_operation(format!(
            "node_group_bits must be within 1..=20, got {}",
            group_bits
        )));
    }
    Ok(())
}

/// Write-path dirt of one group. Cleared for groups a checkpoint persisted.
///
/// Topology checkpoints rewrite a group exactly when `inserted` or `deleted`
/// is set; `column_updated` traces property-only writes to their owning
/// groups for observability without forcing a topology rewrite.
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

/// Observability view of one group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeGroupStats {
    pub group: usize,
    pub base: u32,
    pub rows: usize,
    pub live_edges: u64,
    pub capacity: usize,
    pub density: f32,
    pub dirty: GroupDirty,
}


/// Per-table group layout shared by both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableShardManifest {
    pub group_bits: u32,
    pub out_groups: u32,
    pub in_groups: u32,
}

impl TableShardManifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16);
        out.extend_from_slice(&GROUP_MANIFEST_VERSION.to_le_bytes());
        out.extend_from_slice(&self.group_bits.to_le_bytes());
        out.extend_from_slice(&self.out_groups.to_le_bytes());
        out.extend_from_slice(&self.in_groups.to_le_bytes());
        out
    }

    pub fn decode(data: &[u8]) -> StorageResult<Self> {
        if data.len() != 16 {
            return Err(StorageError::deserialize_error(format!(
                "group manifest must be 16 bytes, got {}",
                data.len()
            )));
        }
        let bad_slice = || StorageError::deserialize_error("group manifest slice too short");
        let version = u32::from_le_bytes(data[0..4].try_into().map_err(|_| bad_slice())?);
        if version != GROUP_MANIFEST_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported group manifest version: {}",
                version
            )));
        }
        let group_bits = u32::from_le_bytes(data[4..8].try_into().map_err(|_| bad_slice())?);
        validate_group_bits(group_bits)?;
        let out_groups = u32::from_le_bytes(data[8..12].try_into().map_err(|_| bad_slice())?);
        let in_groups = u32::from_le_bytes(data[12..16].try_into().map_err(|_| bad_slice())?);
        Ok(Self {
            group_bits,
            out_groups,
            in_groups,
        })
    }
}

#[derive(Debug, Clone)]
struct Shard {
    variant: CsrVariant,
    dirty: GroupDirty,
    /// Whether the group may hold tombstones. Set on deletes and on load,
    /// cleared after a reclaim scan visits the whole group. Lets reclaim
    /// passes skip groups that never held a deletion.
    reclaim_hint: bool,
}

/// Sharded topology container for one edge direction.
///
/// Routes every row-addressed operation to the group owning the bound
/// endpoint. Read paths never create groups; write paths create missing
/// groups on demand.
#[derive(Debug, Clone)]
pub struct CsrShardSet {
    strategy: EdgeStrategy,
    group_bits: u32,
    overflow_chunk_edges: usize,
    shards: Vec<Shard>,
}

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
            shards: Vec::new(),
        };
        if strategy != EdgeStrategy::None {
            set.shards.push(Shard {
                variant: set.fresh_variant()?,
                dirty: GroupDirty::default(),
                reclaim_hint: false,
            });
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

    fn fresh_variant(&self) -> StorageResult<CsrVariant> {
        CsrVariant::from_strategy_with_overflow(
            self.strategy,
            self.group_size(),
            0,
            self.overflow_chunk_edges,
        )
    }

    fn ensure_group_for(&mut self, vid: u32) -> StorageResult<usize> {
        if self.strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            ));
        }
        let gid = group_id_for(vid, self.group_bits);
        while self.shards.len() <= gid {
            self.shards.push(Shard {
                variant: self.fresh_variant()?,
                dirty: GroupDirty::default(),
                reclaim_hint: false,
            });
        }
        Ok(gid)
    }

    fn route(&self, vid: u32) -> Option<(usize, u32)> {
        let gid = group_id_for(vid, self.group_bits);
        self.shards
            .get(gid)
            .map(|_| (gid, local_vid(vid, self.group_bits)))
    }

    /// Borrow one persisted group for checkpoint writes.
    pub fn group_variant(&self, gid: usize) -> Option<&CsrVariant> {
        self.shards.get(gid).map(|shard| &shard.variant)
    }

    /// Mutably borrow one group for checkpoint loads.
    pub fn group_variant_mut(&mut self, gid: usize) -> Option<&mut CsrVariant> {
        self.shards.get_mut(gid).map(|shard| &mut shard.variant)
    }

    /// Dirt of one group; missing groups report clean.
    pub fn group_dirty(&self, gid: usize) -> GroupDirty {
        self.shards
            .get(gid)
            .map(|shard| shard.dirty)
            .unwrap_or_default()
    }

    /// Ids of groups holding uncheckpointed writes.
    pub fn dirty_group_ids(&self) -> Vec<usize> {
        self.shards
            .iter()
            .enumerate()
            .filter_map(|(gid, shard)| shard.dirty.is_dirty().then_some(gid))
            .collect()
    }

    /// Ids of groups holding uncheckpointed property-only writes.
    pub fn column_dirty_group_ids(&self) -> Vec<usize> {
        self.shards
            .iter()
            .enumerate()
            .filter_map(|(gid, shard)| shard.dirty.is_column_dirty().then_some(gid))
            .collect()
    }

    /// Checkpoint class for the current dirt without clearing it.
    pub fn checkpoint_kind(&self) -> EdgeCheckpointKind {
        let rebalance = self.shards.iter().any(|shard| shard.dirty.deleted);
        if rebalance {
            EdgeCheckpointKind::Rebalance
        } else {
            EdgeCheckpointKind::AppendOnly
        }
    }

    /// Trace a property-only write to the group owning `vid`.
    ///
    /// Best effort: vids outside the current group space leave no group
    /// trace and rely on the table-level property dirt.
    pub fn mark_column_updated_for(&mut self, vid: u32) {
        let gid = group_id_for(vid, self.group_bits);
        if let Some(shard) = self.shards.get_mut(gid) {
            shard.dirty.column_updated = true;
        }
    }

    pub fn clear_group_dirty(&mut self, gid: usize) {
        if let Some(shard) = self.shards.get_mut(gid) {
            shard.dirty = GroupDirty::default();
        }
    }

    pub fn clear_all_dirty(&mut self) {
        for shard in &mut self.shards {
            shard.dirty = GroupDirty::default();
        }
    }

    /// Mark every group dirty so the next checkpoint rewrites the full
    /// direction. Used after topology-wide rebuilds such as vertex remapping,
    /// where clean-group skipping would otherwise persist stale group files.
    pub fn mark_all_dirty(&mut self) {
        for shard in &mut self.shards {
            shard.dirty = GroupDirty {
                inserted: true,
                deleted: true,
                column_updated: true,
            };
        }
    }

    /// Resize the group space for loading: grows with fresh variants,
    /// shrinks by dropping trailing groups.
    pub fn resize_groups(&mut self, count: usize) -> StorageResult<()> {
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
        while self.shards.len() < count {
            self.shards.push(Shard {
                variant: self.fresh_variant()?,
                dirty: GroupDirty::default(),
                reclaim_hint: false,
            });
        }
        self.shards.truncate(count);
        self.clear_all_dirty();
        Ok(())
    }

    /// Load one group payload without marking it dirty. The reclaim hint
    /// stays set: loaded groups may hold tombstones the next reclaim pass
    /// must inspect once.
    pub fn load_group(&mut self, gid: usize, data: &[u8]) -> StorageResult<()> {
        let shard = self.shards.get_mut(gid).ok_or_else(|| {
            StorageError::deserialize_error(format!("group {} out of range on load", gid))
        })?;
        shard.variant.load(data)?;
        shard.dirty = GroupDirty::default();
        shard.reclaim_hint = true;
        Ok(())
    }

    /// Drop trailing groups holding no physical entries. Tombstone-only tail
    /// groups are retained so snapshot history before the cutoff survives.
    /// Non-empty strategies keep at least one group so an empty table stays
    /// addressable.
    pub fn truncate_trailing_empty_groups(&mut self) {
        let mut keep = 0usize;
        for (gid, shard) in self.shards.iter().enumerate() {
            if shard.variant.iter_all().next().is_some() {
                keep = gid + 1;
            }
        }
        if self.strategy != EdgeStrategy::None {
            keep = keep.max(1);
        }
        self.shards.truncate(keep);
    }

    /// Whether the primary row of one vertex holds `edge_id`.
    pub fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards[gid].variant.primary_contains(local, edge_id)
    }

    /// Visit every physically stored entry of one vertex without allocating.
    pub fn visit_physical<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let Some((gid, local)) = self.route(src_vid) else {
            return;
        };
        self.shards[gid].variant.visit_physical(local, f);
    }

    /// Set the reclaim hint for the group owning `vid`.
    ///
    /// Used by remap to preserve the hint explicitly instead of relying on
    /// delete side effects.
    pub fn mark_reclaim_hint_for(&mut self, vid: u32) {
        let gid = group_id_for(vid, self.group_bits);
        if let Some(shard) = self.shards.get_mut(gid) {
            shard.reclaim_hint = true;
        }
    }

    /// Whether a group may hold tombstones worth a reclaim scan.
    pub fn group_needs_reclaim_scan(&self, gid: usize) -> bool {
        self.shards
            .get(gid)
            .map(|shard| shard.reclaim_hint)
            .unwrap_or(false)
    }

    /// Clear the reclaim hint after a pass visited the whole group.
    pub fn clear_reclaim_hint(&mut self, gid: usize) {
        if let Some(shard) = self.shards.get_mut(gid) {
            shard.reclaim_hint = false;
        }
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
        let Some(shard) = self.shards.get_mut(gid) else {
            return 0;
        };
        let removed =
            shard
                .variant
                .compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed);
        if removed > 0 {
            shard.dirty.deleted = true;
        }
        removed
    }

    /// Observability view of one group.
    pub fn group_stats(&self, gid: usize) -> Option<NodeGroupStats> {
        let shard = self.shards.get(gid)?;
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
        (0..self.shards.len())
            .filter_map(|gid| self.group_stats(gid))
            .collect()
    }

    /// Whether a group holds uncheckpointed writes.
    pub fn needs_checkpoint(&self, gid: usize) -> bool {
        self.group_dirty(gid).is_dirty()
    }

    /// Clear all edges, keeping the group space. Marks surviving groups
    /// dirty so the next checkpoint persists the cleared state.
    pub fn clear(&mut self) {
        for shard in &mut self.shards {
            shard.variant.clear();
            shard.dirty = GroupDirty {
                inserted: false,
                deleted: true,
                column_updated: false,
            };
            shard.reclaim_hint = false;
        }
    }

    /// Average bytes per edge based on actual memory usage.
    /// Empty tables report the fallback without log noise; only genuinely
    /// degenerate non-empty measurements emit a debug line.
    pub fn bytes_per_edge(&self) -> usize {
        let edges = self.edge_count().max(1) as usize;
        let bytes = self.used_memory_size();
        let bpe = bytes / edges;
        if bpe == 0 {
            let fallback = match self.strategy {
                EdgeStrategy::None => 0,
                _ => std::mem::size_of::<Nbr>(),
            };
            if fallback > 0 {
                log::debug!(
                    "bytes_per_edge: computed bpe=0 ({} bytes / {} edges), using fallback {}",
                    bytes,
                    self.edge_count(),
                    fallback
                );
            }
            fallback
        } else {
            bpe
        }
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
        for shard in &self.shards {
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
    pub fn fragmentation_ratio(&self) -> f32 {
        if self.strategy != EdgeStrategy::Multiple {
            return 0.0;
        }
        let mut total_capacity = 0usize;
        let mut wasted = 0usize;
        for shard in &self.shards {
            if let Some(stats) = shard.variant.fragmentation_stats() {
                total_capacity += stats.total_capacity;
                wasted += stats.wasted_capacity;
            }
        }
        if total_capacity == 0 {
            0.0
        } else {
            wasted as f32 / total_capacity as f32
        }
    }

    /// Estimate wasted bytes due to fragmentation, summed across groups.
    pub fn wasted_bytes_estimate(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.variant.wasted_bytes_estimate())
            .sum()
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
        for shard in &mut self.shards {
            let before = removed;
            removed +=
                shard
                    .variant
                    .compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed);
            if removed > before {
                shard.dirty.deleted = true;
            }
        }
        removed
    }

    /// Iterate edges of a vertex without allocating (Multiple only).
    /// Test-only row-stamp filtered iterator; production scans go through the version authority.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> Option<VertexEdgesIter<'_>> {
        let (gid, local) = self.route(src_vid)?;
        self.shards[gid].variant.iter_edges_of(local, ts)
    }

    /// Iterate all live edges across groups in group order.
    pub fn iter(&self, ts: Timestamp) -> ShardCsrIterator<'_> {
        ShardCsrIterator::new(&self.shards, self.group_bits, ts, false)
    }

    /// Iterate every physically present entry across groups, including
    /// tombstoned ones, in group order.
    pub fn iter_all(&self) -> ShardCsrIterator<'_> {
        ShardCsrIterator::new(&self.shards, self.group_bits, 0, true)
    }

    /// Approximate memory usage in bytes, summed across groups.
    pub fn used_memory_size(&self) -> usize {
        if self.strategy == EdgeStrategy::None {
            return std::mem::size_of::<Self>();
        }
        self.shards
            .iter()
            .map(|shard| shard.variant.used_memory_size())
            .sum::<usize>()
            + std::mem::size_of::<Self>()
    }
}

impl CsrBase for CsrShardSet {
    fn vertex_capacity(&self) -> usize {
        self.shards.len() * self.group_size()
    }

    fn edge_count(&self) -> u64 {
        self.shards
            .iter()
            .map(|shard| shard.variant.edge_count())
            .sum()
    }

    fn dump(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&SHARD_SET_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.group_bits.to_le_bytes());
        out.extend_from_slice(&(self.shards.len() as u32).to_le_bytes());
        for shard in &self.shards {
            let payload = shard.variant.dump();
            out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            out.extend_from_slice(&payload);
        }
        out
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        fn take_bytes<'a>(
            data: &'a [u8],
            cursor: &mut usize,
            len: usize,
        ) -> StorageResult<&'a [u8]> {
            if data.len() - *cursor < len {
                return Err(StorageError::deserialize_error("shard set data too short"));
            }
            let slice = &data[*cursor..*cursor + len];
            *cursor += len;
            Ok(slice)
        }
        let mut cursor = 0usize;
        let version_bytes: [u8; 4] = take_bytes(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("shard set version too short"))?;
        let version = u32::from_le_bytes(version_bytes);
        if version != SHARD_SET_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported shard set version: {}",
                version
            )));
        }
        let group_bits_bytes: [u8; 4] = take_bytes(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("shard set group bits too short"))?;
        let group_bits = u32::from_le_bytes(group_bits_bytes);
        if group_bits != self.group_bits {
            return Err(StorageError::deserialize_error(format!(
                "shard set group bits mismatch: file={}, expected={}",
                group_bits, self.group_bits
            )));
        }
        let count_bytes: [u8; 4] = take_bytes(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("shard set count too short"))?;
        let count = u32::from_le_bytes(count_bytes) as usize;
        self.resize_groups(count)?;
        for gid in 0..count {
            let len_bytes: [u8; 8] = take_bytes(data, &mut cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("shard payload length too short"))?;
            let len = u64::from_le_bytes(len_bytes) as usize;
            let payload = take_bytes(data, &mut cursor, len)?.to_vec();
            self.load_group(gid, &payload)?;
        }
        if cursor != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in shard set".to_string(),
            ));
        }
        self.clear_all_dirty();
        Ok(())
    }
}

fn no_edges_error() -> StorageError {
    StorageError::invalid_operation("no edges stored for this edge type".to_string())
}

impl MutableCsrTrait for CsrShardSet {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let gid = self.ensure_group_for(src_vid)?;
        let local = local_vid(src_vid, self.group_bits);
        self.shards[gid]
            .variant
            .insert_edge(local, dst, edge_id, ts)?;
        self.shards[gid].dirty.inserted = true;
        Ok(())
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Err(no_edges_error());
        };
        let deleted = self.shards[gid].variant.delete_edge(local, edge_id, ts)?;
        if deleted {
            self.shards[gid].dirty.deleted = true;
            self.shards[gid].reclaim_hint = true;
        }
        Ok(deleted)
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let deleted = self.shards[gid].variant.delete_edge_by_dst(local, dst, ts);
        if deleted {
            self.shards[gid].dirty.deleted = true;
            self.shards[gid].reclaim_hint = true;
        }
        deleted
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Ok(false);
        };
        let deleted = self.shards[gid]
            .variant
            .delete_edge_by_offset(local, offset, ts)?;
        if deleted {
            self.shards[gid].dirty.deleted = true;
            self.shards[gid].reclaim_hint = true;
        }
        Ok(deleted)
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        let (gid, local) = self.route(src_vid)?;
        self.shards[gid].variant.nbr_at_offset(local, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (gid, local) = self.route(src_vid)?;
        self.shards[gid].variant.get_edge_physical(local, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Vec::new();
        };
        self.shards[gid].variant.physical_edges_of(local)
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        let Some((gid, local)) = self.route(vid) else {
            return false;
        };
        self.shards[gid].variant.has_physical_entries(local)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards[gid].variant.primary_contains(local, edge_id)
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let reverted = self.shards[gid]
            .variant
            .revert_delete_by_offset(local, offset, ts);
        if reverted {
            self.shards[gid].dirty.deleted = true;
        }
        reverted
    }

    fn remove_edge(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let removed = self.shards[gid].variant.remove_edge(local, edge_id);
        if removed {
            self.shards[gid].dirty.deleted = true;
        }
        removed
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let reverted = self.shards[gid]
            .variant
            .revert_delete_by_edge_id(local, edge_id, ts);
        if reverted {
            self.shards[gid].dirty.deleted = true;
        }
        reverted
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (gid, local) = self.route(src_vid)?;
        self.shards[gid].variant.get_edge(local, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Vec::new();
        };
        self.shards[gid].variant.edges_of(local, ts)
    }

    fn compact_with_ts(&mut self, ts: Timestamp, reserve_ratio: f32) -> usize {
        let mut removed = 0usize;
        for shard in &mut self.shards {
            let n = shard.variant.compact_with_ts(ts, reserve_ratio);
            if n > 0 {
                shard.dirty.deleted = true;
            }
            removed += n;
        }
        removed
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let Some((gid, local)) = self.route(vid) else {
            return 0;
        };
        let removed =
            self.shards[gid]
                .variant
                .compact_vertex_with_reporting(local, cutoff, on_edge_removed);
        if removed > 0 {
            self.shards[gid].dirty.deleted = true;
        }
        removed
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        let Some((gid, local)) = self.route(vid) else {
            return 0;
        };
        self.shards[gid].variant.reclaimable_count(local, cutoff)
    }

    fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        let Some((gid, local)) = self.route(vid) else {
            return false;
        };
        self.shards[gid].variant.vertex_needs_compact(local, cutoff)
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let Some((gid, local)) = self.route(vid) else {
            return (0, 0, 0);
        };
        self.shards[gid].variant.vertex_census(local)
    }

    fn used_memory_size(&self) -> usize {
        CsrShardSet::used_memory_size(self)
    }
}

/// Iterator chaining every group in group order, translating local rows to
/// global vertex ids.
pub struct ShardCsrIterator<'a> {
    shards: &'a [Shard],
    group_bits: u32,
    group_idx: usize,
    base: u32,
    inner: CsrIterator<'a>,
    include_deleted: bool,
    ts: Timestamp,
}

impl<'a> ShardCsrIterator<'a> {
    fn new(shards: &'a [Shard], group_bits: u32, ts: Timestamp, include_deleted: bool) -> Self {
        Self {
            shards,
            group_bits,
            group_idx: 0,
            base: 0,
            inner: CsrIterator::None,
            include_deleted,
            ts,
        }
    }

    fn advance_group(&mut self) -> bool {
        let shards: &'a [Shard] = self.shards;
        if self.group_idx >= shards.len() {
            return false;
        }
        let gid = self.group_idx;
        self.group_idx += 1;
        self.base = group_base(gid, self.group_bits);
        let variant = &shards[gid].variant;
        self.inner = if self.include_deleted {
            variant.iter_all()
        } else {
            variant.iter(self.ts)
        };
        true
    }
}

impl<'a> Iterator for ShardCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((local, nbr)) = self.inner.next() {
                let global = local.as_int64().unwrap_or(0) + self.base as i64;
                return Some((VertexId::from_int64(global), nbr));
            }
            if !self.advance_group() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multi_set() -> CsrShardSet {
        CsrShardSet::new(EdgeStrategy::Multiple, DEFAULT_NODE_GROUP_BITS, 4096).unwrap()
    }

    fn endpoint(dst: u32, rank: i64) -> VertexId {
        VertexId::edge_endpoint_key(dst, rank)
    }

    #[test]
    fn group_mapping_splits_at_group_boundary() {
        assert_eq!(group_id_for(0, 12), 0);
        assert_eq!(group_id_for(4095, 12), 0);
        assert_eq!(group_id_for(4096, 12), 1);
        assert_eq!(local_vid(4096, 12), 0);
        assert_eq!(local_vid(5000, 12), 904);
        assert_eq!(group_base(1, 12), 4096);
    }

    #[test]
    fn invalid_group_bits_rejected() {
        assert!(CsrShardSet::new(EdgeStrategy::Multiple, 0, 4096).is_err());
        assert!(CsrShardSet::new(EdgeStrategy::Multiple, 21, 4096).is_err());
    }

    #[test]
    fn cross_group_insert_and_read() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.insert_edge(5000, endpoint(6000, 1), EdgeId(1), 100)
            .unwrap();
        assert_eq!(set.group_count(), 2);
        assert_eq!(set.edge_count(), 2);
        assert!(set.get_edge(0, endpoint(1, 0), 200).is_some());
        assert!(set.get_edge(5000, endpoint(6000, 1), 200).is_some());
        assert_eq!(set.edges_of(0, 200).len(), 1);
        assert_eq!(set.edges_of(5000, 200).len(), 1);
        assert!(set.edges_of(1, 200).is_empty());
    }

    #[test]
    fn reads_never_create_groups() {
        let set = multi_set();
        assert_eq!(set.group_count(), 1);
        assert!(set.get_edge(9000, endpoint(1, 0), 200).is_none());
        assert!(set.edges_of(9000, 200).is_empty());
        assert_eq!(set.group_count(), 1);
    }

    #[test]
    fn dirty_tracking_per_group() {
        let mut set = multi_set();
        assert!(set.dirty_group_ids().is_empty());
        set.insert_edge(5000, endpoint(1, 0), EdgeId(0), 100)
            .unwrap();
        assert_eq!(set.dirty_group_ids(), vec![1]);
        assert!(!set.needs_checkpoint(0));
        assert!(set.needs_checkpoint(1));
        set.clear_group_dirty(1);
        assert!(set.dirty_group_ids().is_empty());
    }

    #[test]
    fn delete_marks_group_dirty() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.clear_all_dirty();
        assert!(set.delete_edge(0, EdgeId(0), 150).unwrap());
        assert_eq!(set.dirty_group_ids(), vec![0]);
    }

    #[test]
    fn sharded_iter_translates_to_global_rows() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.insert_edge(5000, endpoint(2, 0), EdgeId(1), 100)
            .unwrap();
        let rows: Vec<(i64, EdgeId)> = set
            .iter(200)
            .map(|(src, nbr)| (src.as_int64().unwrap_or(-1), nbr.edge_id))
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.contains(&(0, EdgeId(0))));
        assert!(rows.contains(&(5000, EdgeId(1))));
    }

    #[test]
    fn container_dump_load_roundtrip() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.insert_edge(5000, endpoint(2, 0), EdgeId(1), 100)
            .unwrap();
        let payload = set.dump();
        let mut loaded = multi_set();
        loaded.load(&payload).unwrap();
        assert_eq!(loaded.group_count(), 2);
        assert_eq!(loaded.edge_count(), 2);
        assert!(loaded.get_edge(5000, endpoint(2, 0), 200).is_some());
        assert!(loaded.dirty_group_ids().is_empty());
    }

    #[test]
    fn manifest_rejects_bad_version_and_trailing() {
        let manifest = TableShardManifest {
            group_bits: 12,
            out_groups: 2,
            in_groups: 1,
        };
        let payload = manifest.encode();
        assert_eq!(TableShardManifest::decode(&payload).unwrap(), manifest);
        let mut bad = payload.clone();
        bad[0] = 99;
        assert!(TableShardManifest::decode(&bad).is_err());
        let mut trailing = payload.clone();
        trailing.push(0);
        assert!(TableShardManifest::decode(&trailing).is_err());
        assert!(TableShardManifest::decode(&payload[..8]).is_err());
    }

    #[test]
    fn none_strategy_holds_no_groups() {
        let mut set = CsrShardSet::new(EdgeStrategy::None, 12, 4096).unwrap();
        assert_eq!(set.group_count(), 0);
        assert_eq!(set.vertex_capacity(), 0);
        assert!(set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).is_err());
        assert!(set.delete_edge(0, EdgeId(0), 100).is_err());
        assert!(!set.delete_edge_by_dst(0, endpoint(1, 0), 100));
    }

    #[test]
    fn truncate_drops_trailing_empty_groups() {
        let mut set = multi_set();
        set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
            .unwrap();
        assert_eq!(set.group_count(), 3);
        assert!(set.remove_edge(9000, EdgeId(0)));
        set.truncate_trailing_empty_groups();
        assert_eq!(set.group_count(), 1);
    }

    #[test]
    fn truncate_keeps_tombstone_only_tail_groups() {
        let mut set = multi_set();
        set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
            .unwrap();
        assert_eq!(set.group_count(), 3);
        assert!(set.delete_edge(9000, EdgeId(0), 150).unwrap());
        assert_eq!(set.edge_count(), 0);
        set.truncate_trailing_empty_groups();
        assert_eq!(set.group_count(), 3);
    }

    #[test]
    fn offset_delete_rejects_out_of_degree() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        assert!(!set.delete_edge_by_offset(0, 5, 150).unwrap());
        assert!(set.delete_edge_by_offset(0, 0, 150).unwrap());
    }

    #[test]
    fn physical_reads_ignore_timestamps() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        assert!(set.delete_edge(0, EdgeId(0), 150).unwrap());
        assert!(set.get_edge_physical(0, endpoint(1, 0)).is_some());
        assert_eq!(set.physical_edges_of(0).len(), 1);
        assert!(set.has_physical_entries(0));
    }

    #[test]
    fn column_dirt_does_not_force_topology_checkpoint() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.clear_all_dirty();
        set.mark_column_updated_for(0);
        assert!(set.column_dirty_group_ids() == vec![0]);
        assert!(set.dirty_group_ids().is_empty());
        assert!(!set.needs_checkpoint(0));
        assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::AppendOnly);
    }

    #[test]
    fn checkpoint_kind_turns_rebalance_on_delete_dirt() {
        let mut set = multi_set();
        set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
        assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::AppendOnly);
        assert!(set.delete_edge(0, EdgeId(0), 150).unwrap());
        assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::Rebalance);
        set.clear_all_dirty();
        assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::AppendOnly);
        set.mark_all_dirty();
        assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::Rebalance);
    }

}

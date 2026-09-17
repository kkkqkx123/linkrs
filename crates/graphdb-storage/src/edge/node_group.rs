//! Node-group sharding for edge topology.
//!
//! Each edge direction is partitioned by bound-endpoint interval:
//! `group = vid >> group_bits`, `local = vid & (group_size - 1)`.
//! Every group owns one `CsrVariant` holding only the rows of its interval,
//! plus insert/delete/column-update dirt markers driving incremental
//! checkpoints. Neighbor keys keep global endpoint values; only row
//! addressing is local.
//!
//! Inside a group, fixed row windows form leaf regions carrying their own
//! three-kind dirt. Group dirt is the OR over its regions; reclaim passes
//! decide per region so unchanged regions are skipped, while checkpoints skip
//! clean groups at group granularity. Whole-group and whole-table ratios stay
//! observability only.
//!
//! Groups are sparse: only existing groups are materialized, the manifest
//! records existing group ids rather than a contiguous count, missing groups
//! read as empty and writes create them on demand, and no files are written
//! for missing groups. Endpoints are expected dense; sparse large endpoints
//! must be densified offline via vertex remapping before bulk load, otherwise
//! the group span stays wide while only existing groups consume memory and
//! files. The address width is locked at table creation; the only adjustment
//! outlet is the offline reshard tool, there is no online width-change branch.
//!
//! Timestamps and properties are sharded on disk by owner group and fall with
//! the same dirt as their group: small writes rewrite only dirty groups'
//! timestamp and property shards, never the whole table. The in-memory
//! visibility authority stays global by edge id; sharding changes only the
//! flush unit.
//!
//! Each group additionally holds a committed append log: the write-through
//! delta (inserts plus tombstone markers) since the last base rewrite.
//! Insert-only groups checkpoint by persisting the append log alone; groups
//! with delete dirt rewrite the base and discard the log. Append row indexes
//! are dropped after the flush that persists them.

use graphdb_core::types::{EdgeId, EdgeStrategy, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};
use std::collections::BTreeMap;

use super::csr_variant::CsrIterator;
use super::mutable_csr::VertexEdgesIter;
use super::{CsrBase, CsrVariant, FragmentationStats, MutableCsrTrait, Nbr};

/// Default address bits per node group: 12 bits cover 4096 rows.
pub const DEFAULT_NODE_GROUP_BITS: u32 = 12;
/// Rows per leaf region inside a group. A default group holds 16 regions;
/// region dirt and density merges operate at this granularity.
pub const LEAF_REGION_ROWS: usize = 256;
/// Region density at or above which a dirty region merges as a whole.
/// Below it only rows holding reclaimable entries are visited.
pub const REGION_MERGE_MIN_DENSITY: f32 = 0.4;
/// Group density at or above which a multi-region dirty span merges at
/// group scope. Below it merges stay region-scoped.
pub const GROUP_MERGE_MIN_DENSITY: f32 = 0.65;
/// Container serialization version for a sharded direction. Version 4
/// carries sparse group ids plus per-region dirt and the append-log sidecar
/// contract over version 3 topology columns; version 3 and older payloads
/// are rejected, never converted.
pub const SHARD_SET_FORMAT_VERSION: u32 = 4;
/// Manifest version for the per-table group layout file. Version 5 records
/// existing group ids rather than contiguous counts and admits per-group
/// timestamp, property and segment-statistics shards; older manifests are
/// rejected, never converted.
pub const GROUP_MANIFEST_VERSION: u32 = 5;
/// Wire version of one append-log sidecar payload. Version 2 carries only the
/// address width so group-set growth never invalidates clean groups' sidecars;
/// version 1 payloads are rejected, never converted.
pub(crate) const APPEND_LOG_FORMAT_VERSION: u32 = 2;

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
/// is set; `column_updated` traces property-only writes to their owning
/// groups for observability without forcing a topology rewrite. The column
/// trace is a sampled caliber only: vids outside the current group space
/// leave no trace, and correctness never depends on it. The table-level
/// property dirt alone guarantees the final property flush.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupDirty {
    pub inserted: bool,
    pub deleted: bool,
    pub column_updated: bool,
}

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

/// One committed append-log insert: the row plus the stored neighbor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppendInsert {
    pub local: u32,
    pub nbr: Nbr,
}

/// One committed append-log delete marker: the row plus the tombstone stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppendDelete {
    pub local: u32,
    pub edge_id: EdgeId,
    pub delete_ts: Timestamp,
}

/// Write-through delta of one group since its last base rewrite.
///
/// Every committed topology write lands in the base variant for reads and is
/// also recorded here. Insert-only groups checkpoint by persisting this log
/// alone; the first delete dirt in the group forces a base rewrite that
/// discards the log. The log is a row index only: it is dropped after the
/// flush that persists it and rebuilt from later writes.
#[derive(Debug, Clone, Default)]
pub(crate) struct ShardAppendLog {
    pub inserts: Vec<AppendInsert>,
    pub deletes: Vec<AppendDelete>,
}

impl ShardAppendLog {
    pub fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.deletes.is_empty()
    }

    pub fn clear(&mut self) {
        self.inserts.clear();
        self.deletes.clear();
    }

    pub fn op_count(&self) -> usize {
        self.inserts.len() + self.deletes.len()
    }
}

/// Encode one append-op sequence for a sidecar. Carries only the address
/// width so group-set growth never invalidates clean groups' sidecars; a
/// width mismatch still fails closed on load.
pub(crate) fn encode_append_ops(
    manifest: &TableShardManifest,
    inserts: &[AppendInsert],
    deletes: &[AppendDelete],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&APPEND_LOG_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&manifest.group_bits.to_le_bytes());
    out.extend_from_slice(&(inserts.len() as u64).to_le_bytes());
    for insert in inserts {
        out.extend_from_slice(&insert.local.to_le_bytes());
        out.extend_from_slice(&insert.nbr.endpoint.to_le_bytes());
        out.extend_from_slice(&insert.nbr.rank.to_le_bytes());
        out.extend_from_slice(&insert.nbr.edge_id.0.to_le_bytes());
        out.extend_from_slice(&insert.nbr.create_ts.to_le_bytes());
        out.extend_from_slice(&insert.nbr.delete_ts.to_le_bytes());
    }
    out.extend_from_slice(&(deletes.len() as u64).to_le_bytes());
    for delete in deletes {
        out.extend_from_slice(&delete.local.to_le_bytes());
        out.extend_from_slice(&delete.edge_id.0.to_le_bytes());
        out.extend_from_slice(&delete.delete_ts.to_le_bytes());
    }
    out
}

/// Decode one append-op sequence. Fails closed on version, address-width,
/// section-size or trailing-byte mismatches.
pub(crate) fn decode_append_ops(
    data: &[u8],
    manifest: &TableShardManifest,
) -> StorageResult<(Vec<AppendInsert>, Vec<AppendDelete>)> {
    let mut cursor = 0usize;
    let take = |data: &[u8], cursor: &mut usize, len: usize| -> StorageResult<Vec<u8>> {
        if data.len() - *cursor < len {
            return Err(StorageError::deserialize_error(
                "append log payload too short",
            ));
        }
        let slice = data[*cursor..*cursor + len].to_vec();
        *cursor += len;
        Ok(slice)
    };
    let version = u32::from_le_bytes(
        take(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log version too short"))?,
    );
    if version != APPEND_LOG_FORMAT_VERSION {
        return Err(StorageError::deserialize_error(format!(
            "unsupported append log version: {}",
            version
        )));
    }
    let carried_bits = u32::from_le_bytes(
        take(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log width too short"))?,
    );
    if carried_bits != manifest.group_bits {
        return Err(StorageError::deserialize_error(format!(
            "append log width mismatch: log carries {}, table holds {}",
            carried_bits, manifest.group_bits
        )));
    }
    let insert_count = u64::from_le_bytes(
        take(data, &mut cursor, 8)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log insert count too short"))?,
    ) as usize;
    let mut inserts = Vec::with_capacity(insert_count);
    for _ in 0..insert_count {
        let local = u32::from_le_bytes(
            take(data, &mut cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log insert row too short"))?,
        );
        let endpoint = u32::from_le_bytes(
            take(data, &mut cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log endpoint too short"))?,
        );
        let rank = u64::from_le_bytes(
            take(data, &mut cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log rank too short"))?,
        ) as i64;
        let edge_id = EdgeId(u64::from_le_bytes(
            take(data, &mut cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log edge id too short"))?,
        ));
        let create_ts =
            u64::from_le_bytes(take(data, &mut cursor, 8)?.try_into().map_err(|_| {
                StorageError::deserialize_error("append log create stamp too short")
            })?);
        let delete_ts =
            u64::from_le_bytes(take(data, &mut cursor, 8)?.try_into().map_err(|_| {
                StorageError::deserialize_error("append log delete stamp too short")
            })?);
        let mut nbr = Nbr::with_timestamps(endpoint, rank, edge_id, delete_ts);
        nbr.create_ts = create_ts;
        inserts.push(AppendInsert { local, nbr });
    }
    let delete_count = u64::from_le_bytes(
        take(data, &mut cursor, 8)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log delete count too short"))?,
    ) as usize;
    let mut deletes = Vec::with_capacity(delete_count);
    for _ in 0..delete_count {
        let local = u32::from_le_bytes(
            take(data, &mut cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log delete row too short"))?,
        );
        let edge_id = EdgeId(u64::from_le_bytes(
            take(data, &mut cursor, 8)?.try_into().map_err(|_| {
                StorageError::deserialize_error("append log delete edge id too short")
            })?,
        ));
        let delete_ts =
            u64::from_le_bytes(take(data, &mut cursor, 8)?.try_into().map_err(|_| {
                StorageError::deserialize_error("append log delete stamp too short")
            })?);
        deletes.push(AppendDelete {
            local,
            edge_id,
            delete_ts,
        });
    }
    if cursor != data.len() {
        return Err(StorageError::deserialize_error(
            "unexpected trailing data in append log".to_string(),
        ));
    }
    Ok((inserts, deletes))
}

impl GroupDirty {
    pub fn is_dirty(self) -> bool {
        self.inserted || self.deleted
    }

    /// Sampled column-dirt caliber, not a correctness signal. Topology
    /// checkpoints never consult it; property flushing consults the
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
///
/// Version 4 records existing group ids rather than contiguous counts:
/// sparse endpoints materialize only groups holding rows, missing groups
/// read as empty and never produce files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableShardManifest {
    pub group_bits: u32,
    pub out_groups: Vec<u32>,
    pub in_groups: Vec<u32>,
}

impl TableShardManifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out_groups = self.out_groups.clone();
        out_groups.sort_unstable();
        out_groups.dedup();
        let mut in_groups = self.in_groups.clone();
        in_groups.sort_unstable();
        in_groups.dedup();
        let mut out = Vec::with_capacity(16 + (out_groups.len() + in_groups.len()) * 4);
        out.extend_from_slice(&GROUP_MANIFEST_VERSION.to_le_bytes());
        out.extend_from_slice(&self.group_bits.to_le_bytes());
        out.extend_from_slice(&(out_groups.len() as u32).to_le_bytes());
        for gid in &out_groups {
            out.extend_from_slice(&gid.to_le_bytes());
        }
        out.extend_from_slice(&(in_groups.len() as u32).to_le_bytes());
        for gid in &in_groups {
            out.extend_from_slice(&gid.to_le_bytes());
        }
        out
    }

    pub fn decode(data: &[u8]) -> StorageResult<Self> {
        let bad_slice = || StorageError::deserialize_error("group manifest slice too short");
        if data.len() < 12 {
            return Err(StorageError::deserialize_error(format!(
                "group manifest too short: got {}",
                data.len()
            )));
        }
        let version = u32::from_le_bytes(data[0..4].try_into().map_err(|_| bad_slice())?);
        if version != GROUP_MANIFEST_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported group manifest version: {}",
                version
            )));
        }
        let group_bits = u32::from_le_bytes(data[4..8].try_into().map_err(|_| bad_slice())?);
        validate_group_bits(group_bits)?;
        let mut cursor = 8usize;
        let take_u32 = |data: &[u8], cursor: &mut usize| -> StorageResult<u32> {
            if data.len() - *cursor < 4 {
                return Err(StorageError::deserialize_error(
                    "group manifest slice too short",
                ));
            }
            let value = u32::from_le_bytes(
                data[*cursor..*cursor + 4]
                    .try_into()
                    .map_err(|_| bad_slice())?,
            );
            *cursor += 4;
            Ok(value)
        };
        let out_len = take_u32(data, &mut cursor)? as usize;
        if data.len() - cursor < out_len * 4 + 4 {
            return Err(StorageError::deserialize_error(format!(
                "group manifest too short for {} out groups",
                out_len
            )));
        }
        let mut out_groups = Vec::with_capacity(out_len);
        for _ in 0..out_len {
            out_groups.push(take_u32(data, &mut cursor)?);
        }
        let in_len = take_u32(data, &mut cursor)? as usize;
        if data.len() - cursor != in_len * 4 {
            return Err(StorageError::deserialize_error(format!(
                "group manifest trailing bytes: expected {} in groups, {} bytes remain",
                in_len,
                data.len() - cursor
            )));
        }
        let mut in_groups = Vec::with_capacity(in_len);
        for _ in 0..in_len {
            in_groups.push(take_u32(data, &mut cursor)?);
        }
        out_groups.sort_unstable();
        out_groups.dedup();
        in_groups.sort_unstable();
        in_groups.dedup();
        Ok(Self {
            group_bits,
            out_groups,
            in_groups,
        })
    }
}

/// Leaf regions covering one group of `group_size` rows.
pub fn regions_per_group(group_size: usize) -> usize {
    group_size.div_ceil(LEAF_REGION_ROWS).max(1)
}

/// Leaf region owning a group-local row.
pub fn region_id_for_local(local: u32) -> usize {
    local as usize / LEAF_REGION_ROWS
}

/// Group-local `[start, end)` row window of one leaf region.
pub fn region_local_range(region: usize, group_size: usize) -> (u32, u32) {
    let start = (region * LEAF_REGION_ROWS).min(group_size) as u32;
    let end = ((region + 1) * LEAF_REGION_ROWS).min(group_size) as u32;
    (start, end)
}

#[derive(Debug, Clone)]
struct Shard {
    variant: CsrVariant,
    dirty: GroupDirty,
    regions: Vec<RegionDirty>,
    append: ShardAppendLog,
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
    shards: BTreeMap<usize, Shard>,
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

    fn route(&self, vid: u32) -> Option<(usize, u32)> {
        let gid = group_id_for(vid, self.group_bits);
        self.shards
            .get(&gid)
            .map(|_| (gid, local_vid(vid, self.group_bits)))
    }

    /// Borrow one persisted group for checkpoint writes.
    pub fn group_variant(&self, gid: usize) -> Option<&CsrVariant> {
        self.shards.get(&gid).map(|shard| &shard.variant)
    }

    /// Mutably borrow one group for checkpoint loads.
    pub fn group_variant_mut(&mut self, gid: usize) -> Option<&mut CsrVariant> {
        self.shards.get_mut(&gid).map(|shard| &mut shard.variant)
    }

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

    /// Ids of groups holding sampled property-only write traces.
    ///
    /// Sampled observability caliber, never a correctness basis: vids outside
    /// the current group space leave no trace, and the property file flush
    /// decision consults the table-level flag alone.
    pub fn sampled_column_dirty_group_ids(&self) -> Vec<usize> {
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
    /// Sampled observability caliber: vids outside the current group space
    /// leave no group trace and rely on the table-level property dirt, which
    /// alone guarantees the final property flush. Never consulted for
    /// correctness, only exposed via `sampled_column_dirty_group_ids`.
    pub fn mark_column_updated_for(&mut self, vid: u32) {
        let gid = group_id_for(vid, self.group_bits);
        let rid = region_id_for_local(local_vid(vid, self.group_bits));
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

    /// Clear sampled column-only traces after a flush. Insert and delete
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

    /// Resize the group space for construction only: grows with fresh
    /// variants, shrinks by dropping trailing groups. Construction and load
    /// paths only; normal writes grow through the routed insert path and
    /// never reset whole groups.
    pub(crate) fn resize_groups(&mut self, count: usize) -> StorageResult<()> {
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

    /// Materialize exactly the listed groups for loading a sparse manifest.
    /// Missing groups stay absent: they read as empty and never produce
    /// files. Unlisted materialized groups are dropped. Construction and
    /// load paths only; callers must go through the normal group migration
    /// path instead of resetting a non-empty table.
    pub(crate) fn set_groups(&mut self, ids: &[u32]) -> StorageResult<()> {
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

    /// Whether the primary row of one vertex holds `edge_id`.
    pub fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.primary_contains(local, edge_id))
    }

    /// Owner group of one global vertex id without creating groups.
    ///
    /// Shared row-location entry point for point lookups, adjacency batches
    /// and full scans: every read path resolves rows through this routing
    /// instead of duplicating the group arithmetic.
    pub fn group_of(&self, vid: u32) -> Option<usize> {
        self.route(vid).map(|(gid, _)| gid)
    }

    /// Visit every physically stored entry of one vertex without allocating.
    pub fn visit_physical<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let Some((gid, local)) = self.route(src_vid) else {
            return;
        };
        if let Some(shard) = self.shards.get(&gid) {
            shard.variant.visit_physical(local, f);
        }
    }

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

    fn mark_region_insert(&mut self, gid: usize, local: u32) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.dirty.inserted = true;
            let rid = region_id_for_local(local);
            if let Some(region) = shard.regions.get_mut(rid) {
                region.inserted = true;
            }
        }
    }

    fn mark_region_delete(&mut self, gid: usize, local: u32) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.dirty.deleted = true;
            let rid = region_id_for_local(local);
            if let Some(region) = shard.regions.get_mut(rid) {
                region.deleted = true;
            }
        }
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
    /// reclaimable and carries no delete dirt.
    pub fn select_merge_scope(
        &self,
        gid: usize,
        region: usize,
        cutoff: Timestamp,
    ) -> Option<RegionMergeScope> {
        if self.region_reclaimable_count(gid, region, cutoff) == 0
            && !self.region_needs_rebalance(gid, region)
        {
            return None;
        }
        let region_density = self.region_density(gid, region);
        if region_density < REGION_MERGE_MIN_DENSITY {
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
        if self.dirty_region_ids(gid).len() > 1 && group_live >= GROUP_MERGE_MIN_DENSITY {
            return Some(RegionMergeScope::Group);
        }
        Some(RegionMergeScope::Region)
    }

    /// Compact one leaf region in place, reporting removals. Only rows in
    /// the region window are visited; other regions never move. Marks the
    /// region deleted when entries were dropped so the next checkpoint
    /// merges the rebuilt rows. Rows holding overflow are rebalanced into
    /// primary gaps on the same pass.
    pub fn compact_region_with_reporting(
        &mut self,
        gid: usize,
        region: usize,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
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

    /// Record a committed insert in the group append log.
    pub(crate) fn record_append_insert(&mut self, gid: usize, local: u32, nbr: Nbr) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.append.inserts.push(AppendInsert { local, nbr });
        }
    }

    /// Record a committed tombstone in the group append log.
    pub(crate) fn record_append_delete(
        &mut self,
        gid: usize,
        local: u32,
        edge_id: EdgeId,
        delete_ts: Timestamp,
    ) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.append.deletes.push(AppendDelete {
                local,
                edge_id,
                delete_ts,
            });
        }
    }

    /// Whether a group holds append-log deltas not yet merged into a base.
    pub fn group_has_append_log(&self, gid: usize) -> bool {
        self.shards
            .get(&gid)
            .is_some_and(|shard| !shard.append.is_empty())
    }

    /// Committed op count held in one group append log.
    pub fn group_append_op_count(&self, gid: usize) -> usize {
        self.shards
            .get(&gid)
            .map_or(0, |shard| shard.append.op_count())
    }

    /// Committed ops held in one group append log, oldest first.
    pub(crate) fn group_append_ops(&self, gid: usize) -> (Vec<AppendInsert>, Vec<AppendDelete>) {
        self.shards
            .get(&gid)
            .map(|shard| (shard.append.inserts.clone(), shard.append.deletes.clone()))
            .unwrap_or_default()
    }

    /// Drop one group append log after its states merged into a base.
    /// The row index is memory only; base files plus later logs rebuild it.
    pub fn clear_group_append_log(&mut self, gid: usize) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.append.clear();
        }
    }

    /// Drop every group append log, e.g. after a topology-wide rebuild
    /// whose base rewrite already carries all states.
    pub fn clear_all_append_logs(&mut self) {
        for shard in self.shards.values_mut() {
            shard.append.clear();
        }
    }

    /// Whether one group carries delete dirt and needs a base merge rather
    /// than an append-only persist.
    pub fn group_needs_rebalance(&self, gid: usize) -> bool {
        self.group_dirty(gid).deleted
    }

    /// Encode one group append log for an append-only checkpoint. Carries
    /// only the address width so group-set growth never invalidates clean
    /// groups' sidecars; a width mismatch is rejected on load instead of
    /// replayed against the wrong base.
    pub fn encode_group_append_log(&self, gid: usize, manifest: &TableShardManifest) -> Vec<u8> {
        let (inserts, deletes): (Vec<AppendInsert>, Vec<AppendDelete>) = self
            .shards
            .get(&gid)
            .map(|shard| (shard.append.inserts.clone(), shard.append.deletes.clone()))
            .unwrap_or_default();
        encode_append_ops(manifest, &inserts, &deletes)
    }

    /// Replay one append-log payload into a group base. Fails closed on
    /// version, address-width, section-size or trailing-byte mismatches.
    pub fn replay_group_append_log(
        &mut self,
        gid: usize,
        data: &[u8],
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        let (inserts, deletes) = decode_append_ops(data, manifest)?;
        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
            StorageError::deserialize_error(format!("group {} out of range on append replay", gid))
        })?;
        for insert in inserts {
            shard
                .variant
                .insert_edge(
                    insert.local,
                    VertexId::edge_endpoint_key(insert.nbr.endpoint, insert.nbr.rank),
                    insert.nbr.edge_id,
                    insert.nbr.create_ts,
                )
                .map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "append log insert replay failed in group {}: {}",
                        gid, e
                    ))
                })?;
        }
        for delete in deletes {
            shard
                .variant
                .delete_edge(delete.local, delete.edge_id, delete.delete_ts)
                .map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "append log delete replay failed in group {}: {}",
                        gid, e
                    ))
                })?;
        }
        Ok(())
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

    /// Whether a group holds uncheckpointed writes.
    pub fn needs_checkpoint(&self, gid: usize) -> bool {
        self.group_dirty(gid).is_dirty()
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

    /// Average bytes per edge based on actual memory usage.
    /// Measured value only, with no fallback: empty tables report zero, and
    /// a degenerate zero measurement on a non-empty table reports zero with
    /// a debug line. Callers handle zero explicitly.
    pub fn bytes_per_edge(&self) -> usize {
        super::FragmentationStats::measured_bytes_per_edge(
            self.used_memory_size(),
            self.edge_count(),
        )
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
    pub fn fragmentation_ratio(&self) -> f32 {
        if self.strategy != EdgeStrategy::Multiple {
            return 0.0;
        }
        let mut total_capacity = 0usize;
        let mut wasted = 0usize;
        for shard in self.shards.values() {
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
            .values()
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

    /// Iterate edges of a vertex without allocating (Multiple only).
    /// Test-only row-stamp filtered iterator; production scans go through the version authority.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> Option<VertexEdgesIter<'_>> {
        let (gid, local) = self.route(src_vid)?;
        self.shards.get(&gid)?.variant.iter_edges_of(local, ts)
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

impl CsrBase for CsrShardSet {
    /// Materialized rows only: existing groups times group size.
    ///
    /// Memory-proportional by design, so sparse tables stay proportional to
    /// materialized groups. Not an address upper bound: holes are excluded,
    /// use `address_span_rows` when a true bound is needed.
    fn vertex_capacity(&self) -> usize {
        self.shards.len() * self.group_size()
    }

    fn edge_count(&self) -> u64 {
        self.shards
            .values()
            .map(|shard| shard.variant.edge_count())
            .sum()
    }

    fn dump(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&SHARD_SET_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.group_bits.to_le_bytes());
        out.extend_from_slice(&(self.shards.len() as u32).to_le_bytes());
        for (gid, shard) in self.shards.iter() {
            out.extend_from_slice(&(*gid as u32).to_le_bytes());
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
        let mut ids = Vec::with_capacity(count);
        let mut payloads = Vec::with_capacity(count);
        for _ in 0..count {
            let gid_bytes: [u8; 4] = take_bytes(data, &mut cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("shard group id too short"))?;
            let gid = u32::from_le_bytes(gid_bytes);
            let len_bytes: [u8; 8] = take_bytes(data, &mut cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("shard payload length too short"))?;
            let len = u64::from_le_bytes(len_bytes) as usize;
            let payload = take_bytes(data, &mut cursor, len)?.to_vec();
            ids.push(gid);
            payloads.push(payload);
        }
        self.set_groups(&ids)?;
        for (gid, payload) in ids.into_iter().zip(payloads.into_iter()) {
            self.load_group(gid as usize, &payload)?;
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
        self.shards
            .get_mut(&gid)
            .ok_or_else(|| {
                StorageError::invalid_operation(format!("missing group {} on insert", gid))
            })?
            .variant
            .insert_edge(local, dst, edge_id, ts)?;
        let (decoded_vid, decoded_rank) = dst.decode_edge_endpoint();
        let decoded_endpoint = decoded_vid.as_u64().unwrap_or(0) as u32;
        let nbr = Nbr::with_create_ts(decoded_endpoint, decoded_rank, edge_id, ts);
        self.mark_region_insert(gid, local);
        self.record_append_insert(gid, local, nbr);
        Ok(())
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Err(no_edges_error());
        };
        let deleted = self
            .shards
            .get_mut(&gid)
            .ok_or_else(no_edges_error)?
            .variant
            .delete_edge(local, edge_id, ts)?;
        if deleted {
            self.mark_region_delete(gid, local);
            self.record_append_delete(gid, local, edge_id, ts);
            if let Some(shard) = self.shards.get_mut(&gid) {
                shard.reclaim_hint = true;
            }
        }
        Ok(deleted)
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        let mut noop = |_: EdgeId| {};
        self.delete_edge_by_dst_reporting(src_vid, dst, ts, &mut noop)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        let Some((gid, local)) = self.route(src_vid) else {
            return 0;
        };
        // Single pass: the variant stamps matches and reports their ids
        // through the callback. Ids are parked locally so the append log is
        // fed after the shard borrow ends, with no separate collection scan
        // over the row first.
        let mut doomed: Vec<EdgeId> = Vec::new();
        let deleted = self
            .shards
            .get_mut(&gid)
            .map(|shard| {
                shard.variant.delete_edge_by_dst_reporting(
                    local,
                    dst,
                    ts,
                    &mut |edge_id: EdgeId| {
                        on_deleted(edge_id);
                        doomed.push(edge_id);
                    },
                )
            })
            .unwrap_or(0);
        if deleted > 0 {
            self.mark_region_delete(gid, local);
            for edge_id in doomed {
                self.record_append_delete(gid, local, edge_id, ts);
            }
            if let Some(shard) = self.shards.get_mut(&gid) {
                shard.reclaim_hint = true;
            }
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
        let before = self
            .shards
            .get(&gid)
            .and_then(|shard| shard.variant.nbr_at_offset(local, offset));
        let deleted = self
            .shards
            .get_mut(&gid)
            .ok_or_else(no_edges_error)?
            .variant
            .delete_edge_by_offset(local, offset, ts)?;
        if deleted {
            self.mark_region_delete(gid, local);
            if let Some(nbr) = before {
                self.record_append_delete(gid, local, nbr.edge_id, ts);
            }
            if let Some(shard) = self.shards.get_mut(&gid) {
                shard.reclaim_hint = true;
            }
        }
        Ok(deleted)
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        let (gid, local) = self.route(src_vid)?;
        self.shards.get(&gid)?.variant.nbr_at_offset(local, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (gid, local) = self.route(src_vid)?;
        self.shards.get(&gid)?.variant.get_edge_physical(local, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Vec::new();
        };
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.physical_edges_of(local))
            .unwrap_or_default()
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        let Some((gid, local)) = self.route(vid) else {
            return false;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.has_physical_entries(local))
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.primary_contains(local, edge_id))
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let reverted = self
            .shards
            .get_mut(&gid)
            .map(|shard| shard.variant.revert_delete_by_offset(local, offset, ts))
            .unwrap_or(false);
        if reverted {
            self.mark_region_delete(gid, local);
        }
        reverted
    }

    fn remove_edge(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let removed = self
            .shards
            .get_mut(&gid)
            .map(|shard| shard.variant.remove_edge(local, edge_id))
            .unwrap_or(false);
        if removed {
            self.mark_region_delete(gid, local);
        }
        removed
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let reverted = self
            .shards
            .get_mut(&gid)
            .map(|shard| shard.variant.revert_delete_by_edge_id(local, edge_id, ts))
            .unwrap_or(false);
        if reverted {
            self.mark_region_delete(gid, local);
        }
        reverted
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (gid, local) = self.route(src_vid)?;
        self.shards.get(&gid)?.variant.get_edge(local, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Vec::new();
        };
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.edges_of(local, ts))
            .unwrap_or_default()
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
        let removed = self
            .shards
            .get_mut(&gid)
            .map(|shard| {
                shard
                    .variant
                    .compact_vertex_with_reporting(local, cutoff, on_edge_removed)
            })
            .unwrap_or(0);
        if removed > 0 {
            self.mark_region_delete(gid, local);
        }
        removed
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        let Some((gid, local)) = self.route(vid) else {
            return 0;
        };
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.reclaimable_count(local, cutoff))
            .unwrap_or(0)
    }

    fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        let Some((gid, local)) = self.route(vid) else {
            return false;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.vertex_needs_compact(local, cutoff))
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let Some((gid, local)) = self.route(vid) else {
            return (0, 0, 0);
        };
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.vertex_census(local))
            .unwrap_or((0, 0, 0))
    }

    fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        let Some((gid, local)) = self.route(vid) else {
            return (0, 0);
        };
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.vertex_reclaim_probe(local, cutoff))
            .unwrap_or((0, 0))
    }

    fn row_gap(&self, vid: u32) -> usize {
        let Some((gid, local)) = self.route(vid) else {
            return 0;
        };
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.row_gap(local))
            .unwrap_or(0)
    }

    fn row_density(&self, vid: u32) -> f32 {
        let Some((gid, local)) = self.route(vid) else {
            return 1.0;
        };
        self.shards
            .get(&gid)
            .map(|shard| shard.variant.row_density(local))
            .unwrap_or(1.0)
    }

    fn rebalance_row(&mut self, vid: u32) -> bool {
        let Some((gid, local)) = self.route(vid) else {
            return true;
        };
        self.shards
            .get_mut(&gid)
            .map(|shard| shard.variant.rebalance_row(local))
            .unwrap_or(true)
    }

    fn used_memory_size(&self) -> usize {
        CsrShardSet::used_memory_size(self)
    }
}

/// Iterator chaining every existing group in group order, translating local
/// rows to global vertex ids. Missing groups are absent and never visited.
pub struct ShardCsrIterator<'a> {
    shards: &'a BTreeMap<usize, Shard>,
    order: Vec<usize>,
    group_bits: u32,
    group_pos: usize,
    base: u32,
    inner: CsrIterator<'a>,
    include_deleted: bool,
    ts: Timestamp,
}

impl<'a> ShardCsrIterator<'a> {
    fn new(
        shards: &'a BTreeMap<usize, Shard>,
        group_bits: u32,
        ts: Timestamp,
        include_deleted: bool,
    ) -> Self {
        Self {
            shards,
            order: shards.keys().copied().collect(),
            group_bits,
            group_pos: 0,
            base: 0,
            inner: CsrIterator::None,
            include_deleted,
            ts,
        }
    }

    fn advance_group(&mut self) -> bool {
        if self.group_pos >= self.order.len() {
            return false;
        }
        let gid = self.order[self.group_pos];
        self.group_pos += 1;
        self.base = group_base(gid, self.group_bits);
        let Some(shard) = self.shards.get(&gid) else {
            return self.advance_group();
        };
        let variant = &shard.variant;
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
            out_groups: vec![0, 1],
            in_groups: vec![0],
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
    fn manifest_v3_counts_are_rejected() {
        let mut legacy = Vec::new();
        legacy.extend_from_slice(&3u32.to_le_bytes());
        legacy.extend_from_slice(&12u32.to_le_bytes());
        legacy.extend_from_slice(&2u32.to_le_bytes());
        legacy.extend_from_slice(&1u32.to_le_bytes());
        assert!(TableShardManifest::decode(&legacy).is_err());
    }

    #[test]
    fn sparse_holes_stay_absent_until_written() {
        let mut set = multi_set();
        set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
            .unwrap();
        assert_eq!(set.existing_group_ids(), vec![0, 2]);
        assert_eq!(set.group_count(), 2);
        assert!(set.get_edge(5000, endpoint(9, 0), 200).is_none());
        assert_eq!(set.existing_group_ids(), vec![0, 2]);
        assert!(set.edges_of(5000, 200).is_empty());
        assert_eq!(set.existing_group_ids(), vec![0, 2]);
    }

    #[test]
    fn none_strategy_holds_no_groups() {
        let mut set = CsrShardSet::new(EdgeStrategy::None, 12, 4096).unwrap();
        assert_eq!(set.group_count(), 0);
        assert_eq!(set.vertex_capacity(), 0);
        assert!(set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).is_err());
        assert!(set.delete_edge(0, EdgeId(0), 100).is_err());
        assert_eq!(set.delete_edge_by_dst(0, endpoint(1, 0), 100), 0);
    }

    #[test]
    fn truncate_drops_trailing_empty_groups() {
        let mut set = multi_set();
        set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
            .unwrap();
        assert_eq!(set.existing_group_ids(), vec![0, 2]);
        assert!(set.remove_edge(9000, EdgeId(0)));
        set.truncate_trailing_empty_groups();
        assert_eq!(set.group_count(), 1);
    }

    #[test]
    fn truncate_keeps_tombstone_only_tail_groups() {
        let mut set = multi_set();
        set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
            .unwrap();
        assert_eq!(set.existing_group_ids(), vec![0, 2]);
        assert!(set.delete_edge(9000, EdgeId(0), 150).unwrap());
        assert_eq!(set.edge_count(), 0);
        set.truncate_trailing_empty_groups();
        assert_eq!(set.group_count(), 1);
        assert_eq!(set.existing_group_ids(), vec![2]);
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
        assert!(set.sampled_column_dirty_group_ids() == vec![0]);
        assert!(set.dirty_group_ids().is_empty());
        assert!(!set.needs_checkpoint(0));
        assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::AppendOnly);
    }

    #[test]
    fn out_of_range_column_trace_leaves_no_sample() {
        let mut set = multi_set();
        assert_eq!(set.group_count(), 1);
        set.mark_column_updated_for(9000);
        assert!(set.sampled_column_dirty_group_ids().is_empty());
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

    fn narrow_set() -> CsrShardSet {
        CsrShardSet::new(EdgeStrategy::Multiple, 9, 4096).unwrap()
    }

    #[test]
    fn region_dirt_tracks_per_region_inserts_and_deletes() {
        let mut set = narrow_set();
        assert_eq!(regions_per_group(set.group_size()), 2);
        set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.insert_edge(300, endpoint(2, 0), EdgeId(1), 100)
            .unwrap();
        assert_eq!(set.dirty_region_ids(0), vec![0, 1]);
        assert!(set.region_needs_checkpoint(0, 0));
        assert!(set.region_needs_checkpoint(0, 1));
        assert!(!set.region_needs_rebalance(0, 0));

        set.clear_region_dirty(0, 0);
        assert!(!set.region_needs_checkpoint(0, 0));
        assert!(set.region_needs_checkpoint(0, 1));
        assert_eq!(set.dirty_group_ids(), vec![0]);

        set.clear_region_dirty(0, 1);
        assert!(set.dirty_group_ids().is_empty());
        assert!(!set.needs_checkpoint(0));
    }

    #[test]
    fn region_delete_dirt_drives_rebalance_signal() {
        let mut set = narrow_set();
        set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.clear_all_dirty();
        assert!(set.delete_edge(10, EdgeId(0), 150).unwrap());
        assert!(set.region_needs_rebalance(0, 0));
        assert!(!set.region_needs_rebalance(0, 1));
        assert!(set.group_needs_rebalance(0));
    }

    #[test]
    fn region_census_and_density_observe_rows() {
        let mut set = narrow_set();
        for i in 0..8u32 {
            set.insert_edge(i, endpoint(100 + i, 0), EdgeId(i as u64), 100)
                .unwrap();
        }
        let (live, dead, capacity) = set.region_census(0, 0);
        assert_eq!(live, 8);
        assert_eq!(dead, 0);
        assert!(capacity >= live);
        let density = set.region_density(0, 0);
        assert!((density - live as f32 / capacity as f32).abs() < 1e-6);
        let (live_empty, _, _) = set.region_census(0, 1);
        assert_eq!(live_empty, 0);
        assert_eq!(set.region_density(0, 1), 1.0);
    }

    #[test]
    fn merge_scope_trigger_stays_per_row_reclaimable() {
        let mut set = narrow_set();
        set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
        // No tombstone and no delete dirt: nothing to merge at any cutoff.
        assert_eq!(set.select_merge_scope(0, 0, 200), None);
        assert!(set.delete_edge(10, EdgeId(0), 150).unwrap());
        // Delete dirt alone selects a scope; density only widens it.
        let scope = set.select_merge_scope(0, 0, 140);
        assert!(scope.is_some());
        // Below the deletion stamp nothing is reclaimable and the region
        // carries delete dirt, so the narrow row scope holds.
        assert_eq!(
            set.select_merge_scope(0, 0, 140),
            Some(RegionMergeScope::Row)
        );
    }

    #[test]
    fn compact_region_is_scoped_to_its_window() {
        let mut set = narrow_set();
        set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.insert_edge(300, endpoint(2, 0), EdgeId(1), 100)
            .unwrap();
        assert!(set.delete_edge(10, EdgeId(0), 150).unwrap());
        assert!(set.delete_edge(300, EdgeId(1), 150).unwrap());
        let mut reported = Vec::new();
        let removed =
            set.compact_region_with_reporting(0, 0, 200, &mut |id, ts| reported.push((id, ts)));
        assert_eq!(removed, 1);
        assert_eq!(reported, vec![(EdgeId(0), 150)]);
        // The sibling region still holds its tombstone.
        assert_eq!(set.region_reclaimable_count(0, 1, 200), 1);
        assert_eq!(set.region_reclaimable_count(0, 0, 200), 0);
    }

    #[test]
    fn append_log_encode_replay_roundtrip() {
        let mut set = narrow_set();
        set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
        set.insert_edge(11, endpoint(2, 0), EdgeId(1), 100).unwrap();
        assert_eq!(set.group_append_op_count(0), 2);
        let manifest = TableShardManifest {
            group_bits: 9,
            out_groups: vec![0],
            in_groups: vec![0],
        };
        let payload = set.encode_group_append_log(0, &manifest);

        let mut loaded = narrow_set();
        loaded
            .replay_group_append_log(0, &payload, &manifest)
            .unwrap();
        assert!(loaded.get_edge(10, endpoint(1, 0), 200).is_some());
        assert!(loaded.get_edge(11, endpoint(2, 0), 200).is_some());
        assert_eq!(loaded.edge_count(), 2);
    }

    #[test]
    fn append_log_rejects_version_manifest_and_trailing() {
        let mut set = narrow_set();
        set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
        let manifest = TableShardManifest {
            group_bits: 9,
            out_groups: vec![0],
            in_groups: vec![0],
        };
        let payload = set.encode_group_append_log(0, &manifest);

        let mut bad = payload.clone();
        bad[0] = 99;
        let mut loaded = narrow_set();
        assert!(loaded.replay_group_append_log(0, &bad, &manifest).is_err());

        let other = TableShardManifest {
            group_bits: 10,
            out_groups: vec![0],
            in_groups: vec![0],
        };
        assert!(loaded.replay_group_append_log(0, &payload, &other).is_err());

        let mut trailing = payload.clone();
        trailing.push(0);
        assert!(loaded
            .replay_group_append_log(0, &trailing, &manifest)
            .is_err());
    }

    #[test]
    fn append_log_cleared_after_merge() {
        let mut set = narrow_set();
        set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
        assert!(set.group_has_append_log(0));
        set.clear_group_append_log(0);
        assert!(!set.group_has_append_log(0));
        assert_eq!(set.group_append_op_count(0), 0);
    }
}

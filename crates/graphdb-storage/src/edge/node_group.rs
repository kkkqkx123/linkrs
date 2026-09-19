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
//!
//! Layout by responsibility (`node_group/` subdirectory):
//! - `address` holds group/region address arithmetic and density calibration.
//! - `dirt` holds the region/group dirt model and checkpoint classification.
//! - `manifest` holds the per-table existing-group layout record.
//! - `append_log` holds the per-group write-through delta and its codec.
//! - `core` holds construction, group materialization, routing and bulk insert.
//! - `read` holds routed physical read helpers.
//! - `iter` holds the cross-group scan iterator.
//! - `compaction` holds region observation, merge-scope selection and
//!   tombstone compaction passes.
//! - `stats` holds observability views summed across groups.
//! - `trait_impl` adapts the set to the CSR traits.

use graphdb_core::types::{EdgeStrategy, Timestamp};
use std::collections::BTreeMap;

use super::CsrVariant;

pub(crate) mod address;
pub(crate) mod append_log;
pub(crate) mod compaction;
pub(crate) mod core;
pub(crate) mod dirt;
pub(crate) mod iter;
pub(crate) mod manifest;
pub(crate) mod read;
pub(crate) mod stats;
pub(crate) mod trait_impl;

#[cfg(test)]
mod tests;

pub use address::{
    calibrator_max_density, calibrator_tree_height, group_base, group_id_for, group_size,
    local_vid, region_id_for_local, region_local_range, region_tail_gap, regions_per_group,
    validate_group_bits, DEFAULT_NODE_GROUP_BITS, LEAF_HIGH_CSR_DENSITY, LEAF_REGION_ROWS,
};
pub use compaction::{RegionMergeScope, GROUP_MERGE_MIN_DENSITY, REGION_MERGE_MIN_DENSITY};
pub use dirt::{EdgeCheckpointKind, GroupDirty, RegionDirty};
pub use iter::ShardCsrIterator;
pub use manifest::{TableShardManifest, GROUP_MANIFEST_VERSION};
pub use stats::NodeGroupStats;

pub(crate) use append_log::{decode_append_ops, encode_append_ops, ShardAppendLog};

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
    /// Current hot-path tombstone reuse cutoff, refreshed by the table
    /// maintenance pass and seeded into every freshly created group.
    tombstone_reuse_cutoff: Timestamp,
    shards: BTreeMap<usize, Shard>,
}

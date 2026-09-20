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
//! Concurrency: this container adds no locking of its own and follows the
//! canonical contract in [`super::mutable_csr`]. All mutation takes `&mut
//! self`; the route cache is the only shared mutable state and stays
//! correct under relaxed atomics because removals invalidate before they
//! unlink.
//!
//! Layout by responsibility (`node_group/` subdirectory):
//! - `address` holds group/region address arithmetic and density calibration.
//! - `dirt` holds the region/group dirt model and checkpoint classification.
//! - `manifest` holds the per-table existing-group layout record.
//! - `append_log` holds the per-group write-through delta and its codec.
//! - `core` holds construction, group materialization, routing and bulk insert.
//! - `freeze` holds explicit group freeze and unfreeze between mutable and
//!   packed forms.
//! - `read` holds routed physical read helpers.
//! - `iter` holds the cross-group scan iterator.
//! - `compaction` holds region observation, merge-scope selection and
//!   tombstone compaction passes.
//! - `stats` holds observability views summed across groups.
//! - `trait_impl` adapts the set to the CSR traits.

use graphdb_core::types::{EdgeStrategy, Timestamp};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::{CsrVariant, RecordForm};

pub(crate) mod address;
pub(crate) mod append_log;
pub(crate) mod compaction;
pub(crate) mod core;
pub(crate) mod dirt;
pub(crate) mod freeze;
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
pub use manifest::TableShardManifest;
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

/// Hot-row routing cache slots. Power of two so the slot is a mask, no
/// division. Sized to cover the working set of repeated point lookups while
/// staying small enough to fit a few cache lines.
const ROUTE_CACHE_SLOTS: usize = 128;
/// Empty-slot sentinel: no vertex id ever routes here since the id space is
/// dense from zero and the sentinel is the maximum value.
const ROUTE_CACHE_EMPTY: u32 = u32::MAX;

/// Lock-free hot-row address cache fronting the group map lookup.
///
/// Point lookups and single-edge writes pay a group-shift plus a `BTreeMap`
/// search per row; hot vertices repeat the same address, so a direct-mapped
/// cache of recent `(vertex, group, local)` triples lets repeats skip the map
/// search. Atomics keep `&self` reads lock-free; ordering stays relaxed
/// because a stale entry only costs a re-lookup on the invalidation paths,
/// never a wrong answer: entries are only populated for existing groups and
/// every group-removal path invalidates its entries first.
struct RouteCache {
    vids: Box<[AtomicU32]>,
    gids: Box<[AtomicUsize]>,
    locals: Box<[AtomicU32]>,
}

impl RouteCache {
    fn new() -> Self {
        Self {
            vids: (0..ROUTE_CACHE_SLOTS)
                .map(|_| AtomicU32::new(ROUTE_CACHE_EMPTY))
                .collect(),
            gids: (0..ROUTE_CACHE_SLOTS)
                .map(|_| AtomicUsize::new(0))
                .collect(),
            locals: (0..ROUTE_CACHE_SLOTS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    #[inline]
    fn slot(vid: u32) -> usize {
        vid as usize & (ROUTE_CACHE_SLOTS - 1)
    }

    fn lookup(&self, vid: u32) -> Option<(usize, u32)> {
        let slot = Self::slot(vid);
        if self.vids[slot].load(Ordering::Relaxed) != vid {
            return None;
        }
        Some((
            self.gids[slot].load(Ordering::Relaxed),
            self.locals[slot].load(Ordering::Relaxed),
        ))
    }

    fn insert(&self, vid: u32, gid: usize, local: u32) {
        let slot = Self::slot(vid);
        self.gids[slot].store(gid, Ordering::Relaxed);
        self.locals[slot].store(local, Ordering::Relaxed);
        self.vids[slot].store(vid, Ordering::Relaxed);
    }

    fn invalidate_gid(&self, gid: usize) {
        for slot in 0..ROUTE_CACHE_SLOTS {
            if self.vids[slot].load(Ordering::Relaxed) != ROUTE_CACHE_EMPTY
                && self.gids[slot].load(Ordering::Relaxed) == gid
            {
                self.vids[slot].store(ROUTE_CACHE_EMPTY, Ordering::Relaxed);
            }
        }
    }

    fn clear(&self) {
        for slot in 0..ROUTE_CACHE_SLOTS {
            self.vids[slot].store(ROUTE_CACHE_EMPTY, Ordering::Relaxed);
        }
    }
}

impl Clone for RouteCache {
    fn clone(&self) -> Self {
        // Clones start cold: copying hot addresses would also copy
        // potentially stale group spacing, so the new set warms its own.
        Self::new()
    }
}

impl fmt::Debug for RouteCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let live = (0..ROUTE_CACHE_SLOTS)
            .filter(|slot| self.vids[*slot].load(Ordering::Relaxed) != ROUTE_CACHE_EMPTY)
            .count();
        f.debug_struct("RouteCache")
            .field("slots", &ROUTE_CACHE_SLOTS)
            .field("live", &live)
            .finish()
    }
}

/// Sharded topology container for one edge direction.
///
/// Routes every row-addressed operation to the group owning the bound
/// endpoint. Read paths never create groups; write paths create missing
/// groups on demand.
pub struct CsrShardSet {
    strategy: EdgeStrategy,
    group_bits: u32,
    overflow_chunk_edges: usize,
    /// Record form locked at construction; determines the CsrVariant kind
    /// created by `fresh_variant`. Immutable after construction.
    record_form: RecordForm,
    /// Current hot-path tombstone reuse cutoff, refreshed by the table
    /// maintenance pass and seeded into every freshly created group.
    tombstone_reuse_cutoff: Timestamp,
    shards: BTreeMap<usize, Shard>,
    /// Hot-row address cache skipping the group map search on repeats.
    /// Addressing accelerator only: group creation, removal and resharding
    /// invalidate the affected entries, and misses never populate for
    /// missing groups, so absent rows still read as empty.
    route_cache: RouteCache,
}

impl Clone for CsrShardSet {
    fn clone(&self) -> Self {
        Self {
            strategy: self.strategy,
            group_bits: self.group_bits,
            overflow_chunk_edges: self.overflow_chunk_edges,
            record_form: self.record_form,
            tombstone_reuse_cutoff: self.tombstone_reuse_cutoff,
            shards: self.shards.clone(),
            route_cache: RouteCache::new(),
        }
    }
}

impl fmt::Debug for CsrShardSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CsrShardSet")
            .field("strategy", &self.strategy)
            .field("group_bits", &self.group_bits)
            .field("overflow_chunk_edges", &self.overflow_chunk_edges)
            .field("record_form", &self.record_form)
            .field("tombstone_reuse_cutoff", &self.tombstone_reuse_cutoff)
            .field("shards", &self.shards)
            .field("route_cache", &self.route_cache)
            .finish_non_exhaustive()
    }
}

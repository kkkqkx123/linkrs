//! Owner-group shard routing for timestamps and properties.

use super::EdgeStore;
use graphdb_core::types::{EdgeId, VertexId};
use std::collections::HashSet;

/// Segmented sparse owner-group index keyed by edge id.
///
/// Edge ids are table-allocated dense values; one segment covers
/// `OWNER_SEGMENT_ROWS` ids and untouched segments stay unallocated.
/// Live entries hold group ids, gaps hold `UNASSIGNED`. Slot order is
/// preserved by ascending segment iteration, matching the property
/// edge-map caliber. Trailing empty segments are truncated so churned id
/// ranges never pin memory.
#[derive(Debug, Clone, Default)]
pub(crate) struct EdgeOwnerMap {
    segments: Vec<Option<Box<[u32; 1024]>>>,
}

impl EdgeOwnerMap {
    pub(crate) const UNASSIGNED: u32 = u32::MAX;
    const SEGMENT_ROWS: usize = 1024;
    const SEGMENT_SHIFT: u32 = 10;
    const SEGMENT_MASK: usize = 1024 - 1;

    pub(crate) fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    #[inline]
    fn segment_of(edge_id: &EdgeId) -> (usize, usize) {
        let slot = edge_id.0 as usize;
        (slot >> Self::SEGMENT_SHIFT, slot & Self::SEGMENT_MASK)
    }

    pub(crate) fn get(&self, edge_id: &EdgeId) -> Option<u32> {
        let (seg, off) = Self::segment_of(edge_id);
        let owner = self.segments.get(seg)?.as_ref()?[off];
        (owner != Self::UNASSIGNED).then_some(owner)
    }

    pub(crate) fn insert(&mut self, edge_id: EdgeId, owner: u32) {
        let (seg, off) = Self::segment_of(&edge_id);
        if seg >= self.segments.len() {
            self.segments.resize_with(seg + 1, || None);
        }
        let segment = self.segments[seg]
            .get_or_insert_with(|| Box::new([Self::UNASSIGNED; Self::SEGMENT_ROWS]));
        segment[off] = owner;
    }

    pub(crate) fn or_insert(&mut self, edge_id: EdgeId, owner: u32) {
        let (seg, off) = Self::segment_of(&edge_id);
        if seg >= self.segments.len() {
            self.segments.resize_with(seg + 1, || None);
        }
        let segment = self.segments[seg]
            .get_or_insert_with(|| Box::new([Self::UNASSIGNED; Self::SEGMENT_ROWS]));
        if segment[off] == Self::UNASSIGNED {
            segment[off] = owner;
        }
    }

    pub(crate) fn remove(&mut self, edge_id: &EdgeId) {
        let (seg, off) = Self::segment_of(edge_id);
        let Some(segment) = self.segments.get_mut(seg).and_then(|s| s.as_mut()) else {
            return;
        };
        if segment[off] == Self::UNASSIGNED {
            return;
        }
        segment[off] = Self::UNASSIGNED;
        if segment.iter().all(|owner| *owner == Self::UNASSIGNED) {
            self.segments[seg] = None;
        }
        self.truncate_empty_tail_segments();
    }

    pub(crate) fn clear(&mut self) {
        self.segments.clear();
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (EdgeId, u32)> + '_ {
        self.segments
            .iter()
            .enumerate()
            .filter_map(|(seg, segment)| {
                segment.as_ref().map(|values| {
                    let base = seg * Self::SEGMENT_ROWS;
                    values.iter().enumerate().filter_map(move |(off, owner)| {
                        (*owner != Self::UNASSIGNED)
                            .then_some((EdgeId((base + off) as u64), *owner))
                    })
                })
            })
            .flatten()
    }

    pub(crate) fn allocated_segments(&self) -> usize {
        self.segments.iter().filter(|seg| seg.is_some()).count()
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        self.segments.capacity() * std::mem::size_of::<Option<Box<[u32; 1024]>>>()
            + self.allocated_segments() * Self::SEGMENT_ROWS * std::mem::size_of::<u32>()
    }

    fn truncate_empty_tail_segments(&mut self) {
        while self.segments.last().is_some_and(|seg| seg.is_none()) {
            self.segments.pop();
        }
    }
}

/// Owner-map rebuild outcome for observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnerRebuildStats {
    /// Topology edges assigned to their current owner group.
    pub mapped: usize,
    /// Authority/property orphans without topology converged to the
    /// fallback group.
    pub relocated_orphans: usize,
    /// The deterministic fallback group orphans converged to.
    pub fallback_group: u32,
}

impl EdgeStore {
    /// Owner group for one edge write. Out groups own when out edges exist,
    /// otherwise in groups own. The owner decides which timestamp and
    /// property shard carries the edge with the same dirt as its topology.
    pub(crate) fn owner_gid_for(&self, src: u32, dst: u32) -> u32 {
        if self.schema.has_out() {
            crate::edge::node_group::group_id_for(src, self.config.node_group_bits) as u32
        } else {
            crate::edge::node_group::group_id_for(dst, self.config.node_group_bits) as u32
        }
    }

    /// Existing owner groups in group order. Timestamp and property shards
    /// follow exactly these groups; missing groups have no shard files.
    pub(crate) fn owner_group_ids(&self) -> Vec<u32> {
        let owner = if self.schema.has_out() {
            &self.out_csr
        } else {
            &self.in_csr
        };
        owner
            .existing_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .collect()
    }

    /// Rebuild the owner map, reporting convergence counts.
    ///
    /// Topology edges take their current owner; timestamps or property rows
    /// without topology fall back to the smallest materialized owner so they
    /// stay in an existing shard. Every fallback is counted in the returned
    /// stats: orphans converge to a deterministic group with an observable
    /// count, never silently dropped. Load and reshard paths assert on the
    /// counts and tests observe orphan convergence directly.
    pub(crate) fn rebuild_owner_map_with_stats(&mut self) -> OwnerRebuildStats {
        self.edge_owner.clear();
        let use_out = self.schema.has_out();
        let owner = if use_out { &self.out_csr } else { &self.in_csr };
        let existing = owner.existing_group_ids();
        let mut mapped = 0usize;
        let mut topology_ids = HashSet::new();
        for gid in &existing {
            if let Some(variant) = owner.group_variant(*gid) {
                for (_, nbr) in variant.iter_all() {
                    self.edge_owner.insert(nbr.edge_id, *gid as u32);
                    topology_ids.insert(nbr.edge_id);
                    mapped += 1;
                }
            }
        }
        let fallback = existing.first().copied().unwrap_or(0) as u32;
        let mut relocated_orphans = 0usize;
        for edge_id in self.mvcc.edge_timestamps.keys() {
            if !topology_ids.contains(&edge_id) {
                relocated_orphans += 1;
            }
            self.edge_owner.or_insert(edge_id, fallback);
        }
        for edge_id in self.properties.edge_ids() {
            self.edge_owner.or_insert(edge_id, fallback);
        }
        OwnerRebuildStats {
            mapped,
            relocated_orphans,
            fallback_group: fallback,
        }
    }

    pub(crate) fn edge_endpoint_key(endpoint: u32, rank: i64) -> VertexId {
        VertexId::edge_endpoint_key(endpoint, rank)
    }

    pub(crate) fn resolve_owner_gid(
        edge_id: &EdgeId,
        edge_owner: &EdgeOwnerMap,
        live: &HashSet<u32>,
        fallback: Option<u32>,
    ) -> (u32, bool) {
        let owner = edge_owner.get(edge_id).unwrap_or(0);
        if live.contains(&owner) {
            (owner, false)
        } else {
            (fallback.unwrap_or(0), true)
        }
    }

    /// Address-span row counts including holes, one per direction.
    ///
    /// True address upper bounds for preallocation and range validation.
    /// Materialized capacity instead reports existing groups only and stays
    /// memory-proportional. Callers must pick by purpose and never mix the
    /// two calibers.
    pub fn topology_address_span(&self) -> (usize, usize) {
        (
            self.out_csr.address_span_rows(),
            self.in_csr.address_span_rows(),
        )
    }

    pub(crate) fn try_decode_edge_endpoint(key: VertexId) -> Option<(VertexId, i64)> {
        let decoded = key.try_decode_edge_endpoint();
        if decoded.is_none() {
            log::warn!(
                "try_decode_edge_endpoint: unexpected key kind {:?} length {}, expected a 16-byte edge endpoint key",
                key.kind(),
                key.len(),
            );
        }
        decoded
    }
}

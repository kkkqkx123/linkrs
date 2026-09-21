//! Owner-group shard routing for timestamps and properties.

use super::EdgeStore;
use graphdb_core::types::{EdgeId, VertexId};
use std::collections::HashSet;

/// Dense owner-group index keyed by edge id.
///
/// Edge ids are assigned monotonically per table, so the owner of every edge
/// is addressed by direct subscript instead of hashing. Unassigned slots
/// hold a sentinel and read as absent.
#[derive(Debug, Clone, Default)]
pub(crate) struct EdgeOwnerMap {
    slots: Vec<u32>,
}

impl EdgeOwnerMap {
    pub(crate) const UNASSIGNED: u32 = u32::MAX;

    pub(crate) fn new() -> Self {
        Self { slots: Vec::new() }
    }

    pub(crate) fn get(&self, edge_id: &EdgeId) -> Option<u32> {
        self.slots
            .get(edge_id.0 as usize)
            .copied()
            .filter(|owner| *owner != Self::UNASSIGNED)
    }

    pub(crate) fn insert(&mut self, edge_id: EdgeId, owner: u32) {
        let idx = edge_id.0 as usize;
        if self.slots.len() <= idx {
            self.slots.resize(idx + 1, Self::UNASSIGNED);
        }
        self.slots[idx] = owner;
    }

    pub(crate) fn or_insert(&mut self, edge_id: EdgeId, owner: u32) {
        let idx = edge_id.0 as usize;
        if self.slots.len() <= idx {
            self.slots.resize(idx + 1, Self::UNASSIGNED);
        }
        if self.slots[idx] == Self::UNASSIGNED {
            self.slots[idx] = owner;
        }
    }

    pub(crate) fn remove(&mut self, edge_id: &EdgeId) {
        let idx = edge_id.0 as usize;
        if idx < self.slots.len() {
            self.slots[idx] = Self::UNASSIGNED;
        }
    }

    pub(crate) fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = Self::UNASSIGNED;
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (EdgeId, u32)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, owner)| **owner != Self::UNASSIGNED)
            .map(|(idx, owner)| (EdgeId(idx as u64), *owner))
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

    pub(crate) fn decode_edge_endpoint(key: VertexId) -> (VertexId, i64) {
        let bytes = key.as_bytes();
        if bytes.len() != 16 {
            log::warn!(
                "decode_edge_endpoint: unexpected key length {}, expected 16",
                bytes.len()
            );
        }
        key.decode_edge_endpoint()
    }
}

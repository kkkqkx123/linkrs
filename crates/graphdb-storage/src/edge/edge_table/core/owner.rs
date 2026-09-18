//! Owner-group shard routing for timestamps and properties.

use super::EdgeStore;
use graphdb_core::types::{EdgeId, VertexId};
use std::collections::{HashMap, HashSet};

impl EdgeStore {
    /// Owner group for one edge write. Out groups own when out edges exist,
    /// otherwise in groups own. The owner decides which timestamp and
    /// property shard carries the edge with the same dirt as its topology.
    pub(crate) fn owner_gid_for(&self, src: u32, dst: u32) -> u32 {
        if self.schema.oe_strategy != super::super::super::EdgeStrategy::None {
            crate::edge::node_group::group_id_for(src, self.config.node_group_bits) as u32
        } else {
            crate::edge::node_group::group_id_for(dst, self.config.node_group_bits) as u32
        }
    }

    /// Existing owner groups in group order. Timestamp and property shards
    /// follow exactly these groups; missing groups have no shard files.
    pub(crate) fn owner_group_ids(&self) -> Vec<u32> {
        let owner = if self.schema.oe_strategy != super::super::super::EdgeStrategy::None {
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

    /// Rebuild the owner map from topology plus authority leftovers.
    /// Topology edges take their current owner; timestamps or property rows
    /// without topology (reclaimed physical rows whose authority tombstone
    /// survives) fall back to the smallest materialized owner so they stay
    /// in an existing shard.
    pub(crate) fn rebuild_owner_map(&mut self) {
        self.edge_owner.clear();
        let use_out = self.schema.oe_strategy != super::super::super::EdgeStrategy::None;
        let owner = if use_out { &self.out_csr } else { &self.in_csr };
        let existing = owner.existing_group_ids();
        for gid in &existing {
            if let Some(variant) = owner.group_variant(*gid) {
                for (_, nbr) in variant.iter_all() {
                    self.edge_owner.insert(nbr.edge_id, *gid as u32);
                }
            }
        }
        let fallback = existing.first().copied().unwrap_or(0) as u32;
        for edge_id in self.mvcc.edge_timestamps.keys() {
            self.edge_owner.entry(*edge_id).or_insert(fallback);
        }
        for edge_id in self.properties.edge_ids() {
            self.edge_owner.entry(edge_id).or_insert(fallback);
        }
    }

    pub(crate) fn edge_endpoint_key(endpoint: u32, rank: i64) -> VertexId {
        VertexId::edge_endpoint_key(endpoint, rank)
    }

    pub(crate) fn resolve_owner_gid(
        edge_id: &EdgeId,
        edge_owner: &HashMap<EdgeId, u32>,
        live: &HashSet<u32>,
        fallback: Option<u32>,
    ) -> (u32, bool) {
        let owner = edge_owner.get(edge_id).copied().unwrap_or(0);
        if live.contains(&owner) {
            (owner, false)
        } else {
            (fallback.unwrap_or(0), true)
        }
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

//! CSR trait adapters for the shard set.
//!
//! `MutableCsrTrait` routes every row-addressed mutation and read to the
//! group owning the bound endpoint, mirroring dirt and append-log recording
//! of the single-edge path. `CsrBase` keeps whole-direction dump/load as a
//! fail-closed guard: persistence moves through the per-group incremental
//! protocol only.

use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::super::{csr_shared::decode_endpoint_pair, CsrBase, EdgePosition, MutableCsrTrait, Nbr};
use super::{local_vid, CsrShardSet};

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

    /// Whole-direction dump is rejected by design: persistence moves through
    /// the per-group incremental protocol only, so a whole-direction payload
    /// would silently bypass group dirt, append sidecars and shard manifests.
    /// Any call here is a caller bug and fails loudly instead of returning
    /// an empty payload that a reader could mistake for an empty table.
    fn dump(&self) -> Vec<u8> {
        panic!("whole-direction shard dump removed: use per-group incremental protocol");
    }

    fn dump_into(&self, _out: &mut Vec<u8>) {
        panic!("whole-direction shard dump removed: use per-group incremental protocol");
    }

    /// Whole-direction load is rejected by design, mirroring the dump guard
    /// above. Group payloads load through the per-group path instead.
    fn load(&mut self, _data: &[u8]) -> StorageResult<()> {
        Err(StorageError::deserialize_error(
            "whole-direction shard dump removed: use per-group incremental protocol".to_string(),
        ))
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
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
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

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        let Some((gid, local)) = self.route(src_vid) else {
            return 0;
        };
        let mut doomed: Vec<(EdgeId, Option<EdgePosition>)> = Vec::new();
        let deleted = self
            .shards
            .get_mut(&gid)
            .map(|shard| {
                shard.variant.delete_edge_by_dst_reporting_positioned(
                    local,
                    dst,
                    ts,
                    &mut |edge_id: EdgeId, position: Option<EdgePosition>| {
                        on_deleted(edge_id, position);
                        doomed.push((edge_id, position));
                    },
                )
            })
            .unwrap_or(0);
        if deleted > 0 {
            self.mark_region_delete(gid, local);
            for (edge_id, _) in doomed {
                self.record_append_delete(gid, local, edge_id, ts);
            }
            if let Some(shard) = self.shards.get_mut(&gid) {
                shard.reclaim_hint = true;
            }
        }
        deleted
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        let (gid, local) = self.route(src_vid)?;
        self.shards
            .get(&gid)
            .and_then(|shard| shard.variant.locate_edge(local, edge_id))
    }

    fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        let Some((gid, local)) = self.route(src_vid) else {
            return Ok(false);
        };
        let deleted = self
            .shards
            .get_mut(&gid)
            .ok_or_else(no_edges_error)?
            .variant
            .delete_edge_at_position(local, position, expected, ts)?;
        if deleted {
            self.mark_region_delete(gid, local);
            self.record_append_delete(gid, local, expected, ts);
            if let Some(shard) = self.shards.get_mut(&gid) {
                shard.reclaim_hint = true;
            }
        }
        Ok(deleted)
    }

    fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards
            .get_mut(&gid)
            .map(|shard| {
                shard
                    .variant
                    .revert_delete_at_position(local, position, expected, ts)
            })
            .unwrap_or(false)
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

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        CsrShardSet::fill_physical_into(self, src_vid, out)
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

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let removed = self
            .shards
            .get_mut(&gid)
            .map(|shard| shard.variant.rollback_insert(local, edge_id))
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

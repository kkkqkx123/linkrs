use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::StorageResult;

use super::super::csr_trait::MutableCsrTrait;
use super::super::{EdgePosition, Nbr};
use super::{BundledCsr, BundledOverflowValues};

impl MutableCsrTrait for BundledCsr {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        self.insert_edge_with_value(src_vid, dst, edge_id, None)
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        let located = self.topology.locate_edge(src_vid, edge_id);
        let deleted = self.topology.delete_edge(src_vid, edge_id, ts)?;
        if deleted {
            if let Some((position, _)) = located {
                self.clear_value_at_position(src_vid, position);
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
        let mut positioned = |edge_id: EdgeId, position: Option<EdgePosition>| {
            on_deleted(edge_id);
            let _ = position;
        };
        self.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, &mut positioned)
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        let mut stamped: Vec<(EdgeId, EdgePosition)> = Vec::new();
        let deleted = self.topology.delete_edge_by_dst_reporting_positioned(
            src_vid,
            dst,
            ts,
            &mut |edge_id, position| {
                if let Some(position) = position {
                    stamped.push((edge_id, position));
                }
            },
        );
        for (edge_id, position) in &stamped {
            self.clear_value_at_position(src_vid, *position);
            on_deleted(*edge_id, Some(*position));
        }
        // Topology positions are this variant's positions, so a complete
        // report means every stamped edge carried one.
        debug_assert_eq!(stamped.len(), deleted);
        deleted
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        self.topology.locate_edge(src_vid, edge_id)
    }

    fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        let deleted = self
            .topology
            .delete_edge_at_position(src_vid, position, expected, ts)?;
        if deleted {
            self.clear_value_at_position(src_vid, position);
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
        if !self
            .topology
            .revert_delete_at_position(src_vid, position, expected, ts)
        {
            return false;
        }
        // Holes are never reused by inserts, so the stale raw word still
        // belongs to this edge: revive its validity instead of NULLing it.
        // Callers carrying an explicit value use
        // `revert_delete_at_position_with_value`.
        self.sync_primary_len();
        self.sync_overflow_row(src_vid);
        match position {
            EdgePosition::Primary { slot } => {
                if let Some(idx) = self.primary_index(src_vid, slot) {
                    if idx < self.primary_valid.len() {
                        self.primary_valid.set(idx, true);
                    }
                }
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                if let Some(chunks) = self.overflow_values.get_mut(src_vid) {
                    if let Some(c) = chunks.get_mut(chunk as usize) {
                        if (slot as usize) < c.len() {
                            c.set_valid(slot as usize, true);
                        }
                    }
                }
                true
            }
        }
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if offset < 0 {
            return Ok(false);
        }
        let position = EdgePosition::Primary {
            slot: offset as u32,
        };
        let deleted = self.topology.delete_edge_by_offset(src_vid, offset, ts)?;
        if deleted {
            self.clear_value_at_position(src_vid, position);
        }
        Ok(deleted)
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        // Topology slots erase the edge id on delete, so offset-only and
        // id-only reverts cannot recover it. Use the positioned revert with
        // the expected id and value supplied by the caller.
        let _ = (src_vid, offset, ts);
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        self.topology.nbr_at_offset(src_vid, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        self.topology.get_edge_physical(src_vid, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        self.topology.physical_edges_of(src_vid)
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        self.topology.fill_physical_into(src_vid, out)
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        self.topology.has_physical_entries(vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        self.topology.primary_contains(src_vid, edge_id)
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        self.rollback_insert_shifted(src_vid, edge_id)
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        // Deleted topology slots hold the sentinel instead of the edge id,
        // so an id-keyed scan cannot locate them. Use the positioned revert
        // with value instead.
        let _ = (src_vid, edge_id, ts);
        false
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        self.topology.get_edge(src_vid, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        self.topology.edges_of(src_vid, ts)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        _cutoff: Timestamp,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        self.compact_vertex_shifted(vid, _on_edge_removed)
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        self.topology.reclaimable_count(vid, cutoff)
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        self.topology.vertex_census(vid)
    }

    fn row_gap(&self, vid: u32) -> usize {
        self.topology.row_gap(vid)
    }

    fn row_density(&self, vid: u32) -> f32 {
        self.topology.row_density(vid)
    }

    fn rebalance_row(&mut self, vid: u32) -> bool {
        self.topology.rebalance_row(vid)
    }

    fn used_memory_size(&self) -> usize {
        self.topology.used_memory_size()
            + self.primary_values.len() * 8
            + self.primary_valid.len().div_ceil(8)
            + self
                .overflow_values
                .iter()
                .map(|(_, chunks)| {
                    chunks
                        .iter()
                        .map(BundledOverflowValues::heap_bytes)
                        .sum::<usize>()
                })
                .sum::<usize>()
            + self.overflow_values.table_bytes()
    }
}

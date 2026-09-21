use crate::edge::{EdgePosition, Nbr};
use super::MappedFrozen;

use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

fn mapped_frozen_error() -> StorageError {
    StorageError::invalid_operation(
        "mapped frozen CSR group rejects writes: unfreeze the group before writing".to_string(),
    )
}

impl crate::edge::MutableCsrTrait for MappedFrozen {
    fn insert_edge(
        &mut self,
        _src_vid: u32,
        _dst: VertexId,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        Err(mapped_frozen_error())
    }

    fn delete_edge(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(mapped_frozen_error())
    }

    fn delete_edge_by_dst(&mut self, _src_vid: u32, _dst: VertexId, _ts: Timestamp) -> usize {
        0
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        MappedFrozen::locate_edge(self, src_vid, edge_id)
    }

    fn delete_edge_at_position(
        &mut self,
        _src_vid: u32,
        _position: EdgePosition,
        _expected: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(mapped_frozen_error())
    }

    fn revert_delete_at_position(
        &mut self,
        _src_vid: u32,
        _position: EdgePosition,
        _expected: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        false
    }

    fn delete_edge_by_offset(
        &mut self,
        _src_vid: u32,
        _offset: i32,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(mapped_frozen_error())
    }

    fn revert_delete_by_offset(&mut self, _src_vid: u32, _offset: i32, _ts: Timestamp) -> bool {
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        MappedFrozen::nbr_at_offset(self, src_vid, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        MappedFrozen::get_edge_physical(self, src_vid, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        MappedFrozen::physical_edges_of(self, src_vid)
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        MappedFrozen::fill_physical_into(self, src_vid, out);
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        MappedFrozen::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        MappedFrozen::primary_contains(self, src_vid, edge_id)
    }

    fn rollback_insert(&mut self, _src_vid: u32, _edge_id: EdgeId) -> bool {
        false
    }

    fn revert_delete_by_edge_id(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        false
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        MappedFrozen::get_edge(self, src_vid, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        MappedFrozen::edges_of(self, src_vid, ts)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        _vid: u32,
        _cutoff: Timestamp,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        0
    }

    fn reclaimable_count(&self, _vid: u32, _cutoff: Timestamp) -> usize {
        0
    }

    fn vertex_needs_compact(&self, _vid: u32, _cutoff: Timestamp) -> bool {
        false
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        MappedFrozen::vertex_census(self, vid)
    }

    fn vertex_reclaim_probe(&self, vid: u32, _cutoff: Timestamp) -> (usize, usize) {
        let (_, dead, _) = MappedFrozen::vertex_census(self, vid);
        (dead, 0)
    }

    fn row_gap(&self, _vid: u32) -> usize {
        0
    }

    fn row_density(&self, _vid: u32) -> f32 {
        1.0
    }

    fn rebalance_row(&mut self, _vid: u32) -> bool {
        true
    }

    fn used_memory_size(&self) -> usize {
        MappedFrozen::used_memory_size(self)
    }
}

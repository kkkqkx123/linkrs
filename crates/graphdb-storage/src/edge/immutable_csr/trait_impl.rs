use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::StorageResult;

use super::super::csr_trait::MutableCsrTrait;
use super::super::{EdgePosition, Nbr};
use super::pack::frozen_error;
use super::ImmutableCsr;

impl MutableCsrTrait for ImmutableCsr {
    fn insert_edge(
        &mut self,
        _src_vid: u32,
        _dst: VertexId,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        Err(frozen_error())
    }

    fn delete_edge(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(frozen_error())
    }

    fn delete_edge_by_dst(&mut self, _src_vid: u32, _dst: VertexId, _ts: Timestamp) -> usize {
        0
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        ImmutableCsr::locate_edge(self, src_vid, edge_id)
    }

    fn delete_edge_at_position(
        &mut self,
        _src_vid: u32,
        _position: EdgePosition,
        _expected: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(frozen_error())
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
        Err(frozen_error())
    }

    fn revert_delete_by_offset(&mut self, _src_vid: u32, _offset: i32, _ts: Timestamp) -> bool {
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        ImmutableCsr::nbr_at_offset(self, src_vid, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        ImmutableCsr::get_edge_physical(self, src_vid, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        ImmutableCsr::physical_edges_of(self, src_vid)
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        ImmutableCsr::fill_physical_into(self, src_vid, out);
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        ImmutableCsr::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        ImmutableCsr::primary_contains(self, src_vid, edge_id)
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
        ImmutableCsr::get_edge(self, src_vid, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        ImmutableCsr::edges_of(self, src_vid, ts)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        ImmutableCsr::compact_row(self, vid, cutoff, on_edge_removed)
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        ImmutableCsr::reclaimable_count(self, vid, cutoff)
    }

    fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        ImmutableCsr::reclaimable_count(self, vid, cutoff) > 0
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        ImmutableCsr::vertex_census(self, vid)
    }

    fn vertex_reclaim_probe(&self, _vid: u32, _cutoff: Timestamp) -> (usize, usize) {
        (0, 0)
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
        ImmutableCsr::used_memory_size(self)
    }
}

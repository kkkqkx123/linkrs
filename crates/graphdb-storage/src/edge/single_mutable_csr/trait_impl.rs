//! Trait adapters: `CsrBase` and `MutableCsrTrait` for `SingleMutableCsr`.

use graphdb_core::StorageResult;

use super::super::{CsrBase, EdgeId, EdgePosition, MutableCsrTrait, Nbr, Timestamp, VertexId};
use super::SingleMutableCsr;

impl CsrBase for SingleMutableCsr {
    fn vertex_capacity(&self) -> usize {
        SingleMutableCsr::vertex_capacity(self)
    }

    fn edge_count(&self) -> u64 {
        self.edge_count
    }

    fn dump(&self) -> Vec<u8> {
        SingleMutableCsr::dump(self)
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        SingleMutableCsr::dump_into(self, out)
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        SingleMutableCsr::load(self, data)
    }
}

impl MutableCsrTrait for SingleMutableCsr {
    fn insert_edge(
        &mut self,
        src: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        SingleMutableCsr::insert_edge(self, src, dst, edge_id, ts)
    }

    fn delete_edge(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        SingleMutableCsr::delete_edge(self, src, edge_id, ts)
    }

    fn delete_edge_by_dst(&mut self, src: u32, dst: VertexId, ts: Timestamp) -> usize {
        SingleMutableCsr::delete_edge_by_dst(self, src, dst, ts)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        SingleMutableCsr::delete_edge_by_dst_reporting(self, src, dst, ts, on_deleted)
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        SingleMutableCsr::delete_edge_by_dst_reporting_positioned(self, src, dst, ts, on_deleted)
    }

    fn locate_edge(&self, src: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        SingleMutableCsr::locate_edge(self, src, edge_id)
    }

    fn delete_edge_at_position(
        &mut self,
        src: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        SingleMutableCsr::delete_edge_at_position(self, src, position, expected, ts)
    }

    fn revert_delete_at_position(
        &mut self,
        src: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        SingleMutableCsr::revert_delete_at_position(self, src, position, expected, ts)
    }

    fn delete_edge_by_offset(
        &mut self,
        src: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        SingleMutableCsr::delete_edge_by_offset(self, src, offset, ts)
    }

    fn revert_delete_by_offset(&mut self, src: u32, offset: i32, ts: Timestamp) -> bool {
        SingleMutableCsr::revert_delete_by_offset(self, src, offset, ts)
    }

    fn nbr_at_offset(&self, src: u32, offset: i32) -> Option<Nbr> {
        SingleMutableCsr::nbr_at_offset(self, src, offset)
    }

    fn get_edge_physical(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        SingleMutableCsr::get_edge_physical(self, src, dst)
    }

    fn physical_edges_of(&self, src: u32) -> Vec<Nbr> {
        SingleMutableCsr::physical_edges_of(self, src)
    }

    fn fill_physical_into(&self, src: u32, out: &mut Vec<Nbr>) {
        SingleMutableCsr::fill_physical_into(self, src, out)
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        SingleMutableCsr::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        SingleMutableCsr::primary_contains(self, src_vid, edge_id)
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        SingleMutableCsr::rollback_insert(self, src_vid, edge_id)
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        SingleMutableCsr::revert_delete_by_edge_id(self, src_vid, edge_id, ts)
    }

    fn get_edge(&self, src: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        SingleMutableCsr::get_edge(self, src, dst, ts)
    }

    fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        SingleMutableCsr::edges_of(self, src, ts)
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        SingleMutableCsr::reclaimable_count(self, vid, cutoff)
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        SingleMutableCsr::vertex_census(self, vid)
    }

    fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        SingleMutableCsr::vertex_reclaim_probe(self, vid, cutoff)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        SingleMutableCsr::compact_vertex_with_reporting(self, vid, cutoff, on_edge_removed)
    }

    fn used_memory_size(&self) -> usize {
        SingleMutableCsr::used_memory_size(self)
    }
}

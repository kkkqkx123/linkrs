use graphdb_core::{StorageError, StorageResult};

use super::super::{EdgeId, EdgePosition, MutableCsrTrait, Nbr, Timestamp, VertexId};
use super::CsrVariant;

impl MutableCsrTrait for CsrVariant {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        match self {
            CsrVariant::Multiple(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Single(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Pure(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Bundled(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Frozen(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Mapped(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::None { .. } => Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            )),
        }
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        dispatch!(self, delete_edge(src_vid, edge_id, ts) -> Err(StorageError::invalid_operation(
            "no edges stored for this edge type".to_string()
        )))
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        dispatch!(self, delete_edge_by_dst(src_vid, dst, ts) -> 0)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        // Count-returning deletes report zero for read-only and empty forms:
        // frozen and mapped groups need an explicit unfreeze first, and the
        // placeholder holds no edges. Result-returning deletes below refuse
        // those forms with an error instead of a silent zero.
        match self {
            CsrVariant::Multiple(csr) => {
                csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Single(csr) => {
                csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Pure(csr) => csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted),
            CsrVariant::Bundled(csr) => {
                csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.delete_edge_by_dst_reporting_positioned(
                src_vid,
                dst,
                ts,
                &mut |edge_id, position| on_deleted(edge_id, Some(position)),
            ),
            CsrVariant::Single(csr) => {
                csr.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Pure(csr) => {
                csr.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Bundled(csr) => {
                csr.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, on_deleted)
            }
            _ => self.delete_edge_by_dst_reporting(src_vid, dst, ts, &mut |edge_id| {
                on_deleted(edge_id, None)
            }),
        }
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        match self {
            CsrVariant::Multiple(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Single(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Pure(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Bundled(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Frozen(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Mapped(csr) => csr.locate_edge(src_vid, edge_id),
            _ => None,
        }
    }

    fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.delete_edge_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Single(csr) => {
                csr.delete_edge_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Pure(csr) => csr.delete_edge_at_position(src_vid, position, expected, ts),
            CsrVariant::Bundled(csr) => {
                csr.delete_edge_at_position(src_vid, position, expected, ts)
            }
            _ => Err(StorageError::invalid_operation(
                "row position must not cross variants; re-resolve by edge id".to_string(),
            )),
        }
    }

    fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.revert_delete_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Single(csr) => {
                csr.revert_delete_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Pure(csr) => csr.revert_delete_at_position(src_vid, position, expected, ts),
            CsrVariant::Bundled(csr) => {
                csr.revert_delete_at_position(src_vid, position, expected, ts)
            }
            _ => {
                debug_assert!(
                    false,
                    "row position must not cross variants; re-resolve by edge id"
                );
                false
            }
        }
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        match self {
            CsrVariant::Multiple(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Single(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Pure(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Bundled(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Frozen(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Mapped(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::None { .. } => Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            )),
        }
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        dispatch!(self, revert_delete_by_offset(src_vid, offset, ts) -> false)
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        dispatch!(self, nbr_at_offset(src_vid, offset) -> None)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        dispatch!(self, get_edge_physical(src_vid, dst) -> None)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        dispatch!(self, physical_edges_of(src_vid) -> Vec::new())
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        match self {
            CsrVariant::Multiple(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Single(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Pure(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Bundled(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Frozen(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Mapped(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::None { .. } => out.clear(),
        }
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.has_physical_entries(vid),
            CsrVariant::Single(csr) => csr.has_physical_entries(vid),
            CsrVariant::Pure(csr) => csr.has_physical_entries(vid),
            CsrVariant::Bundled(csr) => csr.has_physical_entries(vid),
            CsrVariant::Frozen(csr) => csr.has_physical_entries(vid),
            CsrVariant::Mapped(csr) => csr.has_physical_entries(vid),
            CsrVariant::None { .. } => false,
        }
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Single(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Pure(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Bundled(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Frozen(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Mapped(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::None { .. } => false,
        }
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        dispatch!(self, rollback_insert(src_vid, edge_id) -> false)
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        dispatch!(self, revert_delete_by_edge_id(src_vid, edge_id, ts) -> false)
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        dispatch!(self, get_edge(src_vid, dst, ts) -> None)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        dispatch!(self, edges_of(src_vid, ts) -> Vec::new())
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Single(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Pure(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Bundled(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Single(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Pure(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Bundled(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Frozen(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        self.reclaimable_count(vid, cutoff) > 0
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        match self {
            CsrVariant::Multiple(csr) => csr.vertex_census(vid),
            CsrVariant::Single(csr) => csr.vertex_census(vid),
            CsrVariant::Pure(csr) => csr.vertex_census(vid),
            CsrVariant::Bundled(csr) => csr.vertex_census(vid),
            CsrVariant::Frozen(csr) => csr.vertex_census(vid),
            CsrVariant::Mapped(csr) => csr.vertex_census(vid),
            CsrVariant::None { .. } => (0, 0, 0),
        }
    }

    fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        match self {
            CsrVariant::Multiple(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::Single(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::Pure(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::Bundled(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::Frozen(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::Mapped(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::None { .. } => (0, 0),
        }
    }

    fn row_gap(&self, vid: u32) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.row_gap(vid),
            CsrVariant::Pure(csr) => csr.row_gap(vid),
            CsrVariant::Bundled(csr) => csr.row_gap(vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => 0,
        }
    }

    fn row_density(&self, vid: u32) -> f32 {
        match self {
            CsrVariant::Multiple(csr) => csr.row_density(vid),
            CsrVariant::Pure(csr) => csr.row_density(vid),
            CsrVariant::Bundled(csr) => csr.row_density(vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => 1.0,
        }
    }

    fn rebalance_row(&mut self, vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.rebalance_row(vid),
            CsrVariant::Pure(csr) => csr.rebalance_row(vid),
            CsrVariant::Bundled(csr) => csr.rebalance_row(vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => true,
        }
    }

    fn used_memory_size(&self) -> usize {
        match self {
            CsrVariant::None { .. } => std::mem::size_of::<Self>(),
            CsrVariant::Multiple(csr) => csr.used_memory_size(),
            CsrVariant::Single(csr) => csr.used_memory_size(),
            CsrVariant::Pure(csr) => csr.used_memory_size(),
            CsrVariant::Bundled(csr) => csr.used_memory_size(),
            CsrVariant::Frozen(csr) => csr.used_memory_size(),
            CsrVariant::Mapped(csr) => csr.used_memory_size(),
        }
    }
}

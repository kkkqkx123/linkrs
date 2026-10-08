use linkrs_core::{StorageError, StorageResult};

use super::super::{
    EdgeId, EdgePosition, MutableCsrTrait, Nbr, Timestamp, VertexId, NO_EDGES_STORED_MSG,
    ROW_POSITION_CROSS_VARIANT_MSG,
};
use super::CsrVariant;

impl MutableCsrTrait for CsrVariant {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        // Capability gate first: writability decides the outcome, the match
        // below only forwards to the concrete writable form.
        if self.is_empty_placeholder() {
            return Err(StorageError::invalid_operation(
                NO_EDGES_STORED_MSG.to_string(),
            ));
        }
        if !self.is_writable() {
            return match self {
                CsrVariant::Frozen(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
                _ => Err(StorageError::invalid_operation(
                    NO_EDGES_STORED_MSG.to_string(),
                )),
            };
        }
        match self {
            CsrVariant::Multiple(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Single(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Pure(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Bundled(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Frozen(_) | CsrVariant::None { .. } => {
                unreachable!("capability gate handles non-writable forms above")
            }
        }
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        // Same capability gate as inserts: result-returning deletes refuse
        // read-only and placeholder forms with an error, never a silent miss.
        // The read-only arm forwards so frozen keeps its own rejection
        // wording; the match below only serves writable forms.
        if self.is_empty_placeholder() {
            return Err(StorageError::invalid_operation(
                NO_EDGES_STORED_MSG.to_string(),
            ));
        }
        if self.is_read_only() {
            return match self {
                CsrVariant::Frozen(csr) => csr.delete_edge(src_vid, edge_id, ts),
                _ => Err(StorageError::invalid_operation(
                    NO_EDGES_STORED_MSG.to_string(),
                )),
            };
        }
        match self {
            CsrVariant::Multiple(csr) => csr.delete_edge(src_vid, edge_id, ts),
            CsrVariant::Single(csr) => csr.delete_edge(src_vid, edge_id, ts),
            CsrVariant::Pure(csr) => csr.delete_edge(src_vid, edge_id, ts),
            CsrVariant::Bundled(csr) => csr.delete_edge(src_vid, edge_id, ts),
            CsrVariant::Frozen(_) | CsrVariant::None { .. } => {
                unreachable!("capability gate handles non-writable forms above")
            }
        }
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        // Counting deletes stay silent on read-only and placeholder forms;
        // the capability query owns that decision.
        if self.is_read_only() || self.is_empty_placeholder() {
            return 0;
        }
        dispatch!(self, delete_edge_by_dst(src_vid, dst, ts) -> 0)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        // Same gate as the other counting deletes: read-only and placeholder
        // forms report zero here, while result-returning deletes above refuse
        // those forms with an error instead of a silent zero.
        if self.is_read_only() || self.is_empty_placeholder() {
            return 0;
        }
        dispatch!(self, delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted) -> 0)
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        // Read-only and placeholder forms report zero directly instead of
        // re-entering the counting path; only writable forms carry positions.
        if self.is_read_only() || self.is_empty_placeholder() {
            return 0;
        }
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
            CsrVariant::Frozen(_) | CsrVariant::None { .. } => {
                unreachable!("capability gate handles non-writable forms above")
            }
        }
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        match self {
            CsrVariant::Multiple(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Single(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Pure(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Bundled(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Frozen(csr) => csr.locate_edge(src_vid, edge_id),
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
        // Positions are variant-local; forms without positional addressing
        // refuse instead of falling back to an id scan.
        if !self.supports_positions() {
            return Err(StorageError::invalid_operation(
                ROW_POSITION_CROSS_VARIANT_MSG.to_string(),
            ));
        }
        match self {
            CsrVariant::Multiple(csr) => {
                csr.delete_edge_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Single(csr) => csr.delete_edge_at_position(src_vid, position, expected, ts),
            CsrVariant::Pure(csr) => csr.delete_edge_at_position(src_vid, position, expected, ts),
            CsrVariant::Bundled(csr) => {
                csr.delete_edge_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Frozen(_) | CsrVariant::None { .. } => {
                unreachable!("capability gate handles non-positional forms above")
            }
        }
    }

    fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        if !self.supports_positions() {
            debug_assert!(false, "{}", ROW_POSITION_CROSS_VARIANT_MSG);
            return false;
        }
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
            CsrVariant::Frozen(_) | CsrVariant::None { .. } => {
                unreachable!("capability gate handles non-positional forms above")
            }
        }
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        // Result-returning delete like `delete_edge`: placeholder and
        // read-only forms are refused, with frozen keeping its own rejection
        // wording through forwarding.
        if self.is_empty_placeholder() {
            return Err(StorageError::invalid_operation(
                NO_EDGES_STORED_MSG.to_string(),
            ));
        }
        if self.is_read_only() {
            return match self {
                CsrVariant::Frozen(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
                _ => Err(StorageError::invalid_operation(
                    NO_EDGES_STORED_MSG.to_string(),
                )),
            };
        }
        match self {
            CsrVariant::Multiple(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Single(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Pure(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Bundled(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Frozen(_) | CsrVariant::None { .. } => {
                unreachable!("capability gate handles non-writable forms above")
            }
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
        CsrVariant::fill_physical_into(self, src_vid, out)
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.has_physical_entries(vid),
            CsrVariant::Single(csr) => csr.has_physical_entries(vid),
            CsrVariant::Pure(csr) => csr.has_physical_entries(vid),
            CsrVariant::Bundled(csr) => csr.has_physical_entries(vid),
            CsrVariant::Frozen(csr) => csr.has_physical_entries(vid),
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
        // Single exemption source lives with the capability query: read-only
        // and placeholder forms report zero here and reclaim through the
        // batched frozen entry or a file rebuild instead.
        if !self.supports_vertex_compact() {
            return 0;
        }
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
            CsrVariant::Frozen(_) | CsrVariant::None { .. } => 0,
        }
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        // Per-variant probe stays unsliced: pure and bundled report hole
        // counts, frozen reports timestamp-eligible tombstones. The capability
        // query only gates the reporting compact entry.
        match self {
            CsrVariant::Multiple(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Single(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Pure(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Bundled(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Frozen(csr) => csr.reclaimable_count(vid, cutoff),
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
            CsrVariant::None { .. } => (0, 0),
        }
    }

    fn row_gap(&self, vid: u32) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.row_gap(vid),
            CsrVariant::Pure(csr) => csr.row_gap(vid),
            CsrVariant::Bundled(csr) => csr.row_gap(vid),
            CsrVariant::Single(_) | CsrVariant::Frozen(_) | CsrVariant::None { .. } => 0,
        }
    }

    fn row_density(&self, vid: u32) -> f32 {
        match self {
            CsrVariant::Multiple(csr) => csr.row_density(vid),
            CsrVariant::Pure(csr) => csr.row_density(vid),
            CsrVariant::Bundled(csr) => csr.row_density(vid),
            CsrVariant::Single(_) | CsrVariant::Frozen(_) | CsrVariant::None { .. } => 1.0,
        }
    }

    fn rebalance_row(&mut self, vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.rebalance_row(vid),
            CsrVariant::Pure(csr) => csr.rebalance_row(vid),
            CsrVariant::Bundled(csr) => csr.rebalance_row(vid),
            CsrVariant::Single(_) | CsrVariant::Frozen(_) | CsrVariant::None { .. } => true,
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
        }
    }
}

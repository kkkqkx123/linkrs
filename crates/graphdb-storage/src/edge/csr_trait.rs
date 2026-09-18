//! CSR Trait Definitions
//!
//! Unified trait interface for different CSR implementations.
//! Supports runtime polymorphism for edge storage selection.

use graphdb_core::StorageResult;

use super::{EdgeId, Nbr, Timestamp, VertexId};

pub trait CsrBase: std::fmt::Debug + Send + Sync {
    fn vertex_capacity(&self) -> usize;

    fn edge_count(&self) -> u64;

    fn dump(&self) -> Vec<u8>;

    fn load(&mut self, data: &[u8]) -> StorageResult<()>;

    /// Append the same bytes as `dump` into `out` without an intermediate
    /// owned buffer, so checkpoint writes borrow the live topology instead
    /// of cloning it for serialization.
    fn dump_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.dump());
    }
}

pub trait MutableCsrTrait: CsrBase {
    /// Insert an edge.
    ///
    /// Topology and properties are decoupled: the CSR stores only the
    /// topology (neighbor, edge_id, timestamps). Properties are stored
    /// separately indexed by `EdgeId`.
    ///
    /// Physical uniqueness only, without timestamp ordering checks. Snapshot
    /// visibility is decided by the version authority above this layer.
    ///
    /// Returns `Ok(())` on success, or an error explaining why insertion failed:
    ///
    /// - `MutableCsr`: checks for duplicate live (neighbor) keys across primary and overflow,
    ///   writes to primary if space available, otherwise spills to overflow with auto-expansion.
    ///   Returns `EdgeAlreadyExists` on duplicate.
    /// - `SingleMutableCsr`: rejects a second live edge in an occupied slot
    ///   with `Conflict`, matching the table-layer Single contract. A
    ///   tombstoned slot accepts a rebuild at any timestamp. No silent
    ///   overwrite exists.
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()>;

    /// Delete an edge by edge_id.
    ///
    /// Returns:
    /// - `Ok(true)` when the edge was deleted.
    /// - `Ok(false)` when the edge does not exist or cannot be deleted at `ts`
    ///   (e.g. `create_ts > ts`), or when it was already deleted at exactly
    ///   `ts` (idempotent re-delete).
    /// - `Err(StorageError::write_write_conflict)` when the edge already exists
    ///   but was deleted at a **different** timestamp.
    ///
    /// - `MutableCsr`: uses `edge_id` to locate and delete the specific edge.
    /// - `SingleMutableCsr`: the stored edge id must match exactly. No wildcard
    ///   edge id is supported; callers pass the precise id or use endpoint
    ///   addressing.
    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool>;

    /// Delete all edges matching (src, dst) with full-match semantics.
    ///
    /// One call deletes every live match; no first-only variant exists.
    /// Returns the deleted count so table rollback can reconcile by count and
    /// tombstone metrics can observe multi-match rows.
    ///
    /// - `MutableCsr`: scans primary and overflow, deletes **all** matching edges.
    /// - `SingleMutableCsr`: deletes the single edge if dst matches (0 or 1).
    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize;

    /// Delete all edges matching (src, dst), reporting every stamped id.
    ///
    /// Same single-pass semantics as `delete_edge_by_dst`, plus one
    /// `on_deleted` call per stamped edge so callers needing the ids (append
    /// logs, audits) skip their own collection scan. The default routes
    /// through `delete_edge_by_dst` and reports nothing; row stores override
    /// it with an in-place reporting pass.
    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        _on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        self.delete_edge_by_dst(src_vid, dst, ts)
    }

    /// Delete an edge by its offset position in the primary block.
    ///
    /// Offset indexes into the live row degree, never into reserved capacity.
    /// Out-of-degree offsets fail without touching any slot. Conflict errors
    /// are propagated instead of folded into a failure value.
    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool>;

    /// Revert a deleted edge by its offset position.
    ///
    /// - `MutableCsr`: offset indexes into the primary block degree.
    /// - `SingleMutableCsr`: only offset == 0 is valid.
    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool;

    /// Read-only view of one primary slot for offset identity checks.
    fn nbr_at_offset(&self, _src_vid: u32, _offset: i32) -> Option<Nbr> {
        None
    }

    /// Locate the first live edge by endpoint without consulting snapshots.
    ///
    /// Physical addressing only; tombstoned and gap slots are skipped.
    /// Snapshot visibility still needs the version authority above this
    /// layer, while tombstone access uses the full physical views.
    fn get_edge_physical(&self, _src_vid: u32, _dst: VertexId) -> Option<Nbr> {
        None
    }

    /// Every physically stored entry of one vertex without timestamp filtering.
    fn physical_edges_of(&self, _src_vid: u32) -> Vec<Nbr> {
        Vec::new()
    }

    /// Fill a caller buffer with every physically stored entry of one vertex.
    ///
    /// Same content as the allocating accessor above, without the per-vertex
    /// allocation. The default forwards to the allocating accessor; row
    /// stores override it with a direct fill.
    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        out.clear();
        out.extend_from_slice(&self.physical_edges_of(src_vid));
    }

    /// Whether one vertex holds any physically stored entry.
    fn has_physical_entries(&self, _vid: u32) -> bool {
        false
    }

    /// Whether the primary row of one vertex holds `edge_id`.
    fn primary_contains(&self, _src_vid: u32, _edge_id: EdgeId) -> bool {
        false
    }

    /// Physically remove an edge by edge id (no tombstone trace).
    ///
    /// Reclaims the slot and updates the edge count. Only implemented by
    /// `MutableCsr`; other strategies default to no-op.
    fn remove_edge(&mut self, _src_vid: u32, _edge_id: EdgeId) -> bool {
        false
    }

    /// Revert a deletion by edge id if it happened at or before `ts`.
    ///
    /// Restores the entry as live. Only implemented by `MutableCsr`; other
    /// strategies default to no-op.
    fn revert_delete_by_edge_id(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        false
    }

    /// Get a specific edge by source and destination.
    /// Test-only row-stamp filter; production reads go through the version authority.
    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr>;

    /// Get all valid edges of a vertex at the given timestamp.
    /// Test-only row-stamp filter; production reads go through the version authority.
    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr>;

    /// Reclaim one vertex in place, dropping entries eligible for collection
    /// at `cutoff` and tightening live entries without touching other rows.
    ///
    /// Returns the number of removed entries. Reported removals flow through
    /// `on_edge_removed` so tombstone promotion stays centralized.
    /// Strategies without per-row fragmentation default to no-op.
    fn compact_vertex_with_reporting(
        &mut self,
        _vid: u32,
        _cutoff: Timestamp,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        0
    }

    /// Count entries of one vertex that are reclaimable at `cutoff`.
    ///
    /// The write-path trigger consults this per-vertex count instead of a
    /// whole-table ratio, so small writes never cause large rebuilds.
    fn reclaimable_count(&self, _vid: u32, _cutoff: Timestamp) -> usize {
        0
    }

    /// Whether one vertex holds anything reclaimable at `cutoff`.
    fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        self.reclaimable_count(vid, cutoff) > 0
    }

    /// Physical entry census of one vertex: `(live, dead, capacity)`.
    ///
    /// Backs the per-vertex fragmentation view; strategies without rows
    /// report zeros.
    fn vertex_census(&self, _vid: u32) -> (usize, usize, usize) {
        (0, 0, 0)
    }

    /// Single-walk reclaim probe of one vertex: `(dead, reclaimable)`.
    ///
    /// Fuses the census and eligibility walks so maintenance passes pay one
    /// row scan instead of two. The default derives both from the separate
    /// primitives; row stores override it with one fused walk.
    fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        let (_, dead, _) = self.vertex_census(vid);
        (dead, self.reclaimable_count(vid, cutoff))
    }

    /// Reserved primary slots minus live primary entries of one row.
    ///
    /// Steady-state write gaps; strategies without rows report zero.
    fn row_gap(&self, _vid: u32) -> usize {
        0
    }

    /// Live primary entries per unit of reserved primary capacity.
    ///
    /// Strategies without rows report full density.
    fn row_density(&self, _vid: u32) -> f32 {
        1.0
    }

    /// Rebalance one row in place, pulling overflow entries into primary
    /// gaps and repacking leftover overflow. Returns true when the row no
    /// longer holds overflow. Strategies without rows report true.
    fn rebalance_row(&mut self, _vid: u32) -> bool {
        true
    }

    /// Return the approximate memory usage in bytes.
    fn used_memory_size(&self) -> usize;
}

#[cfg(test)]
mod tests {}

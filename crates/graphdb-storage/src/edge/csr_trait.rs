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
}

pub trait MutableCsrTrait: CsrBase {
    /// Insert an edge.
    ///
    /// Topology and properties are decoupled: the CSR stores only the
    /// topology (neighbor, edge_id, timestamps). Properties are stored
    /// separately indexed by `EdgeId`.
    ///
    /// Returns `Ok(())` on success, or an error explaining why insertion failed:
    ///
    /// - `MutableCsr`: checks for duplicate (neighbor + valid timestamp) across primary and overflow,
    ///   writes to primary if space available, otherwise spills to overflow with auto-expansion.
    ///   Returns `EdgeAlreadyExists` on duplicate.
    /// - `SingleMutableCsr`: rejects a second live edge in an occupied slot
    ///   with `Conflict`, matching the table-layer Single contract. No silent
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

    /// Locate one edge by endpoint without consulting timestamps.
    ///
    /// Physical addressing only; visibility is decided by the version
    /// authority above this layer.
    fn get_edge_physical(&self, _src_vid: u32, _dst: VertexId) -> Option<Nbr> {
        None
    }

    /// Every physically stored entry of one vertex without timestamp filtering.
    fn physical_edges_of(&self, _src_vid: u32) -> Vec<Nbr> {
        Vec::new()
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

    /// Compact with timestamp threshold and reserve ratio.
    ///
    /// Returns the number of removed edges.
    /// Test-only direct row-stamp filter; production visibility goes through
    /// the version authority plus `compact_with_ts_reporting`.
    fn compact_with_ts(&mut self, _ts: Timestamp, _reserve_ratio: f32) -> usize {
        0
    }

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

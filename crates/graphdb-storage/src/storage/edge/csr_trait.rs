//! CSR Trait Definitions
//!
//! Unified trait interface for different CSR implementations.
//! Supports runtime polymorphism for edge storage selection.

use crate::core::StorageResult;

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
    /// Returns `Ok(())` on success, or an error explaining why insertion failed:
    ///
    /// - `MutableCsr`: checks for duplicate (neighbor + valid timestamp) across primary and overflow,
    ///   writes to primary if space available, otherwise spills to overflow with auto-expansion.
    ///   Returns `EdgeAlreadyExists` on duplicate.
    /// - `SingleMutableCsr`: overwrites based on timestamp ordering (only if new ts > existing ts).
    ///   Returns `Conflict` on timestamp conflict.
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        ts: Timestamp,
    ) -> StorageResult<()>;

    /// Delete an edge by edge_id.
    ///
    /// - `MutableCsr`: uses `edge_id` to locate and delete the specific edge.
    /// - `SingleMutableCsr`: `edge_id` is **ignored** since there is only one edge per vertex.
    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool;

    /// Delete all edges matching (src, dst).
    ///
    /// - `MutableCsr`: scans primary and overflow, deletes **all** matching edges.
    /// - `SingleMutableCsr`: deletes the single edge if dst matches.
    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> bool;

    /// Delete an edge by its offset position in the primary block.
    fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool;

    /// Revert a deleted edge by its offset position.
    ///
    /// - `MutableCsr`: offset indexes into the primary block.
    /// - `SingleMutableCsr`: only offset == 0 is valid.
    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool;

    /// Get a specific edge by source and destination.
    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr>;

    /// Get all valid edges of a vertex at the given timestamp.
    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr>;

    /// Compact with timestamp threshold and reserve ratio.
    ///
    /// Returns the number of removed edges.
    ///
    /// - `MutableCsr`: removes edges with timestamp > `ts`, reserves `reserve_ratio` free space.
    /// - `SingleMutableCsr`: no-op, returns 0.
    fn compact_with_ts(&mut self, _ts: Timestamp, _reserve_ratio: f32) -> usize {
        0
    }

    /// Return the approximate memory usage in bytes.
    fn used_memory_size(&self) -> usize;
}

#[cfg(test)]
mod tests {}

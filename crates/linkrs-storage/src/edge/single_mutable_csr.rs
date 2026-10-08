//! Single-edge mutable CSR (the `Single` variant).
//!
//! Each vertex holds at most one live edge in a direct slot array, giving
//! O(1) addressing. Serves one-to-one relationships (spouse, current
//! employer); a second live insert into an occupied slot is rejected with a
//! conflict error, never silently overwritten.
//!
//! Sparse cost: logical slots stay dense for O(1) addressing, but physical
//! segments allocate lazily behind the `present` bitmap. Untouched sparse
//! slots read as absent without allocating, written slots allocate their
//! 1024-row segment on demand, and emptied segments release back to `None`.
//! Sparse wide spans therefore pay only for materialized segments plus the
//! bitmap, never for the full logical span. The fixed-slot addressing is
//! kept and documented rather than replaced.
//!
//! Contract, unified with the table layer:
//! - Each vertex holds at most one live edge. A second live insert into an
//!   occupied slot is rejected with a conflict error, never silently
//!   overwritten. Callers must delete before rebuilding.
//! - Deletion requires the exact edge id; endpoint plus rank addressing goes
//!   through `delete_edge_by_dst`. No wildcard edge id is supported.
//! - `delete_edge_by_dst` deletes the single matching live entry and reports
//!   the deleted count (0 or 1) so callers can reconcile.
//! - Rebuilding a tombstoned slot needs no timestamp ordering at this layer;
//!   snapshot visibility is decided by the version authority above.
//!
//! If concurrent writes are needed, use MutableCsr (accepts multiple edges).
//!
//! Layout by responsibility (`single_mutable_csr/` subdirectory):
//! - `write` implements insert, delete, revert and rollback.
//! - `read` implements physical reads, threshold scans and row iterators.
//! - `maintenance` handles reclaim probes and compaction.
//! - `persistence` handles dump and load.
//! - `trait_impl` adapts the type to CSR traits.
//! - `iter` holds the full-table iterator.

use super::csr_shared::{grown_vertex_capacity, DEFAULT_VERTEX_CAPACITY};
use super::{ColdStamps, HotNbr, Nbr};

pub(crate) mod iter;
pub(crate) mod maintenance;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod trait_impl;
pub(crate) mod write;

pub use iter::SingleMutableCsrIterator;

/// Unassigned single slot: no edge id, never alive at any timestamp.
pub(super) fn empty_slot() -> Nbr {
    Nbr::dead_gap()
}

#[derive(Debug, Clone)]
pub(super) struct SingleSegment {
    pub(super) hot: [HotNbr; 1024],
    pub(super) cold: [ColdStamps; 1024],
}

impl SingleSegment {
    pub(super) fn fresh() -> Self {
        Self {
            hot: [HotNbr::dead_gap(); 1024],
            cold: [ColdStamps::dead_gap(); 1024],
        }
    }
}

pub struct SingleMutableCsr {
    pub(super) segments: Vec<Option<Box<SingleSegment>>>,
    pub(super) present: Vec<u64>,
    pub(super) vertex_capacity: usize,
    pub(super) edge_count: u64,
}

impl Clone for SingleMutableCsr {
    fn clone(&self) -> Self {
        Self {
            segments: self.segments.clone(),
            present: self.present.clone(),
            vertex_capacity: self.vertex_capacity,
            edge_count: self.edge_count,
        }
    }
}

impl std::fmt::Debug for SingleMutableCsr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleMutableCsr")
            .field("vertex_capacity", &self.vertex_capacity())
            .field("edge_count", &self.edge_count)
            .field("allocated_segments", &self.allocated_segments())
            .finish_non_exhaustive()
    }
}

impl SingleMutableCsr {
    pub(super) const SEGMENT_ROWS: usize = 1024;
    pub(super) const SEGMENT_SHIFT: u32 = 10;
    pub(super) const SEGMENT_MASK: usize = 1024 - 1;

    #[inline]
    pub(super) fn locate(vid: usize) -> (usize, usize) {
        (vid >> Self::SEGMENT_SHIFT, vid & Self::SEGMENT_MASK)
    }

    #[inline]
    pub(super) fn has_present(&self, idx: usize) -> bool {
        let word = idx / 64;
        let bit = idx % 64;
        self.present
            .get(word)
            .is_some_and(|w| w & (1u64 << bit) != 0)
    }

    #[inline]
    pub(super) fn set_present(&mut self, idx: usize, value: bool) {
        let word = idx / 64;
        let bit = idx % 64;
        if self.present.len() <= word {
            self.present.resize(word + 1, 0);
        }
        if value {
            self.present[word] |= 1u64 << bit;
        } else {
            self.present[word] &= !(1u64 << bit);
        }
    }

    pub(super) fn ensure_table_len(&mut self) {
        let need_segments = self.vertex_capacity.div_ceil(Self::SEGMENT_ROWS);
        if self.segments.len() < need_segments {
            self.segments.resize_with(need_segments, || None);
        }
        // The present bitmap stays lazy: missing words read as absent through
        // `has_present`, and `set_present` grows on demand. Pre-sizing it
        // here would charge every sparse wide span for its full logical width.
    }

    pub(crate) fn allocated_segments(&self) -> usize {
        self.segments.iter().filter(|seg| seg.is_some()).count()
    }

    pub(crate) fn sparse_memory_bytes(&self) -> usize {
        self.segments.capacity() * std::mem::size_of::<Option<Box<SingleSegment>>>()
            + self.allocated_segments()
                * Self::SEGMENT_ROWS
                * (std::mem::size_of::<HotNbr>() + std::mem::size_of::<ColdStamps>())
            + self.present.capacity() * std::mem::size_of::<u64>()
    }

    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY)
    }

    pub fn with_capacity(vertex_capacity: usize) -> Self {
        let vertex_cap = vertex_capacity.max(1);
        let mut csr = Self {
            segments: Vec::new(),
            present: Vec::new(),
            vertex_capacity: vertex_cap,
            edge_count: 0,
        };
        csr.ensure_table_len();
        csr
    }

    pub fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity {
            return;
        }
        self.vertex_capacity = new_vertex_capacity;
        self.ensure_table_len();
    }

    pub fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity {
            self.resize(grown_vertex_capacity(min_capacity));
        }
    }

    /// Assembled slot copy at one index. Untouched sparse slots read as
    /// absent, matching the empty-slot miss path without allocating.
    pub(super) fn slot_at(&self, idx: usize) -> Option<Nbr> {
        if idx >= self.vertex_capacity {
            return None;
        }
        if !self.has_present(idx) {
            return None;
        }
        let (seg, off) = Self::locate(idx);
        let segment = self.segments.get(seg)?.as_ref()?;
        let probe = Nbr::from_parts(segment.hot[off], segment.cold[off]);
        (probe.edge_id != INVALID_EDGE_ID).then_some(probe)
    }

    pub(super) fn hot_at(&self, idx: usize) -> Option<HotNbr> {
        if idx >= self.vertex_capacity || !self.has_present(idx) {
            return None;
        }
        let (seg, off) = Self::locate(idx);
        self.segments.get(seg)?.as_ref().map(|s| s.hot[off])
    }

    pub(super) fn cold_at(&self, idx: usize) -> Option<ColdStamps> {
        if idx >= self.vertex_capacity || !self.has_present(idx) {
            return None;
        }
        let (seg, off) = Self::locate(idx);
        self.segments.get(seg)?.as_ref().map(|s| s.cold[off])
    }

    /// Paired writer for one slot: hot and cold stay in lockstep.
    pub(super) fn set_slot(&mut self, idx: usize, nbr: Nbr) {
        let (seg, off) = Self::locate(idx);
        if seg >= self.segments.len() {
            self.segments.resize_with(seg + 1, || None);
        }
        let is_absent = nbr.edge_id == INVALID_EDGE_ID;
        {
            let segment =
                self.segments[seg].get_or_insert_with(|| Box::new(SingleSegment::fresh()));
            segment.hot[off] = nbr.hot();
            segment.cold[off] = nbr.cold();
        }
        self.set_present(idx, !is_absent);
        if is_absent && self.segment_is_empty(seg) {
            self.segments[seg] = None;
        }
    }

    pub(super) fn segment_is_empty(&self, seg: usize) -> bool {
        self.segments
            .get(seg)
            .and_then(|s| s.as_ref())
            .is_some_and(|s| s.hot.iter().all(|h| h.edge_id == INVALID_EDGE_ID))
    }

    pub(super) fn slot_mut_or_alloc(&mut self, idx: usize) -> &mut SingleSegment {
        let (seg, _) = Self::locate(idx);
        if seg >= self.segments.len() {
            self.segments.resize_with(seg + 1, || None);
        }
        self.segments[seg].get_or_insert_with(|| Box::new(SingleSegment::fresh()))
    }

    pub(super) fn stamp_delete(&mut self, idx: usize, ts: Timestamp) {
        let (seg, off) = Self::locate(idx);
        if let Some(segment) = self.segments.get_mut(seg).and_then(|s| s.as_mut()) {
            segment.cold[off].delete_ts = ts;
        }
    }

    pub(super) fn clear_delete(&mut self, idx: usize) {
        let (seg, off) = Self::locate(idx);
        if let Some(segment) = self.segments.get_mut(seg).and_then(|s| s.as_mut()) {
            segment.cold[off].delete_ts = Timestamp::MAX;
        }
    }

    pub(super) fn cold_is_live_at(&self, idx: usize) -> bool {
        self.cold_at(idx).is_some_and(|cold| cold.is_live())
    }

    pub(super) fn get_edge_any_dst(&self, src: u32, ts: Timestamp) -> Option<Nbr> {
        let probe = self.slot_at(src as usize)?;

        if probe.is_alive_at(ts) {
            Some(probe)
        } else {
            None
        }
    }
}

impl Default for SingleMutableCsr {
    fn default() -> Self {
        Self::new()
    }
}

use super::{Timestamp, INVALID_EDGE_ID};

#[cfg(test)]
mod tests;

use super::super::csr_shared::{grown_vertex_capacity, VertexBookkeeping, DEFAULT_VERTEX_CAPACITY};
use super::super::{ColdStamps, HotNbr, Timestamp};
use super::live_set::LiveSetStorage;
use super::overflow::{OverflowChunk, OverflowStorage};
use super::MutableCsr;

/// Default reservations for a fresh CSR: 4096 edge slots, 4-slot initial
/// primary blocks for new vertices, and 4096-edge overflow chunks matching
/// `OVERFLOW_CHUNK_MAX` so one chunk never exceeds one allocation unit.
/// These are construction-time capacities only, not tuning knobs: growth is
/// driven by the graded overflow scheme and the packed density target.
/// Changing the defaults requires a bulk-load memory benchmark first.
pub(crate) const DEFAULT_EDGE_CAPACITY: usize = 4096;
pub(crate) const DEFAULT_VERTEX_DEGREE: usize = 4;
pub(crate) const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;
/// Sentinel for an unknown reuse hint: no known reclaimable slot.
pub(crate) const REUSE_HINT_UNKNOWN: u32 = u32::MAX;

impl MutableCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY, DEFAULT_EDGE_CAPACITY)
    }

    pub fn with_capacity(vertex_capacity: usize, edge_capacity: usize) -> Self {
        Self::with_overflow_chunk_edges(
            vertex_capacity,
            edge_capacity,
            DEFAULT_OVERFLOW_CHUNK_EDGES,
        )
    }

    pub fn with_overflow_chunk_edges(
        vertex_capacity: usize,
        edge_capacity: usize,
        overflow_chunk_edges: usize,
    ) -> Self {
        let vertex_cap = vertex_capacity.max(1);
        let edge_cap = edge_capacity.max(1);

        Self {
            hot_list: Vec::with_capacity(edge_cap),
            cold_list: Vec::with_capacity(edge_cap),
            rows: VertexBookkeeping::with_capacity(vertex_cap),
            overflow_chunks: OverflowStorage::new(),
            overflow_chunk_edges: overflow_chunk_edges.max(1),
            live_sets: LiveSetStorage::new(),
            tombstone_reuse_cutoff: Timestamp::MAX,
            reuse_hint: vec![REUSE_HINT_UNKNOWN; vertex_cap],
            live_counts: vec![0; vertex_cap],
            tombstone_counts: vec![0; vertex_cap],
            primary_sorted: vec![true; vertex_cap],
            edge_count: 0,
            total_edge_capacity: 0,
            overflow_chunk_allocs: 0,
            primary_block_allocs: 0,
            repack_count: 0,
            tombstone_reuse_count: 0,
            live_set_rebuild_count: 0,
        }
    }

    /// Watermark-derived cutoff gating hot-path tombstone reuse. The cutoff
    /// carries exactly one freshness contract, shared by every caller:
    ///
    /// - Only a global-watermark capture (`MvccWatermarks::capture` plus
    ///   margin) may refresh it. The table-local pin cache is never a source:
    ///   it can sit above the global safe point and would widen reuse past
    ///   what reclaim has proven.
    /// - The sentinel `Timestamp::MAX` disables reuse. A maintenance pass that
    ///   cannot capture a fresh watermark must clear instead of reusing, so an
    ///   unrefreshed table degrades to the pre-reuse behavior.
    /// - This setter only narrows reuse: the stored cutoff moves to the
    ///   minimum of the old and new values, so a stale larger value can never
    ///   widen reuse. Fresh watermark advances use `refresh_from_watermark`.
    pub fn set_tombstone_reuse_cutoff(&mut self, cutoff: Timestamp) {
        self.tombstone_reuse_cutoff = self.tombstone_reuse_cutoff.min(cutoff);
    }

    /// Fresh watermark refresh of the reuse cutoff, allowing widening.
    ///
    /// Only call with a freshly captured global watermark bound. Stale values
    /// must go through `set_tombstone_reuse_cutoff` so they can only narrow.
    pub fn refresh_tombstone_reuse_cutoff(&mut self, fresh: Timestamp) {
        self.tombstone_reuse_cutoff = fresh;
    }

    /// Drop the reuse hint back to the disabled sentinel.
    ///
    /// Stale path of the freshness contract above: when no fresh watermark is
    /// available, reuse stops entirely instead of running on an expired bound.
    pub fn clear_tombstone_reuse_cutoff(&mut self) {
        self.tombstone_reuse_cutoff = Timestamp::MAX;
    }

    /// Current reuse cutoff for observability and tests. `Timestamp::MAX`
    /// means reuse is disabled.
    pub fn tombstone_reuse_cutoff(&self) -> Timestamp {
        self.tombstone_reuse_cutoff
    }

    pub fn vertex_capacity(&self) -> usize {
        self.rows.adj_offsets.len()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    /// Resize vertex capacity (requires exclusive access)
    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity() {
            return;
        }

        let tail = self.hot_list.len() as u32;
        self.rows.resize(new_vertex_capacity, tail);
        self.reuse_hint
            .resize(new_vertex_capacity, REUSE_HINT_UNKNOWN);
        self.live_counts.resize(new_vertex_capacity, 0);
        self.tombstone_counts.resize(new_vertex_capacity, 0);
        self.primary_sorted.resize(new_vertex_capacity, true);
        self.overflow_chunks.ensure_capacity(new_vertex_capacity);
        self.live_sets.ensure_capacity(new_vertex_capacity);
    }

    /// Ensure vertex capacity (grows if needed)
    pub fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity() {
            self.resize(grown_vertex_capacity(min_capacity));
        }
    }

    /// Get overflow chunks for a vertex.
    pub fn get_overflow_chunks(&self, vid: u32) -> Option<&Vec<OverflowChunk>> {
        self.overflow_chunks.get(vid)
    }

    /// Allocate the primary block of `DEFAULT_VERTEX_DEGREE` slots for a vertex
    /// on its first edge. Zero-degree vertices hold no slots in either half.
    pub(crate) fn allocate_primary_block(&mut self, src_idx: usize) {
        let block_offset = self.hot_list.len();
        self.hot_list
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, HotNbr::dead_gap());
        self.cold_list
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, ColdStamps::dead_gap());
        self.rows
            .assign_primary_block(src_idx, block_offset as u32, DEFAULT_VERTEX_DEGREE as u32);
        self.add_capacity(DEFAULT_VERTEX_DEGREE);
        self.primary_block_allocs += 1;
    }

    /// Single entry point for capacity growth. Every branch that reserves
    /// slots routes through here so the ledger cannot drift from actual
    /// allocation.
    pub(crate) fn add_capacity(&mut self, slots: usize) {
        self.total_edge_capacity = self.total_edge_capacity.saturating_add(slots);
    }

    /// Single entry point for capacity release. Every branch that frees slots
    /// routes through here; whole-table rebuilds rebase the ledger instead
    /// (see `compact_with_ts_reporting` and `clear`).
    pub(crate) fn sub_capacity(&mut self, slots: usize) {
        self.total_edge_capacity = self.total_edge_capacity.saturating_sub(slots);
    }

    /// Clear all edges
    pub fn clear(&mut self) {
        self.rows.degrees.fill(0);
        self.overflow_chunks.clear();
        self.live_sets.clear();
        self.reuse_hint.fill(REUSE_HINT_UNKNOWN);
        self.live_counts.fill(0);
        self.tombstone_counts.fill(0);
        self.primary_sorted.fill(true);
        self.total_edge_capacity = self
            .rows
            .primary_capacities
            .iter()
            .map(|cap| *cap as usize)
            .sum();
        self.edge_count = 0;
        self.reset_baseline_counters();
    }

    /// Record a freshly tombstoned primary slot as the reuse candidate.
    pub(crate) fn note_primary_tombstone(&mut self, src_idx: usize, slot: usize) {
        if let Some(hint) = self.reuse_hint.get_mut(src_idx) {
            let slot = slot as u32;
            if slot < *hint {
                *hint = slot;
            }
        }
    }

    /// Drop the reuse hint after any row move or slot reuse.
    pub(crate) fn invalidate_reuse_hint(&mut self, src_idx: usize) {
        if let Some(hint) = self.reuse_hint.get_mut(src_idx) {
            *hint = REUSE_HINT_UNKNOWN;
        }
    }

    /// Reset every reuse hint after a whole-table rebuild.
    pub(crate) fn reset_reuse_hints(&mut self) {
        self.reuse_hint.fill(REUSE_HINT_UNKNOWN);
    }

    /// Cached primary order of one row for threshold scans.
    ///
    /// True only when the primary window is known to arrive in key order;
    /// false always falls back to the linear scan, so a stale false costs
    /// speed but never correctness.
    pub(crate) fn primary_sorted_flag(&self, src_idx: usize) -> bool {
        self.primary_sorted[src_idx]
    }

    /// Clear the cached order after a primary write of a new key.
    ///
    /// Deletes, reverts and order-preserving moves must not call this:
    /// they keep the key order intact.
    pub(crate) fn mark_primary_unsorted(&mut self, src_idx: usize) {
        if let Some(flag) = self.primary_sorted.get_mut(src_idx) {
            *flag = false;
        }
    }

    /// Establish the cached order after the maintenance sort.
    pub(crate) fn mark_primary_sorted(&mut self, src_idx: usize) {
        if let Some(flag) = self.primary_sorted.get_mut(src_idx) {
            *flag = true;
        }
    }

    /// Drop every cached order after a rebuild with unknown row order.
    pub(crate) fn reset_primary_sorted(&mut self) {
        self.primary_sorted.fill(false);
    }
}

impl Default for MutableCsr {
    fn default() -> Self {
        Self::new()
    }
}

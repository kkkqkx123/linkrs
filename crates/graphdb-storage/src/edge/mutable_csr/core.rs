use super::super::csr_shared::{grown_vertex_capacity, DEFAULT_VERTEX_CAPACITY};
use super::super::{ColdStamps, HotNbr, Timestamp};
use super::live_set::LiveSetStorage;
use super::overflow::{OverflowChunk, OverflowStorage};
use super::MutableCsr;

pub(crate) const DEFAULT_EDGE_CAPACITY: usize = 4096;
pub(crate) const DEFAULT_VERTEX_DEGREE: usize = 4;
pub(crate) const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;

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
            adj_offsets: vec![0; vertex_cap],
            degrees: vec![0; vertex_cap],
            primary_capacities: vec![0; vertex_cap],
            overflow_chunks: OverflowStorage::new(),
            overflow_chunk_edges: overflow_chunk_edges.max(1),
            live_sets: LiveSetStorage::new(),
            tombstone_reuse_cutoff: Timestamp::MAX,
            edge_count: 0,
            total_edge_capacity: 0,
        }
    }

    /// Watermark-derived cutoff gating hot-path tombstone reuse. The table
    /// maintenance pass refreshes it from its watermark capture; the
    /// sentinel disables reuse. A stale value only narrows reuse, never
    /// widens it, so a missed refresh degrades to the pre-reuse behavior.
    pub fn set_tombstone_reuse_cutoff(&mut self, cutoff: Timestamp) {
        self.tombstone_reuse_cutoff = cutoff;
    }

    pub fn vertex_capacity(&self) -> usize {
        self.adj_offsets.len()
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
        self.adj_offsets.resize(new_vertex_capacity, tail);
        self.degrees.resize(new_vertex_capacity, 0);
        self.primary_capacities.resize(new_vertex_capacity, 0);
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
        self.overflow_chunks.get(&vid)
    }

    /// Allocate the primary block of `DEFAULT_VERTEX_DEGREE` slots for a vertex
    /// on its first edge. Zero-degree vertices hold no slots in either half.
    pub(crate) fn allocate_primary_block(&mut self, src_idx: usize) {
        let block_offset = self.hot_list.len();
        self.hot_list
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, HotNbr::dead_gap());
        self.cold_list
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, ColdStamps::dead_gap());
        self.adj_offsets[src_idx] = block_offset as u32;
        self.primary_capacities[src_idx] = DEFAULT_VERTEX_DEGREE as u32;
        self.add_capacity(DEFAULT_VERTEX_DEGREE);
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
        self.degrees.fill(0);
        self.overflow_chunks.clear();
        self.live_sets.clear();
        self.total_edge_capacity = self
            .primary_capacities
            .iter()
            .map(|cap| *cap as usize)
            .sum();
        self.edge_count = 0;
    }
}

impl Default for MutableCsr {
    fn default() -> Self {
        Self::new()
    }
}

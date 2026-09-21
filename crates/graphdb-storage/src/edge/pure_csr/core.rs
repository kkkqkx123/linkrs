use graphdb_core::types::{EdgeId, Timestamp};

use super::super::csr_shared::{grown_vertex_capacity, VertexBookkeeping, DEFAULT_VERTEX_CAPACITY};
use super::super::{FragmentationStats, Nbr};
use super::live_set::PureLiveSetStorage;
use super::overflow::PureOverflowStorage;
use super::{
    PureTopologyCsr, DEFAULT_OVERFLOW_CHUNK_EDGES, DEFAULT_VERTEX_DEGREE, INVALID_EDGE_ID,
};

impl PureTopologyCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY, DEFAULT_VERTEX_DEGREE)
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
            rows: VertexBookkeeping::with_capacity(vertex_cap),
            endpoints: Vec::with_capacity(edge_cap),
            edge_ids: Vec::with_capacity(edge_cap),
            overflow_chunks: PureOverflowStorage::new(),
            overflow_chunk_edges: overflow_chunk_edges.max(1),
            live_sets: PureLiveSetStorage::new(),
            edge_count: 0,
            total_edge_capacity: 0,
            primary_sorted: vec![true; vertex_cap],
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.rows.len()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    pub(crate) fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity() {
            return;
        }
        let tail = self.endpoints.len() as u32;
        self.rows.resize(new_vertex_capacity, tail);
        self.primary_sorted.resize(new_vertex_capacity, true);
        self.overflow_chunks.ensure_capacity(new_vertex_capacity);
        self.live_sets.ensure_capacity(new_vertex_capacity);
    }

    pub(crate) fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity() {
            self.resize(grown_vertex_capacity(min_capacity));
        }
    }

    pub(crate) fn allocate_primary_block(&mut self, src_idx: usize) {
        let block_offset = self.endpoints.len();
        self.endpoints
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, 0);
        self.edge_ids
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, INVALID_EDGE_ID.0);
        self.rows
            .assign_primary_block(src_idx, block_offset as u32, DEFAULT_VERTEX_DEGREE as u32);
        self.add_capacity(DEFAULT_VERTEX_DEGREE);
    }

    pub(crate) fn add_capacity(&mut self, slots: usize) {
        self.total_edge_capacity = self.total_edge_capacity.saturating_add(slots);
    }

    pub(crate) fn sub_capacity(&mut self, slots: usize) {
        self.total_edge_capacity = self.total_edge_capacity.saturating_sub(slots);
    }

    pub(crate) fn primary_window(&self, src_idx: usize) -> (usize, usize) {
        if src_idx >= self.vertex_capacity() {
            return (0, 0);
        }
        let col_len = self.endpoints.len().min(self.edge_ids.len());
        self.rows.primary_window(src_idx, col_len)
    }

    pub(crate) fn make_nbr(&self, endpoint: u32, edge_id: EdgeId) -> Nbr {
        Nbr {
            endpoint,
            rank: 0,
            edge_id,
            delete_ts: Timestamp::MAX,
        }
    }

    pub fn clear(&mut self) {
        self.rows.degrees.fill(0);
        self.endpoints.clear();
        self.edge_ids.clear();
        self.overflow_chunks.clear();
        self.live_sets.clear();
        self.primary_sorted.fill(true);
        self.total_edge_capacity = self
            .rows
            .primary_capacities
            .iter()
            .map(|cap| *cap as usize)
            .sum();
        self.edge_count = 0;
    }

    pub fn fragmentation_ratio(&self) -> f32 {
        if self.total_edge_capacity == 0 {
            return 0.0;
        }
        self.total_edge_capacity
            .saturating_sub(self.edge_count as usize) as f32
            / self.total_edge_capacity as f32
    }

    pub(crate) fn wasted_bytes_estimate(&self) -> usize {
        const SLOT_BYTES: usize = 4 + 8;
        self.total_edge_capacity
            .saturating_sub(self.edge_count as usize)
            * SLOT_BYTES
    }

    pub fn get_fragmentation_stats(&self) -> FragmentationStats {
        let live_edges = self.edge_count as usize;
        let mut physical_entries = 0usize;
        for vid in 0..self.vertex_capacity() {
            physical_entries += self.rows.degrees[vid] as usize;
        }
        for (_, chunks) in self.overflow_chunks.iter() {
            for chunk in chunks {
                physical_entries += chunk.len();
            }
        }
        let dead_entries = physical_entries.saturating_sub(live_edges);
        let wasted_capacity = self.total_edge_capacity.saturating_sub(live_edges);
        FragmentationStats::with_dead_info(
            self.total_edge_capacity,
            live_edges,
            dead_entries,
            wasted_capacity,
        )
    }
}

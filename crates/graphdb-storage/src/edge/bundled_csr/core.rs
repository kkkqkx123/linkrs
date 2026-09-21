use super::super::csr_shared::SegmentedTable;
use super::super::pure_csr::{PureTopologyCsr, DEFAULT_OVERFLOW_CHUNK_EDGES};
use super::super::FragmentationStats;
use super::BundledCsr;
use bitvec::vec::BitVec;

impl BundledCsr {
    pub fn new() -> Self {
        Self::with_capacity(1024, 4096)
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
        Self {
            topology: PureTopologyCsr::with_overflow_chunk_edges(
                vertex_capacity,
                edge_capacity,
                overflow_chunk_edges,
            ),
            primary_values: Vec::with_capacity(edge_capacity),
            primary_valid: BitVec::with_capacity(edge_capacity),
            overflow_values: SegmentedTable::new(),
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.topology.vertex_capacity()
    }

    pub fn edge_count(&self) -> u64 {
        self.topology.edge_count()
    }

    pub fn clear(&mut self) {
        self.topology.clear();
        self.primary_values.clear();
        self.primary_valid.clear();
        self.overflow_values.clear();
    }

    pub fn fragmentation_ratio(&self) -> f32 {
        self.topology.fragmentation_ratio()
    }

    pub(crate) fn wasted_bytes_estimate(&self) -> usize {
        const SLOT_BYTES: usize = 4 + 8 + 8;
        self.topology
            .total_edge_capacity
            .saturating_sub(self.topology.edge_count as usize)
            * SLOT_BYTES
    }

    pub fn get_fragmentation_stats(&self) -> FragmentationStats {
        self.topology.get_fragmentation_stats()
    }
}

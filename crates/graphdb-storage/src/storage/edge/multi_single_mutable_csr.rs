//! Multi-Single Mutable CSR Implementation
//!
//! Optimized CSR for scenarios where each vertex has a small, fixed number of outgoing edges.
//! Unlike `SingleMutableCsr` (which stores exactly one edge), this stores up to N edges per vertex,
//! providing O(N) access instead of O(1), but supporting more flexible relationships.
//!
//! # Use Cases
//!
//! - One-to-many relationships (e.g., a person has multiple phones)
//! - Fixed-capacity relationships (e.g., a node can have up to 3 connections)
//! - Compact storage for sparse multi-edge scenarios
//!
//! # Configuration
//!
//! The maximum edges per vertex is fixed at creation time and cannot be changed.
//! This allows for predictable memory layout and cache-friendly access patterns.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::{StorageError, StorageResult};
use crate::storage::persistence::{read_u32_le, read_u64_le};

use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId, INVALID_EDGE_ID, INVALID_TIMESTAMP};

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const DEFAULT_EDGES_PER_VERTEX: usize = 4;

/// Multi-Single Mutable CSR: up to N edges per vertex in fixed slots.
pub struct MultiSingleMutableCsr {
    /// Flat array: [v0_slot0, v0_slot1, ..., v1_slot0, v1_slot1, ...]
    edges: Vec<Nbr>,
    /// Number of edges per vertex (fixed)
    edges_per_vertex: usize,
    /// Current count of active edges per vertex
    counts: Vec<u32>,
    /// Total edge count
    edge_count: AtomicU64,
    /// Number of vertices
    vertex_capacity: usize,
}

impl Clone for MultiSingleMutableCsr {
    fn clone(&self) -> Self {
        Self {
            edges: self.edges.clone(),
            edges_per_vertex: self.edges_per_vertex,
            counts: self.counts.clone(),
            edge_count: AtomicU64::new(self.edge_count.load(Ordering::Relaxed)),
            vertex_capacity: self.vertex_capacity,
        }
    }
}

impl fmt::Debug for MultiSingleMutableCsr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiSingleMutableCsr")
            .field("vertex_capacity", &self.vertex_capacity)
            .field("edges_per_vertex", &self.edges_per_vertex)
            .field("edge_count", &self.edge_count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl MultiSingleMutableCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY, DEFAULT_EDGES_PER_VERTEX)
    }

    pub fn with_capacity(vertex_capacity: usize, edges_per_vertex: usize) -> Self {
        let vertex_cap = vertex_capacity.max(1);
        let edges_per = edges_per_vertex.max(1);

        Self {
            edges: vec![
                Nbr::with_delete_ts(
                    VertexId::from_int64(0),
                    INVALID_EDGE_ID,
                    0,
                    INVALID_TIMESTAMP,
                    0
                );
                vertex_cap * edges_per
            ],
            edges_per_vertex: edges_per,
            counts: vec![0u32; vertex_cap],
            edge_count: AtomicU64::new(0),
            vertex_capacity: vertex_cap,
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    pub fn edges_per_vertex(&self) -> usize {
        self.edges_per_vertex
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    pub fn clear(&mut self) {
        self.edges.fill(Nbr::with_delete_ts(
            VertexId::from_int64(0),
            INVALID_EDGE_ID,
            0,
            INVALID_TIMESTAMP,
            0,
        ));
        self.counts.fill(0);
        self.edge_count.store(0, Ordering::Relaxed);
    }

    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity {
            return;
        }

        let additional = new_vertex_capacity - self.vertex_capacity;
        for _ in 0..additional {
            self.edges.extend(std::iter::repeat(
                Nbr::with_delete_ts(
                    VertexId::from_int64(0),
                    INVALID_EDGE_ID,
                    0,
                    INVALID_TIMESTAMP,
                    0,
                ),
            ).take(self.edges_per_vertex));
            self.counts.push(0);
        }
        self.vertex_capacity = new_vertex_capacity;
    }

    fn vertex_offset(&self, src_vid: u32) -> usize {
        (src_vid as usize) * self.edges_per_vertex
    }

    fn get_slot_for_dst(&self, src_vid: u32, dst: VertexId) -> Option<usize> {
        if src_vid as usize >= self.vertex_capacity {
            return None;
        }

        let base = self.vertex_offset(src_vid);
        let count = self.counts[src_vid as usize] as usize;

        for i in 0..count {
            if self.edges[base + i].neighbor == dst {
                return Some(base + i);
            }
        }
        None
    }

    fn find_empty_slot(&self, src_vid: u32) -> Option<usize> {
        if src_vid as usize >= self.vertex_capacity {
            return None;
        }

        let base = self.vertex_offset(src_vid);
        let count = self.counts[src_vid as usize] as usize;

        if count < self.edges_per_vertex {
            Some(base + count)
        } else {
            None
        }
    }
}

impl CsrBase for MultiSingleMutableCsr {
    fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    fn dump(&self) -> Vec<u8> {
        let mut data = Vec::new();

        // Write vertex capacity and edges per vertex
        data.extend(self.vertex_capacity.to_le_bytes());
        data.extend(self.edges_per_vertex.to_le_bytes());

        // Write edges array
        data.extend((self.edges.len() as u64).to_le_bytes());
        for nbr in &self.edges {
            data.extend(nbr.neighbor.as_bytes());
            data.extend(nbr.edge_id.0.to_le_bytes());
            data.extend(nbr.prop_offset.to_le_bytes());
            data.extend(nbr.create_ts.to_le_bytes());
            data.extend(nbr.delete_ts.to_le_bytes());
        }

        // Write counts
        data.extend((self.counts.len() as u64).to_le_bytes());
        for &c in &self.counts {
            data.extend(c.to_le_bytes());
        }

        data
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        let mut offset = 0;

        // Read vertex capacity and edges per vertex
        if data.len() < offset + 16 {
            return Err(StorageError::deserialize_error(
                "MultiSingleMutableCsr: data too short for capacities",
            ));
        }

        let vertex_cap = read_u64_le(data, &mut offset)? as usize;
        let edges_per = read_u64_le(data, &mut offset)? as usize;

        // Read edges array
        let edges_len = read_u64_le(data, &mut offset)? as usize;
        self.edges.clear();

        for _ in 0..edges_len {
            if offset >= data.len() {
                return Err(StorageError::deserialize_error(
                    "MultiSingleMutableCsr: insufficient data for vertex id length",
                ));
            }

            let len = data[offset] as usize;
            offset += 1;

            if data.len().saturating_sub(offset) < len {
                return Err(StorageError::deserialize_error(
                    "MultiSingleMutableCsr: insufficient data for neighbor",
                ));
            }

            let neighbor_bytes = data[offset..offset + len].to_vec();
            offset += len;

            let edge_id = EdgeId(read_u64_le(data, &mut offset)? as u64);
            let prop_offset = read_u32_le(data, &mut offset)?;
            let create_ts = read_u32_le(data, &mut offset)?;
            let delete_ts = read_u32_le(data, &mut offset)?;

            self.edges.push(Nbr {
                neighbor: VertexId::from_bytes(neighbor_bytes),
                edge_id,
                prop_offset,
                create_ts,
                delete_ts,
            });
        }

        // Read counts
        let counts_len = read_u64_le(data, &mut offset)? as usize;
        self.counts.clear();
        for _ in 0..counts_len {
            self.counts.push(read_u32_le(data, &mut offset)?);
        }

        self.vertex_capacity = vertex_cap;
        self.edges_per_vertex = edges_per;
        Ok(())
    }
}

impl MutableCsrTrait for MultiSingleMutableCsr {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        ts: Timestamp,
    ) -> bool {
        if src_vid as usize >= self.vertex_capacity {
            self.resize((src_vid as usize + 1).max(self.vertex_capacity * 2));
        }

        // Check if edge already exists
        if let Some(slot) = self.get_slot_for_dst(src_vid, dst) {
            // Update existing edge if timestamp is newer
            if ts > self.edges[slot].create_ts {
                self.edges[slot] = Nbr::new(dst, edge_id, prop_offset, ts);
                return true;
            }
            return false;
        }

        // Try to insert in empty slot
        if let Some(slot) = self.find_empty_slot(src_vid) {
            self.edges[slot] = Nbr::new(dst, edge_id, prop_offset, ts);
            self.counts[src_vid as usize] += 1;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return true;
        }

        // No space available
        false
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        if src_vid as usize >= self.vertex_capacity {
            return false;
        }

        let base = self.vertex_offset(src_vid);
        let count = self.counts[src_vid as usize] as usize;

        for i in 0..count {
            if self.edges[base + i].edge_id == edge_id {
                self.edges[base + i].delete_ts = ts;
                return true;
            }
        }
        false
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> bool {
        if let Some(slot) = self.get_slot_for_dst(src_vid, dst) {
            self.edges[slot].delete_ts = ts;
            return true;
        }
        false
    }

    fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if src_vid as usize >= self.vertex_capacity || offset < 0 {
            return false;
        }

        let base = self.vertex_offset(src_vid);
        let count = self.counts[src_vid as usize] as usize;

        if (offset as usize) < count {
            self.edges[base + offset as usize].delete_ts = ts;
            return true;
        }
        false
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if src_vid as usize >= self.vertex_capacity || offset < 0 {
            return false;
        }

        let base = self.vertex_offset(src_vid);
        let count = self.counts[src_vid as usize] as usize;

        if (offset as usize) < count {
            let slot = base + offset as usize;
            let nbr = &mut self.edges[slot];
            // Only revert deletions that happened at or before rollback time.
            if nbr.delete_ts < u32::MAX && nbr.delete_ts <= ts {
                nbr.delete_ts = u32::MAX;
                return true;
            }
        }
        false
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        if let Some(slot) = self.get_slot_for_dst(src_vid, dst) {
            let nbr = self.edges[slot];
            if nbr.is_valid_at(ts) {
                return Some(nbr);
            }
        }
        None
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        if src_vid as usize >= self.vertex_capacity {
            return Vec::new();
        }

        let base = self.vertex_offset(src_vid);
        let count = self.counts[src_vid as usize] as usize;

        self.edges[base..base + count]
            .iter()
            .filter(|nbr| nbr.is_valid_at(ts))
            .copied()
            .collect()
    }

    fn compact_with_ts(&mut self, ts: Timestamp, _reserve_ratio: f32) -> usize {
        let mut removed = 0;

        // Remove expired edges by shifting
        for src_vid in 0..self.vertex_capacity as u32 {
            let base = self.vertex_offset(src_vid);
            let count = self.counts[src_vid as usize] as usize;

            let mut write_idx = 0;
            for read_idx in 0..count {
                if self.edges[base + read_idx].delete_ts > ts {
                    if write_idx != read_idx {
                        self.edges[base + write_idx] = self.edges[base + read_idx];
                    }
                    write_idx += 1;
                } else {
                    removed += 1;
                }
            }

            self.counts[src_vid as usize] = write_idx as u32;
        }

        // Update edge count
        self.edge_count.fetch_sub(removed as u64, Ordering::Relaxed);

        removed
    }

    fn used_memory_size(&self) -> usize {
        let edges_size = self.edges.len() * std::mem::size_of::<Nbr>();
        let counts_size = self.counts.len() * std::mem::size_of::<u32>();
        edges_size + counts_size + std::mem::size_of::<Self>()
    }
}

impl MultiSingleMutableCsr {
    pub fn iter(&self, ts: Timestamp) -> MultiSingleMutableCsrIterator<'_> {
        MultiSingleMutableCsrIterator::new(self, ts)
    }
}

/// Iterator over multi-single CSR edges
pub struct MultiSingleMutableCsrIterator<'a> {
    csr: &'a MultiSingleMutableCsr,
    ts: Timestamp,
    current_vertex: usize,
    edge_idx: usize,
}

impl<'a> MultiSingleMutableCsrIterator<'a> {
    pub fn new(csr: &'a MultiSingleMutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            current_vertex: 0,
            edge_idx: 0,
        }
    }
}

impl<'a> Iterator for MultiSingleMutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity {
            let count = self.csr.counts[self.current_vertex] as usize;

            while self.edge_idx < count {
                let base = self.csr.vertex_offset(self.current_vertex as u32);
                let nbr = self.csr.edges[base + self.edge_idx];
                self.edge_idx += 1;

                if nbr.is_valid_at(self.ts) {
                    return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                }
            }

            // Move to next vertex
            self.current_vertex += 1;
            self.edge_idx = 0;
        }

        None
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_single_insert_and_query() {
        let mut csr = MultiSingleMutableCsr::with_capacity(10, 4);

        // Insert multiple edges for same vertex
        assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 1));
        assert!(csr.insert_edge(0, VertexId::from_int64(2), EdgeId(101), 4, 1));
        assert!(csr.insert_edge(0, VertexId::from_int64(3), EdgeId(102), 8, 1));

        assert_eq!(csr.edge_count(), 3);

        // Query all edges
        let edges = csr.edges_of(0, 999);
        assert_eq!(edges.len(), 3);

        // Query specific edge
        assert!(csr.get_edge(0, VertexId::from_int64(1), 999).is_some());
    }

    #[test]
    fn test_multi_single_capacity_exceeded() {
        let mut csr = MultiSingleMutableCsr::with_capacity(10, 2);

        assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 1));
        assert!(csr.insert_edge(0, VertexId::from_int64(2), EdgeId(101), 4, 1));
        // Third insertion should fail due to capacity
        assert!(!csr.insert_edge(0, VertexId::from_int64(3), EdgeId(102), 8, 1));

        assert_eq!(csr.edge_count(), 2);
    }

    #[test]
    fn test_multi_single_delete_and_revert() {
        let mut csr = MultiSingleMutableCsr::with_capacity(10, 4);

        assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 1));
        assert!(csr.insert_edge(0, VertexId::from_int64(2), EdgeId(101), 4, 1));

        // Delete first edge
        assert!(csr.delete_edge_by_offset(0, 0, 2));

        let edges = csr.edges_of(0, 2);
        assert_eq!(edges.len(), 1);

        // Revert deletion
        assert!(csr.revert_delete_by_offset(0, 0, 3));

        let edges = csr.edges_of(0, 3);
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn test_multi_single_iterator() {
        let mut csr = MultiSingleMutableCsr::with_capacity(5, 3);

        // Insert edges across multiple vertices
        assert!(csr.insert_edge(0, VertexId::from_int64(10), EdgeId(1), 0, 1));
        assert!(csr.insert_edge(0, VertexId::from_int64(11), EdgeId(2), 1, 1));
        assert!(csr.insert_edge(1, VertexId::from_int64(20), EdgeId(3), 2, 1));
        assert!(csr.insert_edge(2, VertexId::from_int64(30), EdgeId(4), 3, 1));

        // Collect via iterator
        let edges_from_iter: Vec<_> = csr.iter(999).collect();
        assert_eq!(edges_from_iter.len(), 4);

        // Verify each edge is present
        let edge_ids: Vec<_> = edges_from_iter.iter().map(|(_, nbr)| nbr.edge_id.0).collect();
        assert!(edge_ids.contains(&1));
        assert!(edge_ids.contains(&2));
        assert!(edge_ids.contains(&3));
        assert!(edge_ids.contains(&4));
    }

    #[test]
    fn test_multi_single_timestamp_filtering() {
        let mut csr = MultiSingleMutableCsr::with_capacity(10, 4);

        // Insert edges at different timestamps
        assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 10));
        assert!(csr.insert_edge(0, VertexId::from_int64(2), EdgeId(101), 4, 20));
        assert!(csr.insert_edge(0, VertexId::from_int64(3), EdgeId(102), 8, 30));

        // Query at different timestamps
        assert_eq!(csr.edges_of(0, 5).len(), 0);   // Before any edge
        assert_eq!(csr.edges_of(0, 10).len(), 1);  // After first edge
        assert_eq!(csr.edges_of(0, 25).len(), 2);  // After second edge
        assert_eq!(csr.edges_of(0, 35).len(), 3);  // After all edges

        // Delete one edge
        assert!(csr.delete_edge(0, EdgeId(101), 40));

        // Query after deletion
        assert_eq!(csr.edges_of(0, 35).len(), 3);  // Before deletion
        assert_eq!(csr.edges_of(0, 40).len(), 2);  // After deletion
    }

    #[test]
    fn test_multi_single_compact_with_ts() {
        let mut csr = MultiSingleMutableCsr::with_capacity(10, 4);

        // Insert edges
        assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 5));
        assert!(csr.insert_edge(0, VertexId::from_int64(2), EdgeId(101), 4, 10));
        assert!(csr.insert_edge(0, VertexId::from_int64(3), EdgeId(102), 8, 15));

        // Delete some edges
        assert!(csr.delete_edge(0, EdgeId(100), 20));
        assert!(csr.delete_edge(0, EdgeId(101), 25));

        // Compact at ts=30
        let removed = csr.compact_with_ts(30, 0.2);
        assert_eq!(removed, 2); // Two edges deleted

        assert_eq!(csr.edge_count(), 1);

        // Verify remaining edge
        let edges = csr.edges_of(0, 999);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_id.0, 102);
    }

    #[test]
    fn test_multi_single_multiple_vertices() {
        let mut csr = MultiSingleMutableCsr::with_capacity(5, 2);

        // Add to each vertex (with limited capacity)
        for src in 0..5 {
            assert!(csr.insert_edge(src, VertexId::from_int64(100), EdgeId(src as u64 * 2), 0, 1));
            assert!(csr.insert_edge(src, VertexId::from_int64(101), EdgeId(src as u64 * 2 + 1), 4, 1));
            // Third edge should fail (capacity is 2)
            assert!(!csr.insert_edge(src, VertexId::from_int64(102), EdgeId(src as u64 * 2 + 2), 8, 1));
        }

        assert_eq!(csr.edge_count(), 10); // 5 vertices * 2 edges each

        // Verify each vertex has exactly 2 edges
        for src in 0..5 {
            let edges = csr.edges_of(src, 999);
            assert_eq!(edges.len(), 2);
        }
    }
}


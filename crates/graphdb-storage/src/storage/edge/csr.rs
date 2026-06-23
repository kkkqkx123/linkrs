//! CSR (Compressed Sparse Row) Implementation
//!
//! Immutable CSR for read-optimized edge storage.
//! Uses contiguous storage for memory efficiency and cache locality.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::{StorageError, StorageResult};
use crate::storage::persistence::{read_u32_le, read_u64_le};

use super::{CsrBase, EdgeId, ImmutableNbr, Nbr, Timestamp, VertexId};

fn write_vertex_id(out: &mut Vec<u8>, id: VertexId) {
    let bytes = id.as_bytes();
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn read_vertex_id(data: &[u8], offset: &mut usize) -> StorageResult<VertexId> {
    if *offset >= data.len() {
        return Err(StorageError::deserialize_error(
            "CSR data too short for vertex id length",
        ));
    }

    let len = data[*offset] as usize;
    *offset += 1;
    if data.len().saturating_sub(*offset) < len {
        return Err(StorageError::deserialize_error(
            "CSR data too short for vertex id bytes",
        ));
    }

    let id = VertexId::from_bytes(data[*offset..*offset + len].to_vec());
    *offset += len;
    Ok(id)
}

/// Immutable CSR with contiguous storage
///
/// Standard CSR format:
/// - `offsets`: Offset array where offsets[v] is the start index in edges for vertex v
/// - `edges`: Contiguous array of all edges
/// - offsets[vertex_capacity] stores the total edge count
#[derive(Debug)]
pub struct Csr {
    offsets: Vec<u32>,
    edges: Vec<ImmutableNbr>,
    edge_count: AtomicU64,
    vertex_capacity: usize,
}

impl Clone for Csr {
    fn clone(&self) -> Self {
        Self {
            offsets: self.offsets.clone(),
            edges: self.edges.clone(),
            edge_count: AtomicU64::new(self.edge_count.load(Ordering::Relaxed)),
            vertex_capacity: self.vertex_capacity,
        }
    }
}

impl Csr {
    pub fn new() -> Self {
        Self {
            offsets: vec![0],
            edges: Vec::new(),
            edge_count: AtomicU64::new(0),
            vertex_capacity: 1,
        }
    }

    pub fn with_capacity(vertex_capacity: usize, edge_capacity: usize) -> Self {
        Self {
            offsets: vec![0; vertex_capacity + 1],
            edges: Vec::with_capacity(edge_capacity),
            edge_count: AtomicU64::new(0),
            vertex_capacity,
        }
    }

    /// Get approximate memory bytes per edge stored in this CSR.
    ///
    /// Used for merge heuristics and size estimation.
    /// Csr is immutable (post-freeze), so uses average across strategies.
    ///
    /// Returns ~24 bytes per edge (immutable CSR storage).
    pub fn bytes_per_edge(&self) -> usize {
        24  // Immutable CSR uses ~24 bytes per edge (compressed format)
    }

    /// Resize vertex capacity
    fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity > self.vertex_capacity {
            let last_offset = *self.offsets.last().unwrap_or(&0);
            self.offsets.resize(new_vertex_capacity + 1, last_offset);
            self.vertex_capacity = new_vertex_capacity;
        }
    }

    pub fn edges_of(&self, vid: u32) -> &[ImmutableNbr] {
        let vid_idx = vid as usize;
        if vid_idx >= self.vertex_capacity {
            return &[];
        }

        let start = self.offsets[vid_idx] as usize;
        let end = self.offsets[vid_idx + 1] as usize;

        if start >= self.edges.len() || end > self.edges.len() || start > end {
            return &[];
        }

        &self.edges[start..end]
    }

    /// Get a specific edge
    pub fn get_edge(&self, src: u32, dst: VertexId) -> Option<&ImmutableNbr> {
        let edges = self.edges_of(src);
        edges.iter().find(|e| e.neighbor == dst)
    }

    /// Get edges with their CSR positions for position-based EdgeId mapping.
    ///
    /// Returns (absolute_csr_position, edge) pairs. The position can be used with
    /// a CSR position-to-entry mapping to recover segment-level EdgeIds that may be
    /// stored separately from the edge data.
    pub fn edges_of_with_position(&self, vid: u32) -> Vec<(usize, &ImmutableNbr)> {
        let vid_idx = vid as usize;
        if vid_idx >= self.vertex_capacity {
            return Vec::new();
        }

        let start = self.offsets[vid_idx] as usize;
        let end = self.offsets[vid_idx + 1] as usize;

        if start >= self.edges.len() || end > self.edges.len() || start > end {
            return Vec::new();
        }

        self.edges[start..end]
            .iter()
            .enumerate()
            .map(|(i, edge)| (start + i, edge))
            .collect()
    }

    pub fn from_nbr_entries(entries: &[(u32, Nbr)], vertex_capacity: usize) -> Self {
        let mut csr = Self::with_capacity(vertex_capacity.max(1), entries.len());
        if entries.is_empty() {
            return csr;
        }

        let src_list: Vec<_> = entries.iter().map(|(src, _)| *src).collect();
        let dst_list: Vec<_> = entries.iter().map(|(_, nbr)| nbr.neighbor).collect();
        let edge_ids: Vec<_> = entries.iter().map(|(_, nbr)| nbr.edge_id).collect();
        let prop_offsets: Vec<_> = entries.iter().map(|(_, nbr)| nbr.prop_offset).collect();
        let timestamps: Vec<_> = entries.iter().map(|(_, nbr)| nbr.create_ts).collect();
        csr.batch_put_edges_with_timestamps(
            &src_list,
            &dst_list,
            &edge_ids,
            &prop_offsets,
            &timestamps,
        );
        csr
    }

    pub fn batch_put_edges_with_timestamps(
        &mut self,
        src_list: &[u32],
        dst_list: &[VertexId],
        edge_ids: &[EdgeId],
        prop_offsets: &[u32],
        timestamps: &[Timestamp],
    ) {
        if src_list.is_empty() {
            return;
        }

        let max_vertex = *src_list.iter().max().unwrap_or(&0) as usize;
        if max_vertex >= self.vertex_capacity {
            self.resize(max_vertex + 1);
        }

        let mut degrees = vec![0u32; self.vertex_capacity];
        for src in src_list {
            let src_idx = *src as usize;
            if src_idx < degrees.len() {
                degrees[src_idx] += 1;
            }
        }

        let mut new_offsets = vec![0u32; self.vertex_capacity + 1];
        let mut cumsum = 0u32;
        for (i, &deg) in degrees.iter().enumerate() {
            new_offsets[i] = cumsum;
            cumsum += deg;
        }
        new_offsets[self.vertex_capacity] = cumsum;

        let mut new_edges =
            vec![ImmutableNbr::new(VertexId::from_int64(0), EdgeId(0), 0); src_list.len()];
        let mut current_pos = new_offsets.clone();
        for i in 0..src_list.len() {
            let src = src_list[i] as usize;
            if src < current_pos.len() - 1 {
                let pos = current_pos[src] as usize;
                if pos < new_edges.len() {
                    new_edges[pos] = ImmutableNbr::with_timestamp(
                        dst_list[i],
                        edge_ids[i],
                        prop_offsets[i],
                        timestamps[i],
                    );
                    current_pos[src] += 1;
                }
            }
        }

        self.offsets = new_offsets;
        self.edges = new_edges;
        self.edge_count
            .store(src_list.len() as u64, Ordering::Relaxed);
    }

    /// Create iterator over all edges
    pub fn iter(&self) -> CsrIterator<'_> {
        CsrIterator::new(self)
    }

    /// Dump to bytes
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();

        result.extend_from_slice(&(self.vertex_capacity as u64).to_le_bytes());
        result.extend_from_slice(&self.edge_count.load(Ordering::Relaxed).to_le_bytes());

        result.extend_from_slice(&(self.offsets.len() as u64).to_le_bytes());
        for &offset in &self.offsets {
            result.extend_from_slice(&offset.to_le_bytes());
        }

        result.extend_from_slice(&(self.edges.len() as u64).to_le_bytes());
        for edge in &self.edges {
            write_vertex_id(&mut result, edge.neighbor);
            result.extend_from_slice(&edge.edge_id.to_le_bytes());
            result.extend_from_slice(&edge.prop_offset.to_le_bytes());
            result.extend_from_slice(&edge.timestamp.to_le_bytes());
        }

        result
    }

    /// Load from bytes
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 24 {
            return Err(StorageError::deserialize_error(
                "Immutable CSR data too short for header",
            ));
        }

        let mut offset = 0usize;

        let vertex_capacity = read_u64_le(data, &mut offset)? as usize;
        let edge_count = read_u64_le(data, &mut offset)?;
        let offsets_len = read_u64_le(data, &mut offset)? as usize;

        let mut offsets = Vec::with_capacity(offsets_len);
        for _ in 0..offsets_len {
            offsets.push(read_u32_le(data, &mut offset)?);
        }

        let edges_len = read_u64_le(data, &mut offset)? as usize;

        let mut edges = Vec::with_capacity(edges_len);
        for _ in 0..edges_len {
            let neighbor = read_vertex_id(data, &mut offset)?;
            let raw_edge_id = read_u64_le(data, &mut offset)?;
            let prop_offset = read_u32_le(data, &mut offset)?;
            let timestamp = read_u32_le(data, &mut offset)?;

            edges.push(ImmutableNbr::with_timestamp(
                neighbor,
                EdgeId(raw_edge_id),
                prop_offset,
                timestamp,
            ));
        }

        self.vertex_capacity = vertex_capacity;
        self.offsets = offsets;
        self.edges = edges;
        self.edge_count.store(edge_count, Ordering::Relaxed);

        Ok(())
    }

    pub fn used_memory_size(&self) -> usize {
        self.offsets.len() * std::mem::size_of::<u32>()
            + self.edges.len() * std::mem::size_of::<ImmutableNbr>()
            + std::mem::size_of::<Self>()
    }
}

impl Default for Csr {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterator over all edges in the CSR
pub struct CsrIterator<'a> {
    csr: &'a Csr,
    current_vertex: usize,
    current_edge: usize,
}

impl<'a> CsrIterator<'a> {
    pub fn new(csr: &'a Csr) -> Self {
        Self {
            csr,
            current_vertex: 0,
            current_edge: 0,
        }
    }
}

impl<'a> Iterator for CsrIterator<'a> {
    type Item = (VertexId, &'a ImmutableNbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity {
            let start = self.csr.offsets[self.current_vertex] as usize;
            let end = self.csr.offsets[self.current_vertex + 1] as usize;

            if self.current_edge < start {
                self.current_edge = start;
            }

            if self.current_edge < end && self.current_edge < self.csr.edges.len() {
                let edge = &self.csr.edges[self.current_edge];
                self.current_edge += 1;
                return Some((VertexId::from_int64(self.current_vertex as i64), edge));
            }

            self.current_vertex += 1;
            self.current_edge = 0;
        }
        None
    }
}

impl CsrBase for Csr {
    fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    fn dump(&self) -> Vec<u8> {
        Csr::dump(self)
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        Csr::load(self, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut csr = Csr::with_capacity(10, 100);

        csr.batch_put_edges_with_timestamps(
            &[0u32, 0, 1, 2],
            &[1, 2, 3, 0].map(|v| VertexId::from_int64(v as i64)),
            &[EdgeId(0), EdgeId(1), EdgeId(2), EdgeId(3)],
            &[0, 1, 2, 3],
            &[100, 100, 100, 100],
        );

        assert_eq!(csr.edge_count(), 4);

        assert_eq!(csr.edges_of(0).len(), 2);
        assert_eq!(csr.edges_of(1).len(), 1);
        assert_eq!(csr.edges_of(2).len(), 1);

        assert!(csr.get_edge(0, VertexId::from_int64(1)).is_some());
        assert!(csr.get_edge(0, VertexId::from_int64(2)).is_some());
        assert!(csr.get_edge(0, VertexId::from_int64(3)).is_none());
    }

    #[test]
    fn test_iterator() {
        let mut csr = Csr::with_capacity(5, 20);

        csr.batch_put_edges_with_timestamps(
            &[0u32, 1, 2],
            &[1, 2, 3].map(VertexId::from_int64),
            &[EdgeId(0), EdgeId(1), EdgeId(2)],
            &[0, 0, 0],
            &[100, 100, 100],
        );

        let count = csr.iter().count();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_dump_and_load() {
        let mut csr1 = Csr::with_capacity(10, 100);

        csr1.batch_put_edges_with_timestamps(
            &[0u32, 0, 1, 2],
            &[1, 2, 3, 0].map(|v| VertexId::from_int64(v as i64)),
            &[EdgeId(0), EdgeId(1), EdgeId(2), EdgeId(3)],
            &[0, 1, 2, 3],
            &[100, 100, 100, 100],
        );

        let data = csr1.dump();

        let mut csr2 = Csr::new();
        let _ = csr2.load(&data);

        assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
        assert_eq!(csr2.edge_count(), csr1.edge_count());
        assert!(csr2.get_edge(0, VertexId::from_int64(1)).is_some());
        assert!(csr2.get_edge(0, VertexId::from_int64(2)).is_some());
        assert!(csr2.get_edge(1, VertexId::from_int64(3)).is_some());
    }

    #[test]
    fn test_get_edge() {
        let mut csr = Csr::with_capacity(10, 100);

        csr.batch_put_edges_with_timestamps(
            &[0u32, 0],
            &[1, 2].map(VertexId::from_int64),
            &[EdgeId(100), EdgeId(101)],
            &[0, 1],
            &[100, 100],
        );

        let edge = csr.get_edge(0, VertexId::from_int64(1));
        assert!(edge.is_some());
        assert_eq!(edge.unwrap().edge_id, EdgeId(100));

        let edge = csr.get_edge(0, VertexId::from_int64(3));
        assert!(edge.is_none());
    }
}

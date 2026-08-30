use graphdb_core::types::VertexId;

use super::MutableCsr;
use crate::edge::Nbr;
use graphdb_core::types::Timestamp;

/// Iterator over edges of a single vertex in MutableCsr.
pub struct VertexEdgesIter<'a> {
    csr: &'a MutableCsr,
    ts: Timestamp,
    primary_idx: usize,
    primary_end: usize,
    overflow_chunks: Option<&'a Vec<Vec<Nbr>>>,
    overflow_chunk_idx: usize,
    overflow_edge_idx: usize,
}

impl<'a> VertexEdgesIter<'a> {
    pub fn new(csr: &'a MutableCsr, src_vid: u32, ts: Timestamp) -> Self {
        let src_idx = src_vid as usize;
        if src_idx >= csr.vertex_capacity() {
            return Self {
                csr,
                ts,
                primary_idx: 0,
                primary_end: 0,
                overflow_chunks: None,
                overflow_chunk_idx: 0,
                overflow_edge_idx: 0,
            };
        }

        let degree = csr.degrees[src_idx] as usize;
        let offset = csr.adj_offsets[src_idx] as usize;
        Self {
            csr,
            ts,
            primary_idx: offset,
            primary_end: offset + degree,
            overflow_chunks: csr.overflow_chunks.get(&src_vid),
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
        }
    }
}

impl<'a> Iterator for VertexEdgesIter<'a> {
    type Item = &'a Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        while self.primary_idx < self.primary_end {
            let nbr = &self.csr.nbr_list[self.primary_idx];
            self.primary_idx += 1;
            if nbr.is_alive_at(self.ts) {
                return Some(nbr);
            }
        }

        if let Some(chunks) = self.overflow_chunks {
            while self.overflow_chunk_idx < chunks.len() {
                let chunk = &chunks[self.overflow_chunk_idx];
                while self.overflow_edge_idx < chunk.len() {
                    let nbr = &chunk[self.overflow_edge_idx];
                    self.overflow_edge_idx += 1;
                    if nbr.is_alive_at(self.ts) {
                        return Some(nbr);
                    }
                }
                self.overflow_chunk_idx += 1;
                self.overflow_edge_idx = 0;
            }
        }

        None
    }
}

pub struct MutableCsrIterator<'a> {
    csr: &'a MutableCsr,
    ts: Timestamp,
    include_deleted: bool,
    current_vertex: usize,
    current_edge: usize,
    in_overflow: bool,
    overflow_chunks: Option<&'a Vec<Vec<Nbr>>>,
    overflow_chunk_idx: usize,
    overflow_edge_idx: usize,
}

impl<'a> MutableCsrIterator<'a> {
    pub fn new(csr: &'a MutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            include_deleted: false,
            current_vertex: 0,
            current_edge: 0,
            in_overflow: false,
            overflow_chunks: None,
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
        }
    }

    /// Iterator over every stored entry, including tombstoned ones.
    pub fn new_all(csr: &'a MutableCsr) -> Self {
        Self {
            csr,
            ts: 0,
            include_deleted: true,
            current_vertex: 0,
            current_edge: 0,
            in_overflow: false,
            overflow_chunks: None,
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
        }
    }
}

impl<'a> Iterator for MutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity() {
            let degree = self.csr.degrees[self.current_vertex] as usize;
            let offset = self.csr.adj_offsets[self.current_vertex] as usize;

            if !self.in_overflow {
                if self.current_edge == 0 {
                    self.overflow_chunks =
                        self.csr.overflow_chunks.get(&(self.current_vertex as u32));
                    self.overflow_chunk_idx = 0;
                    self.overflow_edge_idx = 0;
                }
                while self.current_edge < degree {
                    let nbr = self.csr.nbr_list[offset + self.current_edge];
                    self.current_edge += 1;
                    if self.include_deleted || nbr.is_alive_at(self.ts) {
                        return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                    }
                }
                self.in_overflow = true;
            }

            if let Some(chunks) = self.overflow_chunks {
                while self.overflow_chunk_idx < chunks.len() {
                    let chunk = &chunks[self.overflow_chunk_idx];
                    while self.overflow_edge_idx < chunk.len() {
                        let nbr = chunk[self.overflow_edge_idx];
                        self.overflow_edge_idx += 1;
                        if self.include_deleted || nbr.is_alive_at(self.ts) {
                            return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                        }
                    }
                    self.overflow_chunk_idx += 1;
                    self.overflow_edge_idx = 0;
                }
            }

            self.current_vertex += 1;
            self.current_edge = 0;
            self.in_overflow = false;
            self.overflow_chunk_idx = 0;
            self.overflow_edge_idx = 0;
        }
        None
    }
}

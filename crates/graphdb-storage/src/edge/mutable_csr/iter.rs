use std::iter::Zip;
use std::slice::Iter as SliceIter;

use graphdb_core::types::VertexId;

use super::overflow::OverflowChunk;
use super::MutableCsr;
use crate::edge::{ColdStamps, HotNbr, Nbr};
use graphdb_core::types::Timestamp;

impl MutableCsr {
    /// Create iterator over all edges
    pub fn iter(&self, ts: Timestamp) -> MutableCsrIterator<'_> {
        MutableCsrIterator::new(self, ts)
    }

    /// Create an iterator over all physically present edges, including
    /// entries marked as deleted (delete_ts != MAX). Used when rebuilding the
    /// CSR so tombstoned entries survive remapping.
    pub fn iter_all(&self) -> MutableCsrIterator<'_> {
        MutableCsrIterator::new_all(self)
    }

    /// Iterate edges of a vertex without collecting into a Vec.
    /// Test-only row-stamp filtered iterator; production scans go through the version authority.
    /// Filters by the delete replica only: creation stamps live in the table
    /// authority, not in the row, so this probe can never decide full MVCC
    /// visibility on its own.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> VertexEdgesIter<'_> {
        VertexEdgesIter::new(self, src_vid, ts)
    }
}

/// Iterator over edges of a single vertex in MutableCsr.
///
/// The primary row is held as a fused slice-pair iterator, so the walk pays
/// one bounds check per row instead of one per slot.
pub struct VertexEdgesIter<'a> {
    primary: Zip<SliceIter<'a, HotNbr>, SliceIter<'a, ColdStamps>>,
    ts: Timestamp,
    overflow_chunks: Option<&'a Vec<OverflowChunk>>,
    overflow_chunk_idx: usize,
    overflow_edge_idx: usize,
}

impl<'a> VertexEdgesIter<'a> {
    pub fn new(csr: &'a MutableCsr, src_vid: u32, ts: Timestamp) -> Self {
        let (hot, cold) = csr.primary_pair(src_vid as usize);
        Self {
            primary: hot.iter().zip(cold.iter()),
            ts,
            overflow_chunks: csr.overflow_chunks.get(src_vid),
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
        }
    }
}

impl<'a> Iterator for VertexEdgesIter<'a> {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        for (hot, cold) in self.primary.by_ref() {
            let nbr = Nbr::from_parts(*hot, *cold);
            if nbr.is_alive_at(self.ts) {
                return Some(nbr);
            }
        }

        if let Some(chunks) = self.overflow_chunks {
            while self.overflow_chunk_idx < chunks.len() {
                let chunk = &chunks[self.overflow_chunk_idx];
                while self.overflow_edge_idx < chunk.len() {
                    let nbr = chunk.slot_at(self.overflow_edge_idx).unwrap();
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
    overflow_chunks: Option<&'a Vec<OverflowChunk>>,
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
        // Copy the shared reference out so slice borrows below live for 'a
        // and do not conflict with the cursor mutations on `self`.
        let csr: &'a MutableCsr = self.csr;
        while self.current_vertex < csr.vertex_capacity() {
            if !self.in_overflow {
                if self.current_edge == 0 {
                    self.overflow_chunks = csr.overflow_chunks.get(self.current_vertex as u32);
                    self.overflow_chunk_idx = 0;
                    self.overflow_edge_idx = 0;
                }
                let (hot, cold) = csr.primary_pair(self.current_vertex);
                while self.current_edge < hot.len() {
                    let nbr = Nbr::from_parts(hot[self.current_edge], cold[self.current_edge]);
                    self.current_edge += 1;
                    if self.include_deleted || nbr.is_alive_at(self.ts) {
                        return Some((
                            VertexId::from_u32(u32::try_from(self.current_vertex).ok()?),
                            nbr,
                        ));
                    }
                }
                self.in_overflow = true;
            }

            if let Some(chunks) = self.overflow_chunks {
                while self.overflow_chunk_idx < chunks.len() {
                    let chunk = &chunks[self.overflow_chunk_idx];
                    while self.overflow_edge_idx < chunk.len() {
                        let nbr = chunk.slot_at(self.overflow_edge_idx).unwrap();
                        self.overflow_edge_idx += 1;
                        if self.include_deleted || nbr.is_alive_at(self.ts) {
                            return Some((
                                VertexId::from_u32(u32::try_from(self.current_vertex).ok()?),
                                nbr,
                            ));
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

use graphdb_core::types::{EdgeId, VertexId};

use super::super::Nbr;
use super::overflow::PureOverflowChunk;
use super::{PureTopologyCsr, INVALID_EDGE_ID};

/// Borrowed walk over one pure-topology row.
///
/// Holds slices of the primary window plus the overflow chain reference, so
/// iteration needs no allocation. Sentinel holes are skipped inline.
#[derive(Debug, Clone, Copy)]
pub struct PureRowIter<'a> {
    pub(crate) csr: &'a PureTopologyCsr,
    pub(crate) primary_endpoints: &'a [u32],
    pub(crate) primary_ids: &'a [u64],
    pub(crate) primary_idx: usize,
    pub(crate) overflow: Option<&'a Vec<PureOverflowChunk>>,
    pub(crate) chunk_idx: usize,
    pub(crate) slot_idx: usize,
}

impl<'a> Iterator for PureRowIter<'a> {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        while self.primary_idx < self.primary_endpoints.len() {
            let endpoint = self.primary_endpoints[self.primary_idx];
            let edge_id = EdgeId(self.primary_ids[self.primary_idx]);
            self.primary_idx += 1;
            if edge_id != INVALID_EDGE_ID {
                return Some(self.csr.make_nbr(endpoint, edge_id));
            }
        }
        let chunks = self.overflow?;
        while self.chunk_idx < chunks.len() {
            let chunk = &chunks[self.chunk_idx];
            while self.slot_idx < chunk.len() {
                let endpoint = chunk.endpoints[self.slot_idx];
                let edge_id = EdgeId(chunk.edge_ids[self.slot_idx]);
                self.slot_idx += 1;
                if edge_id != INVALID_EDGE_ID {
                    return Some(self.csr.make_nbr(endpoint, edge_id));
                }
            }
            self.chunk_idx += 1;
            self.slot_idx = 0;
        }
        None
    }
}

/// Borrowed walk over every live entry of the table without allocating.
///
/// Advances one borrowed row walk at a time, so full-table rebuilds and
/// scans iterate with no intermediate vector regardless of primary versus
/// overflow layout. Sentinel holes are skipped inline by the row walk.
#[derive(Debug, Clone, Copy)]
pub struct PureAllIter<'a> {
    pub(crate) csr: &'a PureTopologyCsr,
    pub(crate) cap: u32,
    pub(crate) vid: u32,
    pub(crate) row: PureRowIter<'a>,
}

impl<'a> Iterator for PureAllIter<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.vid >= self.cap {
                return None;
            }
            if let Some(nbr) = self.row.next() {
                return Some((VertexId::from_int64(self.vid as i64), nbr));
            }
            self.vid += 1;
            if self.vid < self.cap {
                self.row = self.csr.iter_row(self.vid);
            }
        }
    }
}

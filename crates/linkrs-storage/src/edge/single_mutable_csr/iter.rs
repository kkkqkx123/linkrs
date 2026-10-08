//! Full-table iterator for `SingleMutableCsr`.

use linkrs_core::types::{Timestamp, VertexId};

use super::super::{Nbr, INVALID_EDGE_ID};
use super::SingleMutableCsr;

pub struct SingleMutableCsrIterator<'a> {
    csr: &'a SingleMutableCsr,
    current_vertex: usize,
    ts: Timestamp,
    include_deleted: bool,
}

impl<'a> SingleMutableCsrIterator<'a> {
    pub fn new(csr: &'a SingleMutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            current_vertex: 0,
            ts,
            include_deleted: false,
        }
    }

    /// Iterator over every stored entry, including tombstoned ones.
    pub fn new_all(csr: &'a SingleMutableCsr) -> Self {
        Self {
            csr,
            current_vertex: 0,
            ts: 0,
            include_deleted: true,
        }
    }
}

impl<'a> Iterator for SingleMutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity() {
            let vid = self.current_vertex;
            self.current_vertex += 1;

            let nbr = if self.include_deleted {
                self.csr
                    .slot_at(vid)
                    .filter(|n| n.edge_id != INVALID_EDGE_ID)
            } else {
                self.csr.get_edge_any_dst(vid as u32, self.ts)
            };
            if let Some(nbr) = nbr {
                return Some((VertexId::from_u32(u32::try_from(vid).ok()?), nbr));
            }
        }
        None
    }
}

impl SingleMutableCsr {
    pub fn iter(&self, ts: Timestamp) -> SingleMutableCsrIterator<'_> {
        SingleMutableCsrIterator::new(self, ts)
    }

    /// Iterate over all physically present entries, including tombstoned ones.
    pub fn iter_all(&self) -> SingleMutableCsrIterator<'_> {
        SingleMutableCsrIterator::new_all(self)
    }
}

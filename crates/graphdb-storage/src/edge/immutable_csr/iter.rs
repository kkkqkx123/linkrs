use super::super::{ColdStamps, HotNbr, Nbr, Timestamp, VertexId};
use super::ImmutableCsr;

impl ImmutableCsr {
    /// Iterate timestamp-visible entries across all rows.
    pub fn iter(&self, ts: Timestamp) -> ImmutableCsrIterator<'_> {
        ImmutableCsrIterator::new(self, ts)
    }

    /// Iterate timestamp-visible entries of one row without allocating.
    ///
    /// Zero-copy counterpart of `edges_of`: walks the packed hot slice and
    /// assembles each record with its cold half inline, so per-vertex scans
    /// over frozen groups never touch the allocator. Out-of-range rows yield
    /// an empty iterator.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> FrozenRowIter<'_> {
        let (start, end) = self.row_window(src_vid).unwrap_or((0, 0));
        FrozenRowIter {
            hot: &self.hot_entries[start..end],
            cold: &self.cold_entries[start..end],
            ts,
            idx: 0,
        }
    }

    /// Iterate every physically stored entry, including tombstoned ones.
    pub fn iter_all(&self) -> ImmutableCsrIterator<'_> {
        ImmutableCsrIterator::new_all(self)
    }
}

/// Iterator over one frozen row, yielding assembled entries.
///
/// The packed row is already contiguous, so iteration is a filtered
/// hot-slice walk with no allocation and no pointer chasing. Records are
/// assembled by value because the halves live in separate slices.
pub struct FrozenRowIter<'a> {
    hot: &'a [HotNbr],
    cold: &'a [ColdStamps],
    ts: Timestamp,
    idx: usize,
}

impl<'a> Iterator for FrozenRowIter<'a> {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.hot.len() {
            let nbr = Nbr::from_parts(self.hot[self.idx], self.cold[self.idx]);
            self.idx += 1;
            if nbr.is_alive_at(self.ts) {
                return Some(nbr);
            }
        }
        None
    }
}

/// Iterator over frozen rows, yielding local vertex ids.
pub struct ImmutableCsrIterator<'a> {
    csr: &'a ImmutableCsr,
    ts: Timestamp,
    include_deleted: bool,
    row: usize,
    idx: usize,
}

impl<'a> ImmutableCsrIterator<'a> {
    fn new(csr: &'a ImmutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            include_deleted: false,
            row: 0,
            idx: 0,
        }
    }

    fn new_all(csr: &'a ImmutableCsr) -> Self {
        Self {
            csr,
            ts: 0,
            include_deleted: true,
            row: 0,
            idx: 0,
        }
    }
}

impl<'a> Iterator for ImmutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.row < self.csr.degrees.len() {
            let end = self.csr.offsets[self.row] as usize + self.csr.degrees[self.row] as usize;
            while self.idx < end {
                let nbr = self.csr.slot_at(self.idx).unwrap();
                self.idx += 1;
                if self.include_deleted || nbr.is_alive_at(self.ts) {
                    return Some((VertexId::from_int64(self.row as i64), nbr));
                }
            }
            self.row += 1;
            if self.row < self.csr.offsets.len() {
                self.idx = self.csr.offsets[self.row] as usize;
            }
        }
        None
    }
}

use super::super::{EdgePosition, Nbr};
use super::MappedFrozen;
use graphdb_core::types::{EdgeId, Timestamp, VertexId};

impl MappedFrozen {
    /// Iterate timestamp-visible entries across all rows without materializing
    /// any row: each item decodes one slot on demand from the mapping.
    pub fn iter(&self, ts: Timestamp) -> MappedFrozenIterator {
        MappedFrozenIterator::filtered(self.clone(), ts)
    }

    /// Iterate every physically stored entry, including tombstoned ones.
    pub fn iter_all(&self) -> MappedFrozenIterator {
        MappedFrozenIterator::all(self.clone())
    }

    /// Iterate timestamp-visible entries of one row without allocating.
    ///
    /// Detached-snapshot contract: the serving file embeds the full
    /// create/delete stamp history, so point-in-time reads inside the
    /// snapshot are decided from the embedded stamps alone, without the
    /// table version authority. Live tables must never use this path for
    /// visibility; they decide through `EdgeStore::is_visible`.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> MappedFrozenRowIter {
        let (start, end) = self.row_window(src_vid).unwrap_or((0, 0));
        MappedFrozenRowIter {
            view: self.clone(),
            idx: start,
            end,
            ts,
        }
    }
}

/// Owned per-row iterator over a mapped view: holds a counted handle instead
/// of borrowed slices, so the mapping stays alive for the whole walk and no
/// row is ever materialized.
#[derive(Debug, Clone)]
pub struct MappedFrozenRowIter {
    view: MappedFrozen,
    idx: usize,
    end: usize,
    ts: Timestamp,
}

impl Iterator for MappedFrozenRowIter {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.end {
            let nbr = self.view.slot_at(self.idx)?;
            self.idx += 1;
            if nbr.is_alive_at(self.ts) {
                return Some(nbr);
            }
        }
        None
    }
}

/// Owned whole-table iterator over a mapped view, yielding local vertex ids.
/// Timestamp-filtered unless built for the physical walk.
#[derive(Debug, Clone)]
pub struct MappedFrozenIterator {
    view: MappedFrozen,
    ts: Timestamp,
    include_deleted: bool,
    row: usize,
    idx: usize,
}

impl MappedFrozenIterator {
    fn filtered(view: MappedFrozen, ts: Timestamp) -> Self {
        let start = view.offsets.first().copied().unwrap_or(0) as usize;
        Self {
            view,
            ts,
            include_deleted: false,
            row: 0,
            idx: start,
        }
    }

    fn all(view: MappedFrozen) -> Self {
        let mut iter = Self::filtered(view, 0);
        iter.include_deleted = true;
        iter
    }
}

impl Iterator for MappedFrozenIterator {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.row < self.view.rows {
            let end = self.view.offsets[self.row] as usize + self.view.degree_at(self.row) as usize;
            while self.idx < end {
                let nbr = self.view.slot_at(self.idx)?;
                self.idx += 1;
                if self.include_deleted || nbr.is_alive_at(self.ts) {
                    return Some((VertexId::from_int64(self.row as i64), nbr));
                }
            }
            self.row += 1;
            if self.row < self.view.rows {
                self.idx = self.view.offsets[self.row] as usize;
            }
        }
        None
    }
}

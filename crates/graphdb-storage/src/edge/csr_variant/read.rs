use super::super::{HotNbr, MutableCsrTrait, Nbr, Timestamp};
use super::{CsrIterator, CsrRowIter, CsrVariant};

impl CsrVariant {
    /// Iterate edges of a vertex without allocating, for every strategy.
    ///
    /// Test-only row-stamp filtered iterator; production scans go through
    /// the version authority. Missing groups and the `None` strategy yield
    /// no iterator. Bundled rows yield topology only: pair each item with
    /// the value accessors instead of reading values from this walk.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> Option<CsrRowIter<'_>> {
        // Pure rows store no timestamps, so the timestamp carries no
        // information there; the borrowed walk yields live entries directly.
        let _ = ts;
        match self {
            CsrVariant::Multiple(csr) => Some(CsrRowIter::Multiple(csr.iter_edges_of(src_vid, ts))),
            CsrVariant::Single(csr) => Some(CsrRowIter::Single(
                csr.iter_edges_of(src_vid, ts).into_iter(),
            )),
            CsrVariant::Pure(csr) => Some(CsrRowIter::Pure(csr.iter_row(src_vid))),
            CsrVariant::Bundled(csr) => Some(CsrRowIter::Bundled(csr.iter_row(src_vid))),
            CsrVariant::Frozen(csr) => Some(CsrRowIter::Frozen(csr.iter_edges_of(src_vid, ts))),
            CsrVariant::Mapped(csr) => Some(CsrRowIter::Mapped(csr.iter_edges_of(src_vid, ts))),
            CsrVariant::None { .. } => None,
        }
    }

    /// Iterate over all physically present entries, including tombstoned ones.
    ///
    /// Used when rebuilding the CSR (e.g. vertex ID remapping) so entries
    /// marked as deleted survive the rebuild. Pure and bundled holes carry
    /// the unassignable sentinel instead of an edge, so they are skipped:
    /// no rebuild may resurrect a hole as a live edge. Bundled values are
    /// not yielded here; migration paths resolve them through the paired
    /// value entry.
    pub fn iter_all(&self) -> CsrIterator<'_> {
        match self {
            CsrVariant::Multiple(csr) => CsrIterator::Multiple(csr.iter_all()),
            CsrVariant::Single(csr) => CsrIterator::Single(csr.iter_all()),
            CsrVariant::Pure(csr) => CsrIterator::Pure(csr.iter_all()),
            CsrVariant::Bundled(csr) => CsrIterator::Bundled(csr.iter_all()),
            CsrVariant::Frozen(csr) => CsrIterator::Frozen(csr.iter_all()),
            CsrVariant::Mapped(csr) => CsrIterator::Mapped(csr.iter_all()),
            CsrVariant::None { .. } => CsrIterator::None,
        }
    }

    /// Visit every physically stored entry of one vertex without allocating.
    ///
    /// Bundled rows visit topology only; use `visit_physical_with_values`
    /// when the inline value is needed.
    pub fn visit_physical<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        match self {
            CsrVariant::Multiple(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Single(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Pure(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Bundled(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Frozen(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Mapped(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::None { .. } => {}
        }
    }

    /// Visit every physically stored hot half of one vertex without
    /// allocating and without touching the stamp lines.
    ///
    /// Hot-only counterpart of [`Self::visit_physical`] for traversals that
    /// resolve visibility through the version authority by `edge_id`.
    pub fn visit_hot<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(HotNbr) -> bool,
    {
        match self {
            CsrVariant::Multiple(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Single(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Pure(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Bundled(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Frozen(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Mapped(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::None { .. } => {}
        }
    }

    /// Fill a caller buffer with every physically stored entry of one vertex.
    ///
    /// Same content as the allocating trait accessor, without the per-vertex
    /// allocation. Batch scans reuse one buffer across vertices. Behavior is
    /// uniform across variants: the buffer is cleared first, then the row is
    /// appended in the variant's promised order (insertion order for mutable
    /// forms, sorted order for frozen forms; see the enum-level contract).
    /// Bundled rows fill topology only; pair with the value accessors.
    pub fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        match self {
            CsrVariant::Multiple(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Single(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Pure(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Bundled(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Frozen(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Mapped(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::None { .. } => out.clear(),
        }
    }

    /// Whether the live entries of one row arrive in key order.
    ///
    /// Only frozen, mapped and single-slot rows promise order and may use
    /// bisection. All other forms report an observation that callers must
    /// not cache across restarts: the flag is memory-only, rebuilt on load,
    /// and query planning falls back to a linear walk unless the promise
    /// holds for the row's current variant.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Single(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Pure(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Bundled(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Frozen(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Mapped(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::None { .. } => true,
        }
    }

    /// Visit live entries whose `(endpoint, rank)` key falls in the inclusive
    /// `[lower, upper]` range. Pure and bundled rows carry no rank, so the
    /// rank halves of the bounds are explicitly ignored there: callers pass
    /// endpoint intervals, and reusing one binary interval across forms
    /// scans the wider endpoint range on these forms by contract rather than
    /// by silent widening.
    pub fn visit_threshold<F>(
        &self,
        src_vid: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        f: F,
    ) where
        F: FnMut(Nbr) -> bool,
    {
        match self {
            CsrVariant::Multiple(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::Single(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::Pure(csr) => {
                csr.visit_threshold(src_vid, lower.map(|(ep, _)| ep), upper.map(|(ep, _)| ep), f)
            }
            CsrVariant::Bundled(csr) => {
                csr.visit_threshold(src_vid, lower.map(|(ep, _)| ep), upper.map(|(ep, _)| ep), f)
            }
            CsrVariant::Frozen(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::Mapped(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::None { .. } => {}
        }
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    ///
    /// Same explicit rank-half rule as `visit_threshold` on pure and bundled
    /// rows.
    pub fn fill_threshold_into(
        &self,
        src_vid: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        self.visit_threshold(src_vid, lower, upper, |nbr| {
            out.push(nbr);
            true
        });
    }
}

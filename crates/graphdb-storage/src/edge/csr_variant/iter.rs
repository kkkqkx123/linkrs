use super::super::mutable_csr::VertexEdgesIter;
use super::super::{
    FrozenRowIter, ImmutableCsrIterator, MappedFrozenIterator, MappedFrozenRowIter,
    MutableCsrIterator, Nbr, PureAllIter, PureRowIter, SingleMutableCsrIterator, VertexId,
};

/// Iterator over CSR edges, supporting multiple implementation types
pub enum CsrIterator<'a> {
    /// Iterator over multi-edge CSR
    Multiple(MutableCsrIterator<'a>),
    /// Iterator over single-edge CSR
    Single(SingleMutableCsrIterator<'a>),
    /// Iterator over pure topology CSR
    Pure(PureAllIter<'a>),
    /// Iterator over bundled CSR
    Bundled(PureAllIter<'a>),
    /// Iterator over frozen packed CSR
    Frozen(ImmutableCsrIterator<'a>),
    /// Iterator over memory-mapped frozen CSR (owns its mapping handle)
    Mapped(MappedFrozenIterator),
    /// Empty iterator
    None,
}

impl<'a> Iterator for CsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            CsrIterator::Multiple(iter) => iter.next(),
            CsrIterator::Single(iter) => iter.next(),
            CsrIterator::Pure(iter) => iter.next(),
            CsrIterator::Bundled(iter) => iter.next(),
            CsrIterator::Frozen(iter) => iter.next(),
            CsrIterator::Mapped(iter) => iter.next(),
            CsrIterator::None => None,
        }
    }
}

/// Borrowed per-vertex edge iterator covering every CSR strategy.
///
/// Allocation-free counterpart of `edges_of` at the variant level: each arm
/// wraps the strategy's own iterator, so callers above the variant (shard
/// sets, table scans) iterate one row without materializing a vector,
/// regardless of the underlying layout. Records are assembled by value
/// because the split halves live in separate slices.
///
/// Bundled rows yield topology only through this iterator: the inline value
/// column stays unread. Callers needing values must use the paired value
/// entry (`visit_physical_with_values` or the `bundled_value_*` accessors)
/// instead of this walk alone, otherwise values are silently dropped.
pub enum CsrRowIter<'a> {
    /// Multi-edge row: primary block plus overflow chain walk.
    Multiple(VertexEdgesIter<'a>),
    /// Single-edge row: at most one assembled slot.
    Single(std::option::IntoIter<Nbr>),
    /// Pure topology row: borrowed primary plus overflow walk.
    Pure(PureRowIter<'a>),
    /// Bundled row: borrowed topology walk only. Values are not yielded;
    /// resolve them through the paired value entry per edge.
    Bundled(PureRowIter<'a>),
    /// Frozen row: filtered packed-slice walk.
    Frozen(FrozenRowIter<'a>),
    /// Mapped frozen row: on-demand decode walk owning its mapping handle.
    Mapped(MappedFrozenRowIter),
}

impl<'a> Iterator for CsrRowIter<'a> {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            CsrRowIter::Multiple(iter) => iter.next(),
            CsrRowIter::Single(iter) => iter.next(),
            CsrRowIter::Pure(iter) => iter.next(),
            CsrRowIter::Bundled(iter) => iter.next(),
            CsrRowIter::Frozen(iter) => iter.next(),
            CsrRowIter::Mapped(iter) => iter.next(),
        }
    }
}

//! Frozen immutable CSR topology.
//!
//! Packed read-only form of one group's adjacency: a single contiguous
//! neighbor segment plus a per-row degree table. Empty rows hold no slots.
//! There is no capacity array, no overflow chain, no live index and no lock:
//! a frozen group pays only its entries plus two small per-row arrays.
//!
//! A frozen group keeps every neighbor byte of the mutable group it was
//! packed from, including edge ids, delete timestamps and tombstones, except
//! reserved-slot gap sentinels (`INVALID_EDGE_ID` fillers), which carry no
//! edge and are dropped at pack time. Freezing changes the physical layout
//! and the row order, so timestamp-filtered reads observe the same logical
//! content before and after, not the same byte order. Visibility authority
//! stays above this layer; row stamps are physical replicas as in the
//! mutable form.
//!
//! Every packed row is sorted by `(endpoint, rank, edge_id)`.
//! Endpoint and rank keep one point-query key contiguous, and edge ids make
//! the order total. Point queries bisect the key range and return the first
//! timestamp-visible version inside it. Scans stay linear over the sorted
//! rows.
//!
//! A frozen group packed from a valued bundled group additionally carries
//! the inline value column: `values`/`valid` run slot-parallel to the packed
//! halves in the same row order, so freezing a valued bundled group preserves
//! its properties without migrating to the columnar form. Groups packed from
//! any other source hold no value columns and read every value as NULL.
//!
//! Row offsets are rebuilt in memory on open and on load, never persisted.
//! Every mutating entry point rejects writes: a frozen group must be
//! explicitly unfrozen back into a mutable variant before it accepts writes
//! again. There is no implicit unfreeze on the write path.

use super::{ColdStamps, HotNbr};
use bitvec::order::Lsb0;
use bitvec::vec::BitVec;

pub(crate) mod iter;
pub(crate) mod maintenance;
pub(crate) mod pack;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod trait_impl;

#[cfg(test)]
mod tests;

pub use iter::{FrozenRowIter, ImmutableCsrIterator};

/// Packed immutable adjacency of one group.
///
/// `hot_entries`/`cold_entries` hold every row back to back in row order,
/// each row sorted by `(endpoint, rank, edge_id)`;
/// `degrees[row]` is the row length and `offsets[row]` its start inside the
/// halves. Empty rows contribute no slots. `offsets` is memory-only state
/// rebuilt by packing and by loading.
///
/// `values`/`valid` carry the bundled inline value column slot-parallel to
/// the packed halves when the group was packed from a valued bundled group;
/// otherwise both stay empty and every value reads as NULL.
#[derive(Debug, Clone)]
pub struct ImmutableCsr {
    hot_entries: Vec<HotNbr>,
    cold_entries: Vec<ColdStamps>,
    degrees: Vec<u32>,
    offsets: Vec<u32>,
    edge_count: u64,
    values: Vec<u64>,
    valid: BitVec<u8, Lsb0>,
}

impl Default for ImmutableCsr {
    fn default() -> Self {
        Self::new()
    }
}

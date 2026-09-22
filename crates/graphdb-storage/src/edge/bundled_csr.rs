//! Bundled CSR: pure topology plus one inline scalar value column.
//!
//! Extends `PureTopologyCsr` with a `value: u64` column (20 bytes/edge)
//! for single numeric scalar attributes.  Type encoding/decoding is
//! centralized in `encode_scalar`/`decode_scalar`; the row stores only the
//! raw 64-bit representation.  The type is resolved from the published
//! schema at read time, never stored inline.
//!
//! The value column is slot-parallel to the topology: `primary_values` and
//! `primary_valid` run alongside the primary `endpoints`/`edge_ids` block,
//! and each topology overflow chunk has a parallel `BundledOverflowValues`
//! chunk of identical length.  Every topology mutation goes through the
//! shared `PureTopologyCsr` entry (`insert_edge_returning_position`,
//! positioned deletes) or mirrors its slot moves exactly (`rollback_insert`,
//! `compact_vertex_with_reporting`), so the two columns never drift.
//!
//! A deleted slot keeps its stale raw word but clears its validity bit;
//! the positional revert revives the retained word, and the value-blind
//! trait revert follows the same path.
//!
//! Freezing carries the value column slot-parallel into the packed frozen
//! layout, so valued bundled groups freeze and unfreeze with their inline
//! content intact; the mmap serving sidecar mirrors the same columns.
//!
//! Suitability boundary: this form fits exactly one inline scalar attribute
//! on a read-heavy, schema-stable edge type. Anything else (multiple
//! attributes, non-encodable types, online schema changes) belongs to the
//! columnar form, which is also the default.
//! The bundled form is intentionally not extended beyond a single column.
//!
//! Layout by responsibility (`bundled_csr/` subdirectory):
//! - `codec` holds scalar encoding and decoding.
//! - `overflow_values` holds the parallel overflow value chunk.
//! - `core` holds lifecycle and capacity management.
//! - `read` implements physical reads and value getters.
//! - `write` implements value writes and slot sync.
//! - `maintenance` handles sorting and compaction.
//! - `persistence` handles dump, load and encoding reports.
//! - `trait_impl` adapts the type to CSR traits.

use graphdb_core::types::EdgeId;

use super::csr_shared::SegmentedTable;
use super::pure_csr::PureTopologyCsr;
use bitvec::order::Lsb0;
use bitvec::vec::BitVec;

pub(crate) mod codec;
pub(crate) mod core;
pub(crate) mod maintenance;
pub(crate) mod overflow_values;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod trait_impl;
pub(crate) mod write;

#[cfg(test)]
mod tests;

pub use codec::{decode_scalar, encode_scalar};
pub(crate) use overflow_values::BundledOverflowValues;

const INVALID_EDGE_ID: EdgeId = EdgeId(u64::MAX);

/// Bundled CSR: pure topology + one inline `value: u64` column.
///
/// The value column never participates in dedup or routing; it strictly
/// follows the topology slots.
#[derive(Debug, Clone)]
pub struct BundledCsr {
    /// Underlying pure topology (endpoint + edge_id columns).
    topology: PureTopologyCsr,
    /// Per-slot value column, parallel to the topology primary block.
    primary_values: Vec<u64>,
    /// Per-slot validity bitmap, parallel to the topology primary block.
    /// One bit per slot: true means the slot holds a valid value, false
    /// means NULL. Bitmap form replaces the former byte-per-slot vector so
    /// wide tables pay one bit, not one byte, per edge.
    primary_valid: BitVec<u8, Lsb0>,
    /// Per-overflow-chunk value columns, keyed by vertex like the topology
    /// overflow table with identical chunk counts and lengths.
    overflow_values: SegmentedTable<Vec<BundledOverflowValues>>,
}

impl BundledCsr {
    /// Number of valid (non-NULL) inline values across primary and overflow.
    pub fn valid_value_count(&self) -> usize {
        let mut count = self.primary_valid.count_ones();
        for (_, chunks) in self.overflow_values.iter() {
            for chunk in chunks.iter() {
                count += chunk.valid.count_ones();
            }
        }
        count
    }

    /// Validity bitmap length in slots, for observability and tests.
    pub fn validity_len(&self) -> usize {
        self.primary_valid.len()
    }
}

impl Default for BundledCsr {
    fn default() -> Self {
        Self::new()
    }
}

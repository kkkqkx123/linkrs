//! CSR Variant
//!
//! Enum wrapper for different CSR implementations (mutable).
//! Provides runtime polymorphism without dynamic dispatch (dyn).
//!
//! # CSR Type Selection
//!
//! The `EdgeStrategy` enum determines which CSR implementation to use:
//! - `Multiple`: Standard `MutableCsr` for general multi-edge scenarios
//! - `Single`: `SingleMutableCsr` for one-edge-per-vertex (O(1) access)
//! - `None`: No edges stored
//!
//! # Variants
//!
//! - `CsrVariant::Multiple`: Mutable CSR with dynamic capacity growth
//! - `CsrVariant::Single`: Mutable single-edge CSR
//! - `CsrVariant::Frozen`: Packed immutable CSR, writes rejected until unfrozen
//! - `CsrVariant::Mapped`: Memory-mapped frozen CSR, same read-only semantics
//! - `CsrVariant::None`: Placeholder for relationships with no edges
//!
//! Layout by responsibility (`csr_variant/` subdirectory):
//! - `core` holds construction, clearing and stats.
//! - `persistence` handles dump, load and scratch dumps.
//! - `trait_impl` adapts the type to CSR traits.
//! - `read` implements iterators, visits and range scans.
//! - `maintenance` handles compaction and sorting.
//! - `values` implements bundled inline-value helpers.
//! - `iter` holds iterator enums and their impls.

use super::bundled_csr::BundledCsr;
use super::pure_csr::PureTopologyCsr;
use super::{ImmutableCsr, MappedFrozen, MutableCsr, SingleMutableCsr};

/// Macro for dispatching method calls to the underlying CSR variant.
///
/// Expands to a match statement with proper None handling. Mutability lives
/// with the receiver, so this single macro serves both mutable and immutable
/// call sites; a second macro would only duplicate the match below.
///
/// # Usage
///
/// - Method with no arguments and return value with default:
///   `dispatch!(self, method() -> default_value)`
/// - Method with arguments and return value with default:
///   `dispatch!(self, method(arg1, arg2) -> default_value)`
///
/// # Examples
///
/// ```ignore
/// let result = dispatch!(self, insert_edge(vid, dst, id, ts) -> false);
/// let result = dispatch!(self, edges_of(vid, ts) -> Vec::new());
/// ```
macro_rules! dispatch {
    // Method with arguments and return value with default for None
    ($self:expr, $method:ident($($arg:expr),+ $(,)?) -> $default:expr) => {
        match $self {
            CsrVariant::Multiple(csr) => csr.$method($($arg),+),
            CsrVariant::Single(csr) => csr.$method($($arg),+),
            CsrVariant::Pure(csr) => csr.$method($($arg),+),
            CsrVariant::Bundled(csr) => csr.$method($($arg),+),
            CsrVariant::Frozen(csr) => csr.$method($($arg),+),
            CsrVariant::Mapped(csr) => csr.$method($($arg),+),
            CsrVariant::None { .. } => $default,
        }
    };

    // Method with no arguments and return value with default for None
    ($self:expr, $method:ident() -> $default:expr) => {
        match $self {
            CsrVariant::Multiple(csr) => csr.$method(),
            CsrVariant::Single(csr) => csr.$method(),
            CsrVariant::Pure(csr) => csr.$method(),
            CsrVariant::Bundled(csr) => csr.$method(),
            CsrVariant::Frozen(csr) => csr.$method(),
            CsrVariant::Mapped(csr) => csr.$method(),
            CsrVariant::None { .. } => $default,
        }
    };
}

pub(crate) mod core;
pub(crate) mod iter;
pub(crate) mod maintenance;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod trait_impl;
pub(crate) mod values;

#[cfg(test)]
mod tests;

pub use iter::{CsrIterator, CsrRowIter};

/// Polymorphic CSR wrapper supporting multiple implementation strategies.
///
/// Combines mutable CSR implementations into a single enum for runtime
/// selection without generic monomorphization.
///
/// The enum dispatch via `CsrVariant` keeps one implementation per trait
/// method; callers use the trait interface so behavior stays in one place.
///
/// Cross-layer traversal contract: row positions (`EdgePosition` chunk/slot
/// pairs, primary-block offsets, overflow indices) are variant-local and
/// never cross a layer boundary. Every handoff between layers (read to
/// write, scan to point lookup, live table to migration) re-resolves the
/// target through the edge-id key first; a position obtained from one
/// variant is never interpreted by another.
///
/// # Row-view and ordering contract
///
/// All variants promise one shared row-view semantic: a row walk yields the
/// physically stored entries of one vertex, each exactly once, as assembled
/// `Nbr` records. Gap sentinels are excluded by every walk; tombstones are
/// included so reclaim and audit paths observe them. Visibility is decided
/// by the version authority above, never by these walks. Three access shapes
/// serve it with unified naming: the borrowed [`Self::visit_physical`] walk
/// (no allocation, preferred for inline forms and hot scans), the zero-alloc
/// [`Self::fill_physical_into`] caller-buffer fill (preferred for batch
/// scans), and the allocating `physical_edges_of` accessor (test and offline
/// use only). No new traversal dialect may be added per variant; new needs
/// go through these three.
///
/// Ordering is promised per form, never globally: mutable, single, pure and
/// bundled rows are insertion-ordered and promise no order; frozen and
/// mapped rows are packed sorted by `(endpoint, rank, edge_id)` and promise
/// that order plus key-interval bisection. Freeze, compaction, compression
/// and snapshot rebuilds may change the order; the query layer must never depend on an
/// unpromised order. [`MutableCsr::is_row_sorted`](super::MutableCsr::is_row_sorted)
/// reports the advisory per-row state for plan selection.
///
/// Mapped rows hold their mapping by value (`Arc` inside the iterator), so a
/// row walk stays valid across group replacement; the walk still yields the
/// snapshot it was created from, never the replaced group.
#[derive(Debug, Clone)]
pub enum CsrVariant {
    /// Multi-edge mutable CSR: each vertex can have multiple outgoing edges
    Multiple(Box<MutableCsr>),
    /// Single-edge mutable CSR: each vertex has at most one outgoing edge
    Single(SingleMutableCsr),
    /// Pure topology CSR: 12 bytes/edge, no rank, no timestamps
    Pure(Box<PureTopologyCsr>),
    /// Bundled CSR: 20 bytes/edge, inline single scalar value column
    Bundled(Box<BundledCsr>),
    /// Frozen packed CSR: read-only until explicitly unfrozen
    Frozen(Box<ImmutableCsr>),
    /// Memory-mapped frozen CSR: same read-only content as `Frozen`
    Mapped(Box<MappedFrozen>),
    /// No-edge placeholder: vertices exist but have no outgoing edges
    None { vertex_capacity: usize },
}

//! Mutable CSR Implementation
//!
//! Two-level CSR with fixed-size overflow chunks for stable append cost.
//! Primary blocks are stored contiguously in `nbr_list` (flat CSR layout).
//! Each overflow allocation adds one chunk and never copies an existing chunk. This keeps
//! high-degree vertex growth linear and avoids the repeated doubling/copying behavior that
//! previously produced unreachable blocks in the primary neighbor array.
//!
//! # Zero-Degree Rows
//!
//! Primary blocks are allocated lazily on the first edge of a vertex. A vertex without
//! edges holds no slots in `nbr_list`, and overflow chunks are addressed by dense
//! row subscript. This keeps the per-row fixed cost to 12 bytes
//! (offset + degree + capacity) with no per-row hashing.
//!
//! Layout by responsibility (`mutable_csr/` subdirectory)://! - `core` holds lifecycle and capacity management.
//! - `live_set` maintains the live endpoint index.
//! - `row` handles density tiering, gaps and rebalance.
//! - `write` implements the mutation path.
//! - `read` implements timestamped and physical reads.
//! - `iter` holds iterators and their constructors.
//! - `persistence` handles dump, load and encoding reports.
//! - `compaction` reclaims eligible tombstones.
//! - `stats` reports memory and fragmentation.
//! - `trait_impl` adapts the type to CSR traits.
//! - `overflow` and `serialization` stay as storage primitives.

use std::fmt;

use super::{Nbr, Timestamp};

use live_set::LiveSetStorage;

pub(crate) mod compaction;
pub(crate) mod core;
pub mod iter;
pub(crate) mod live_set;
pub mod overflow;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod row;
pub mod serialization;
pub(crate) mod stats;
pub(crate) mod trait_impl;
pub(crate) mod write;

#[cfg(test)]
mod tests;

pub use iter::{MutableCsrIterator, VertexEdgesIter};
pub use overflow::OverflowStorage;
pub use write::EdgePosition;

pub(crate) use row::PACKED_CSR_DENSITY;

pub struct MutableCsr {
    nbr_list: Vec<Nbr>,
    adj_offsets: Vec<u32>,
    degrees: Vec<u32>,
    primary_capacities: Vec<u32>,

    overflow_chunks: OverflowStorage,
    overflow_chunk_edges: usize,
    /// Single live-endpoint set per vertex covering primary and overflow:
    /// (endpoint, rank) of edges whose `delete_ts == MAX`. One set replaces
    /// the former primary/overflow pair, so duplicate checks never consult
    /// two heaps and never fall back to linear scans. Narrow rows hold a
    /// sorted inline array, wide rows a hash set; see `live_set`. Rows are
    /// addressed by dense subscript; empty rows hold no set.
    live_sets: LiveSetStorage,
    /// Watermark-derived cutoff for hot-path tombstone reuse, refreshed by
    /// the table maintenance pass. The sentinel disables reuse. Memory-only:
    /// never persisted, rebuilt to the default on construction.
    tombstone_reuse_cutoff: Timestamp,

    edge_count: u64,
    total_edge_capacity: usize,
}

impl Clone for MutableCsr {
    fn clone(&self) -> Self {
        Self {
            nbr_list: self.nbr_list.clone(),
            adj_offsets: self.adj_offsets.clone(),
            degrees: self.degrees.clone(),
            primary_capacities: self.primary_capacities.clone(),
            overflow_chunks: self.overflow_chunks.clone(),
            overflow_chunk_edges: self.overflow_chunk_edges,
            live_sets: self.live_sets.clone(),
            tombstone_reuse_cutoff: self.tombstone_reuse_cutoff,
            edge_count: self.edge_count,
            total_edge_capacity: self.total_edge_capacity,
        }
    }
}

impl fmt::Debug for MutableCsr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutableCsr")
            .field("vertex_capacity", &self.vertex_capacity())
            .field("total_edge_capacity", &self.total_edge_capacity)
            .field("edge_count", &self.edge_count)
            .finish_non_exhaustive()
    }
}

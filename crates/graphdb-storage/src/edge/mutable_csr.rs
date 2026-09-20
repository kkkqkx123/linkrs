//! Mutable CSR Implementation
//!
//! Two-level CSR with fixed-size overflow chunks for stable append cost.
//! Primary blocks are stored contiguously in `hot_list`/`cold_list`
//! (flat CSR layout with split topology and timestamp halves).
//! Each overflow allocation adds one chunk and never copies an existing chunk. This keeps
//! high-degree vertex growth linear and avoids the repeated doubling/copying behavior that
//! previously produced unreachable blocks in the primary neighbor array.
//!
//! # Zero-Degree Rows
//!
//! Primary blocks are allocated lazily on the first edge of a vertex. A vertex without
//! edges holds no slots in the primary halves, and overflow chunks plus live-key
//! sets are addressed by segmented sparse row index (only touched segments
//! allocate). This keeps the per-row fixed cost to 12 bytes
//! (offset + degree + capacity) with no per-row hashing.
//!
//! Layout by responsibility (`mutable_csr/` subdirectory):
//! - `core` holds lifecycle and capacity management.
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
//!
//! # Concurrency contract (canonical)
//!
//! This section is the single authority for CSR write discipline; the
//! sharded container (`node_group`) and the table (`edge_table::core`) only
//! reference it and add no second set of rules.
//!
//! - Single writer: `MutableCsr` carries no internal locks. Every mutation
//!   takes `&mut self`, so aliased writes are rejected by the borrow checker
//!   at compile time, never detected at runtime.
//! - Concurrent readers are safe while no mutation is in flight. The type is
//!   `Send + Sync`, so shared references may cross threads; obtaining a
//!   `&mut` while readers exist is again a compile-time error.
//! - Caller serialization: the table or transaction layer serializes writers.
//!   Vertex-level locking is a caller decision, not a property of this struct.
//! - The only runtime-shared mutable state below this level is the sharded
//!   route cache, built from atomics: a stale entry costs a re-lookup, never
//!   a wrong answer. Debug builds cross-check cached routes against the group
//!   map so cache-coherence violations fail fast instead of hiding.
//! - No new lock module exists. If write concurrency is ever needed, per-row
//!   locking must be evaluated against a contention benchmark first; a global
//!   lock is not an acceptable default.

use std::fmt;

use super::csr_shared::VertexBookkeeping;
use super::{ColdStamps, HotNbr, Nbr, Timestamp};

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
pub use overflow::{OverflowChunk, OverflowStorage};
pub use write::EdgePosition;

pub(crate) use row::PACKED_CSR_DENSITY;

/// Mutable multi-edge CSR with lazy primary blocks and overflow chains.
///
/// Single-writer discipline: this type carries no internal locks. Concurrent
/// readers are safe while no mutation is in flight; concurrent writers must
/// be serialized by the caller (table or transaction layer). Vertex-level
/// locking is a caller decision, not a property of this struct.
pub struct MutableCsr {
    hot_list: Vec<HotNbr>,
    cold_list: Vec<ColdStamps>,
    rows: VertexBookkeeping,

    overflow_chunks: OverflowStorage,
    overflow_chunk_edges: usize,
    /// Single live-endpoint set per wide vertex covering primary and overflow:
    /// (endpoint, rank) of edges whose `delete_ts == MAX`. One set replaces
    /// the former primary/overflow pair, so duplicate checks never consult
    /// two heaps and never fall back to linear scans on indexed rows.
    /// Only rows wider than the live-set bound carry a set; narrow rows
    /// answer through direct row scans with no per-vertex memory. Rows are
    /// addressed by dense subscript; empty and narrow rows hold no set.
    live_sets: LiveSetStorage,
    /// Watermark-derived cutoff for hot-path tombstone reuse, refreshed by
    /// the table maintenance pass. The sentinel disables reuse. Memory-only:
    /// never persisted, rebuilt to the default on construction.
    tombstone_reuse_cutoff: Timestamp,
    /// First known reclaimable primary slot per vertex, or unknown sentinel.
    /// Set on primary deletes, consumed on reuse, invalidated by any row move.
    /// Memory-only hint: a stale value only costs one failed probe before the
    /// bounded scan fallback.
    reuse_hint: Vec<u32>,

    /// Incremental live entry count per vertex (narrow rows only).
    /// Indexed rows track membership via the live set instead.
    live_counts: Vec<u32>,
    /// Incremental tombstone count per vertex (narrow rows only).
    tombstone_counts: Vec<u32>,

    edge_count: u64,
    total_edge_capacity: usize,

    // -- baseline instrumentation probes (memory-only, not persisted) --
    overflow_chunk_allocs: u64,
    primary_block_allocs: u64,
    repack_count: u64,
    tombstone_reuse_count: u64,
    live_set_rebuild_count: u64,
    vertex_expansion_count: u64,
}

impl MutableCsr {
    /// Assembled slot copy at a primary-list index.
    ///
    /// Hot and cold halves grow in lockstep, so one index addresses both.
    /// Callers that only need one half must read it directly instead of
    /// paying for the assembly here.
    #[inline]
    pub(crate) fn slot_at(&self, idx: usize) -> Option<Nbr> {
        Some(Nbr::from_parts(
            *self.hot_list.get(idx)?,
            *self.cold_list.get(idx)?,
        ))
    }

    /// Write an assembled record into a primary-list index.
    ///
    /// Sole paired writer for the primary halves: hot and cold stay in
    /// lockstep by construction instead of by caller discipline.
    #[inline]
    pub(crate) fn set_slot(&mut self, idx: usize, nbr: Nbr) {
        self.hot_list[idx] = nbr.hot();
        self.cold_list[idx] = nbr.cold();
        debug_assert_eq!(self.hot_list.len(), self.cold_list.len());
    }

    /// Cold half at a primary-list index without touching topology lines.
    #[inline]
    pub(crate) fn cold_at(&self, idx: usize) -> Option<ColdStamps> {
        self.cold_list.get(idx).copied()
    }
}

impl Clone for MutableCsr {
    fn clone(&self) -> Self {
        Self {
            hot_list: self.hot_list.clone(),
            cold_list: self.cold_list.clone(),
            rows: self.rows.clone(),
            overflow_chunks: self.overflow_chunks.clone(),
            overflow_chunk_edges: self.overflow_chunk_edges,
            live_sets: self.live_sets.clone(),
            tombstone_reuse_cutoff: self.tombstone_reuse_cutoff,
            reuse_hint: self.reuse_hint.clone(),
            live_counts: self.live_counts.clone(),
            tombstone_counts: self.tombstone_counts.clone(),
            edge_count: self.edge_count,
            total_edge_capacity: self.total_edge_capacity,
            overflow_chunk_allocs: self.overflow_chunk_allocs,
            primary_block_allocs: self.primary_block_allocs,
            repack_count: self.repack_count,
            tombstone_reuse_count: self.tombstone_reuse_count,
            live_set_rebuild_count: self.live_set_rebuild_count,
            vertex_expansion_count: self.vertex_expansion_count,
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

//! Pure Topology CSR
//!
//! Minimal CSR storing only `(endpoint: u32, edge_id: u64)` per edge (12
//! bytes/edge).  No rank (always 0), no timestamps.  Physical deletion
//! overwrites `edge_id` with the `INVALID_EDGE_ID` sentinel; the endpoint
//! slot remains so downstream position references stay valid.
//!
//! # Design
//!
//! The layout mirrors [`super::mutable_csr::MutableCsr`] in structure but
//! strips every column that the topology-only use-case does not need:
//!
//! * **Primary block** - flat `endpoints: Vec<u32>` + `edge_ids: Vec<u64>`
//!   arrays with per-vertex `adj_offsets`, `degrees` and
//!   `primary_capacities` bookkeeping.
//! * **Overflow chunks** - [`PureOverflowChunk`] SoA pairs of
//!   `endpoints + edge_ids` stored in a [`SegmentedTable`] keyed by vertex.
//! * **Live endpoint set** - [`PureLiveKeySet`] keyed by endpoint only
//!   (rank is always 0) with a width bound of [`LIVE_SET_WIDTH_BOUND`];
//!   narrow rows scan instead of allocating a set.
//!
//! Reads assemble [`Nbr`] on the fly with `rank = 0`,
//! `delete_ts = Timestamp::MAX`.  No MVCC state is stored or checked.
//!
//! Single-writer discipline: this type carries no internal locks. Concurrent
//! reads are safe while no mutation is in flight; concurrent writers must be
//! serialized by the caller.

use graphdb_core::types::EdgeId;

use super::csr_shared::VertexBookkeeping;

pub(crate) mod core;
pub(crate) mod iter;
pub(crate) mod live_set;
pub(crate) mod maintenance;
pub(crate) mod overflow;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod trait_impl;
pub(crate) mod write;

#[cfg(test)]
mod tests;

pub use iter::{PureAllIter, PureRowIter};
pub(crate) use live_set::PureLiveSetStorage;
pub(crate) use overflow::{PureOverflowChunk, PureOverflowStorage};

const INVALID_EDGE_ID: EdgeId = EdgeId(u64::MAX);

pub(crate) const DEFAULT_VERTEX_DEGREE: usize = 4;

pub(crate) const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;

pub(crate) const LIVE_SET_WIDTH_BOUND: usize = 8;

pub struct PureTopologyCsr {
    pub(crate) rows: VertexBookkeeping,
    pub(crate) endpoints: Vec<u32>,
    pub(crate) edge_ids: Vec<u64>,
    pub(crate) overflow_chunks: PureOverflowStorage,
    pub(crate) overflow_chunk_edges: usize,
    pub(crate) live_sets: PureLiveSetStorage,
    pub(crate) edge_count: u64,
    pub(crate) total_edge_capacity: usize,
    /// Cached primary key order of one row for threshold scans.
    ///
    /// Same contract as the mutable form: true only when the primary
    /// window is known to arrive in key order, false always falls back
    /// to the linear scan. Memory-resident only, never persisted.
    pub(crate) primary_sorted: Vec<bool>,
}

impl Clone for PureTopologyCsr {
    fn clone(&self) -> Self {
        Self {
            rows: self.rows.clone(),
            endpoints: self.endpoints.clone(),
            edge_ids: self.edge_ids.clone(),
            overflow_chunks: self.overflow_chunks.clone(),
            overflow_chunk_edges: self.overflow_chunk_edges,
            live_sets: self.live_sets.clone(),
            edge_count: self.edge_count,
            total_edge_capacity: self.total_edge_capacity,
            primary_sorted: self.primary_sorted.clone(),
        }
    }
}

impl std::fmt::Debug for PureTopologyCsr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PureTopologyCsr")
            .field("vertex_capacity", &self.vertex_capacity())
            .field("total_edge_capacity", &self.total_edge_capacity)
            .field("edge_count", &self.edge_count)
            .finish_non_exhaustive()
    }
}

impl Default for PureTopologyCsr {
    fn default() -> Self {
        Self::new()
    }
}

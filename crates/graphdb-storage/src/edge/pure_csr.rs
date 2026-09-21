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

use std::collections::HashMap;

use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::csr_shared::{
    grown_vertex_capacity, OverflowTable, SegmentedTable, VertexBookkeeping,
    DEFAULT_VERTEX_CAPACITY,
};
use super::csr_trait::{CsrBase, MutableCsrTrait};
use super::{EdgePosition, Nbr};

use crate::persistence::{read_u32_le, read_u64_le};

const INVALID_EDGE_ID: EdgeId = EdgeId(u64::MAX);

pub(crate) const DEFAULT_VERTEX_DEGREE: usize = 4;

pub(crate) const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;

pub(crate) const LIVE_SET_WIDTH_BOUND: usize = 8;

#[derive(Debug, Clone, Default)]
pub(crate) struct PureOverflowChunk {
    pub(crate) endpoints: Vec<u32>,
    pub(crate) edge_ids: Vec<u64>,
}

impl PureOverflowChunk {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        Self {
            endpoints: Vec::with_capacity(cap),
            edge_ids: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.endpoints.len()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    #[inline]
    pub(crate) fn capacity(&self) -> usize {
        self.endpoints.capacity()
    }

    #[inline]
    pub(crate) fn push(&mut self, endpoint: u32, edge_id: EdgeId) {
        self.endpoints.push(endpoint);
        self.edge_ids.push(edge_id.0);
    }

    #[inline]
    pub(crate) fn remove(&mut self, index: usize) {
        self.endpoints.remove(index);
        self.edge_ids.remove(index);
    }

    #[inline]
    pub(crate) fn endpoint_at(&self, index: usize) -> Option<u32> {
        self.endpoints.get(index).copied()
    }

    #[inline]
    pub(crate) fn edge_id_at(&self, index: usize) -> Option<EdgeId> {
        self.edge_ids.get(index).map(|&v| EdgeId(v))
    }

    pub(crate) fn consolidated(endpoints: &[u32], edge_ids: &[u64]) -> Self {
        Self {
            endpoints: endpoints.to_vec(),
            edge_ids: edge_ids.to_vec(),
        }
    }
}

impl super::csr_shared::OverflowChunkSpec for PureOverflowChunk {
    type Slot = (u32, EdgeId);

    fn with_capacity(cap: usize) -> Self {
        PureOverflowChunk::with_capacity(cap)
    }

    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.capacity()
    }

    #[inline]
    fn push_slot(&mut self, slot: (u32, EdgeId)) {
        self.push(slot.0, slot.1);
    }
}

/// Pure-CSR overflow storage: the shared overflow table over
/// endpoint/edge-id chunk halves.
pub(crate) type PureOverflowStorage = OverflowTable<PureOverflowChunk>;

#[derive(Debug, Clone, Default)]
pub(crate) struct PureLiveKeySet {
    positions: HashMap<u32, EdgePosition>,
}

impl PureLiveKeySet {
    fn from_positions(positions: Vec<(u32, EdgePosition)>) -> Self {
        Self {
            positions: positions.into_iter().collect(),
        }
    }

    #[inline]
    fn contains(&self, endpoint: &u32) -> bool {
        self.positions.contains_key(endpoint)
    }

    #[inline]
    fn position(&self, endpoint: &u32) -> Option<EdgePosition> {
        self.positions.get(endpoint).copied()
    }

    #[inline]
    fn len(&self) -> usize {
        self.positions.len()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    #[inline]
    fn insert(&mut self, endpoint: u32, position: EdgePosition) {
        self.positions.insert(endpoint, position);
    }

    #[inline]
    fn remove(&mut self, endpoint: &u32) {
        self.positions.remove(endpoint);
    }

    fn heap_bytes(&self) -> usize {
        self.positions.len() * (std::mem::size_of::<(u32, EdgePosition)>() + 8)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PureLiveSetStorage {
    table: SegmentedTable<PureLiveKeySet>,
    live_rows: usize,
}

impl PureLiveSetStorage {
    fn new() -> Self {
        Self {
            table: SegmentedTable::new(),
            live_rows: 0,
        }
    }

    fn ensure_capacity(&mut self, vertex_capacity: usize) {
        self.table.ensure_capacity(vertex_capacity);
    }

    #[inline]
    fn get(&self, vid: u32) -> Option<&PureLiveKeySet> {
        self.table.get(vid)
    }

    fn insert(&mut self, vid: u32, set: PureLiveKeySet) {
        let slot = self.table.slot_mut(vid);
        if slot.is_none() {
            self.live_rows += 1;
        }
        *slot = Some(set);
    }

    fn remove(&mut self, vid: u32) {
        if self.table.take(vid).is_some() {
            self.live_rows = self.live_rows.saturating_sub(1);
        }
    }

    fn insert_key(&mut self, vid: u32, endpoint: u32, position: EdgePosition) {
        let slot = self.table.slot_mut(vid);
        match slot {
            Some(set) => set.insert(endpoint, position),
            slot @ None => {
                *slot = Some(PureLiveKeySet::from_positions(vec![(endpoint, position)]));
                self.live_rows += 1;
            }
        }
    }

    fn remove_key(&mut self, vid: u32, endpoint: &u32) {
        let emptied = match self.table.get_mut(vid) {
            Some(set) => {
                set.remove(endpoint);
                set.is_empty()
            }
            None => return,
        };
        if emptied {
            self.table.take(vid);
            self.live_rows = self.live_rows.saturating_sub(1);
        }
    }

    fn clear(&mut self) {
        self.table.clear();
        self.live_rows = 0;
    }

    fn heap_bytes_total(&self) -> usize {
        self.table.iter().map(|(_, set)| set.heap_bytes()).sum()
    }

    fn index_bytes(&self) -> usize {
        self.table.table_bytes()
            + self.live_rows * (std::mem::size_of::<u32>() + std::mem::size_of::<PureLiveKeySet>())
    }
}

pub struct PureTopologyCsr {
    pub(crate) rows: VertexBookkeeping,
    pub(crate) endpoints: Vec<u32>,
    pub(crate) edge_ids: Vec<u64>,
    pub(crate) overflow_chunks: PureOverflowStorage,
    pub(crate) overflow_chunk_edges: usize,
    pub(crate) live_sets: PureLiveSetStorage,
    pub(crate) edge_count: u64,
    pub(crate) total_edge_capacity: usize,
}

/// Borrowed walk over one pure-topology row.
///
/// Holds slices of the primary window plus the overflow chain reference, so
/// iteration needs no allocation. Sentinel holes are skipped inline.
#[derive(Debug, Clone, Copy)]
pub struct PureRowIter<'a> {
    csr: &'a PureTopologyCsr,
    primary_endpoints: &'a [u32],
    primary_ids: &'a [u64],
    primary_idx: usize,
    overflow: Option<&'a Vec<PureOverflowChunk>>,
    chunk_idx: usize,
    slot_idx: usize,
}

impl<'a> Iterator for PureRowIter<'a> {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        while self.primary_idx < self.primary_endpoints.len() {
            let endpoint = self.primary_endpoints[self.primary_idx];
            let edge_id = EdgeId(self.primary_ids[self.primary_idx]);
            self.primary_idx += 1;
            if edge_id != INVALID_EDGE_ID {
                return Some(self.csr.make_nbr(endpoint, edge_id));
            }
        }
        let chunks = self.overflow?;
        while self.chunk_idx < chunks.len() {
            let chunk = &chunks[self.chunk_idx];
            while self.slot_idx < chunk.len() {
                let endpoint = chunk.endpoints[self.slot_idx];
                let edge_id = EdgeId(chunk.edge_ids[self.slot_idx]);
                self.slot_idx += 1;
                if edge_id != INVALID_EDGE_ID {
                    return Some(self.csr.make_nbr(endpoint, edge_id));
                }
            }
            self.chunk_idx += 1;
            self.slot_idx = 0;
        }
        None
    }
}

/// Borrowed walk over every live entry of the table without allocating.
///
/// Advances one borrowed row walk at a time, so full-table rebuilds and
/// scans iterate with no intermediate vector regardless of primary versus
/// overflow layout. Sentinel holes are skipped inline by the row walk.
#[derive(Debug, Clone, Copy)]
pub struct PureAllIter<'a> {
    csr: &'a PureTopologyCsr,
    cap: u32,
    vid: u32,
    row: PureRowIter<'a>,
}

impl<'a> Iterator for PureAllIter<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.vid >= self.cap {
                return None;
            }
            if let Some(nbr) = self.row.next() {
                return Some((VertexId::from_int64(self.vid as i64), nbr));
            }
            self.vid += 1;
            if self.vid < self.cap {
                self.row = self.csr.iter_row(self.vid);
            }
        }
    }
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

impl PureTopologyCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY, DEFAULT_VERTEX_DEGREE)
    }

    pub fn with_capacity(vertex_capacity: usize, edge_capacity: usize) -> Self {
        Self::with_overflow_chunk_edges(
            vertex_capacity,
            edge_capacity,
            DEFAULT_OVERFLOW_CHUNK_EDGES,
        )
    }

    pub fn with_overflow_chunk_edges(
        vertex_capacity: usize,
        edge_capacity: usize,
        overflow_chunk_edges: usize,
    ) -> Self {
        let vertex_cap = vertex_capacity.max(1);
        let edge_cap = edge_capacity.max(1);

        Self {
            rows: VertexBookkeeping::with_capacity(vertex_cap),
            endpoints: Vec::with_capacity(edge_cap),
            edge_ids: Vec::with_capacity(edge_cap),
            overflow_chunks: PureOverflowStorage::new(),
            overflow_chunk_edges: overflow_chunk_edges.max(1),
            live_sets: PureLiveSetStorage::new(),
            edge_count: 0,
            total_edge_capacity: 0,
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.rows.len()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    pub(crate) fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity() {
            return;
        }
        let tail = self.endpoints.len() as u32;
        self.rows.resize(new_vertex_capacity, tail);
        self.overflow_chunks.ensure_capacity(new_vertex_capacity);
        self.live_sets.ensure_capacity(new_vertex_capacity);
    }

    pub(crate) fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity() {
            self.resize(grown_vertex_capacity(min_capacity));
        }
    }

    pub(crate) fn allocate_primary_block(&mut self, src_idx: usize) {
        let block_offset = self.endpoints.len();
        self.endpoints
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, 0);
        self.edge_ids
            .resize(block_offset + DEFAULT_VERTEX_DEGREE, INVALID_EDGE_ID.0);
        self.rows
            .assign_primary_block(src_idx, block_offset as u32, DEFAULT_VERTEX_DEGREE as u32);
        self.add_capacity(DEFAULT_VERTEX_DEGREE);
    }

    pub(crate) fn add_capacity(&mut self, slots: usize) {
        self.total_edge_capacity = self.total_edge_capacity.saturating_add(slots);
    }

    pub(crate) fn sub_capacity(&mut self, slots: usize) {
        self.total_edge_capacity = self.total_edge_capacity.saturating_sub(slots);
    }

    pub(crate) fn primary_window(&self, src_idx: usize) -> (usize, usize) {
        if src_idx >= self.vertex_capacity() {
            return (0, 0);
        }
        let col_len = self.endpoints.len().min(self.edge_ids.len());
        self.rows.primary_window(src_idx, col_len)
    }

    pub(crate) fn make_nbr(&self, endpoint: u32, edge_id: EdgeId) -> Nbr {
        Nbr {
            endpoint,
            rank: 0,
            edge_id,
            delete_ts: Timestamp::MAX,
        }
    }

    pub(crate) fn scan_overflow_for_edge_id(
        &self,
        src_vid: u32,
        edge_id: EdgeId,
    ) -> Option<(usize, usize)> {
        let chunks = self.overflow_chunks.get(src_vid)?;
        for (chunk_idx, chunk) in chunks.iter().enumerate() {
            for (edge_idx, &eid) in chunk.edge_ids.iter().enumerate() {
                if EdgeId(eid) == edge_id {
                    return Some((chunk_idx, edge_idx));
                }
            }
        }
        None
    }

    /// Insert one edge, reporting the physical slot it landed in.
    ///
    /// Shared by the trait entry below and by the bundled form, so both
    /// shapes run identical dedup and spill decisions with no duplicated
    /// row-management logic.
    pub(crate) fn insert_edge_returning_position(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
    ) -> StorageResult<EdgePosition> {
        let (decoded_endpoint, decoded_rank) = dst.decode_edge_endpoint();
        let endpoint = decoded_endpoint.as_int64().unwrap_or(0) as u32;

        if decoded_rank != 0 {
            return Err(StorageError::conflict(format!(
                "[PureTopologyCsr] rank must be 0, got {}",
                decoded_rank
            )));
        }

        let src_idx = src_vid as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        if self.rows.primary_capacities[src_idx] == 0 {
            self.allocate_primary_block(src_idx);
        }

        let live = if let Some(set) = self.live_sets.get(src_vid) {
            if set.contains(&endpoint) {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
            set.len()
        } else {
            let (present, live) = self.row_live_scan(src_vid, endpoint);
            if present {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
            live
        };

        let degree = self.rows.degrees[src_idx] as usize;
        if degree < self.rows.primary_capacities[src_idx] as usize {
            let base = self.rows.adj_offsets[src_idx] as usize;
            self.endpoints[base + degree] = endpoint;
            self.edge_ids[base + degree] = edge_id.0;
            self.rows.degrees[src_idx] += 1;
            let position = EdgePosition::Primary {
                slot: degree as u32,
            };
            self.track_live_insert(src_vid, endpoint, position);
            self.edge_count += 1;
            return Ok(position);
        }

        let effective_chunk_edges = if live > 0 {
            (live * 2).max(self.overflow_chunk_edges)
        } else {
            self.overflow_chunk_edges
        };
        let (chunk_count, added) =
            self.overflow_chunks
                .push_to_row(src_vid, (endpoint, edge_id), effective_chunk_edges);
        if let Some(new_cap) = added {
            self.add_capacity(new_cap);
        }
        let position = self
            .overflow_chunks
            .get(src_vid)
            .map(|chunks| {
                let chunk = chunks.len().saturating_sub(1);
                let slot = chunks
                    .last()
                    .map(|tail| tail.len().saturating_sub(1))
                    .unwrap_or(0);
                EdgePosition::Overflow {
                    chunk: chunk as u32,
                    slot: slot as u32,
                }
            })
            .unwrap_or(EdgePosition::Overflow {
                chunk: chunk_count.saturating_sub(1) as u32,
                slot: 0,
            });
        self.track_live_insert(src_vid, endpoint, position);
        self.edge_count += 1;
        Ok(position)
    }

    pub fn clear(&mut self) {
        self.rows.degrees.fill(0);
        self.endpoints.clear();
        self.edge_ids.clear();
        self.overflow_chunks.clear();
        self.live_sets.clear();
        self.total_edge_capacity = self
            .rows
            .primary_capacities
            .iter()
            .map(|cap| *cap as usize)
            .sum();
        self.edge_count = 0;
    }

    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        for i in start..end {
            let endpoint = self.endpoints[i];
            let edge_id = EdgeId(self.edge_ids[i]);
            if edge_id == INVALID_EDGE_ID {
                continue;
            }
            if !f(self.make_nbr(endpoint, edge_id)) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    let endpoint = chunk.endpoints[i];
                    let edge_id = EdgeId(chunk.edge_ids[i]);
                    if edge_id == INVALID_EDGE_ID {
                        continue;
                    }
                    if !f(self.make_nbr(endpoint, edge_id)) {
                        return;
                    }
                }
            }
        }
    }

    pub fn visit_physical_with_position<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(EdgePosition, Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        for (i, idx) in (start..end).enumerate() {
            let endpoint = self.endpoints[idx];
            let edge_id = EdgeId(self.edge_ids[idx]);
            if !f(
                EdgePosition::Primary { slot: i as u32 },
                self.make_nbr(endpoint, edge_id),
            ) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for (chunk_idx, chunk) in chunks.iter().enumerate() {
                for slot_idx in 0..chunk.len() {
                    let endpoint = chunk.endpoints[slot_idx];
                    let edge_id = EdgeId(chunk.edge_ids[slot_idx]);
                    if !f(
                        EdgePosition::Overflow {
                            chunk: chunk_idx as u32,
                            slot: slot_idx as u32,
                        },
                        self.make_nbr(endpoint, edge_id),
                    ) {
                        return;
                    }
                }
            }
        }
    }

    pub fn visit_hot<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(super::HotNbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        for idx in start..end {
            let h = super::HotNbr {
                endpoint: self.endpoints[idx],
                rank: 0,
                edge_id: EdgeId(self.edge_ids[idx]),
            };
            if !f(h) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    let h = super::HotNbr {
                        endpoint: chunk.endpoints[i],
                        rank: 0,
                        edge_id: EdgeId(chunk.edge_ids[i]),
                    };
                    if !f(h) {
                        return;
                    }
                }
            }
        }
    }

    pub(crate) fn rebuild_live_sets(&mut self) {
        let capacity = self.vertex_capacity() as u32;
        self.live_sets.clear();
        self.live_sets.ensure_capacity(self.vertex_capacity());
        for vid in 0..capacity {
            self.rebuild_live_set_for_vertex(vid);
        }
    }

    /// Borrowed row walk over live entries without allocating.
    ///
    /// Skips sentinel holes inline, so callers iterate one row with no
    /// intermediate vector regardless of primary versus overflow layout.
    pub fn iter_row(&self, src_vid: u32) -> PureRowIter<'_> {
        let src_idx = src_vid as usize;
        let (start, end) = if src_idx < self.vertex_capacity() {
            self.primary_window(src_idx)
        } else {
            (0, 0)
        };
        let (primary_endpoints, primary_ids) =
            if start <= end && end <= self.endpoints.len() && end <= self.edge_ids.len() {
                (&self.endpoints[start..end], &self.edge_ids[start..end])
            } else {
                (&[][..], &[][..])
            };
        PureRowIter {
            csr: self,
            primary_endpoints,
            primary_ids,
            primary_idx: 0,
            overflow: self.overflow_chunks.get(src_vid),
            chunk_idx: 0,
            slot_idx: 0,
        }
    }

    /// Whether the primary window of one row arrives in key order.
    ///
    /// Sorted-prefix probe behind threshold scans: after a maintenance sort
    /// live entries lead sorted with sentinel holes sunk to the back, so the
    /// window bisects even while overflow stays unsorted.
    pub fn is_primary_sorted(&self, src_vid: u32) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return true;
        }
        let (start, end) = self.primary_window(src_idx);
        if end.saturating_sub(start) <= 1 {
            return true;
        }
        for i in start..end.saturating_sub(1) {
            if (self.endpoints[i], self.edge_ids[i]) > (self.endpoints[i + 1], self.edge_ids[i + 1])
            {
                return false;
            }
        }
        true
    }

    /// Whether the live endpoints of one row arrive in ascending order.
    ///
    /// Pure rows are insertion-ordered, so this usually reports false on
    /// multi-edge rows. Frozen packing sorts rows, after which the same
    /// check would report true. Query planning consults this before
    /// choosing a bisection over a linear walk.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        let mut last: Option<(u32, u64)> = None;
        for nbr in self.iter_row(src_vid) {
            let key = (nbr.endpoint, nbr.edge_id.0);
            if let Some(prev) = last {
                if key < prev {
                    return false;
                }
            }
            last = Some(key);
        }
        true
    }

    /// Sort one primary row into `(endpoint, edge_id)` order.
    ///
    /// Maintenance-only entry: sorting moves slots, so every previously
    /// issued `EdgePosition` for this row becomes stale and the caller must
    /// relocate through the edge-id key first. Live entries sort to the
    /// front; sentinel holes sink to the back with a maximal endpoint so
    /// the whole window stays ordered. The live index is rebuilt. Overflow
    /// chunks stay in insertion order as the unsorted suffix. No watermark
    /// or sorted flag is persisted.
    pub fn sort_row(&mut self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return false;
        }
        let (start, end) = self.primary_window(idx);
        if end.saturating_sub(start) <= 1 {
            return false;
        }
        let mut live: Vec<(u32, u64)> = Vec::new();
        for i in start..end {
            let eid = self.edge_ids[i];
            if eid != INVALID_EDGE_ID.0 {
                live.push((self.endpoints[i], eid));
            }
        }
        if live.len() <= 1 {
            return false;
        }
        let mut sorted = live.clone();
        sorted.sort_unstable();
        if sorted == live {
            return false;
        }
        for (offset, (endpoint, eid)) in sorted.iter().enumerate() {
            self.endpoints[start + offset] = *endpoint;
            self.edge_ids[start + offset] = *eid;
        }
        for i in start + sorted.len()..end {
            self.endpoints[i] = u32::MAX;
            self.edge_ids[i] = INVALID_EDGE_ID.0;
        }
        self.rebuild_live_set_for_vertex(vid);
        true
    }

    /// Sort every primary row that is out of order. Returns reordered rows.
    pub fn sort_all_rows(&mut self) -> usize {
        let rows = self.vertex_capacity();
        let mut reordered = 0usize;
        for vid in 0..rows {
            if self.sort_row(vid as u32) {
                reordered += 1;
            }
        }
        reordered
    }

    /// Visit live entries whose endpoint falls in the inclusive
    /// `[lower, upper]` range (`None` means unbounded).
    ///
    /// Sorted primary prefixes bisect to the endpoint window, then filter
    /// holes inline, and the overflow suffix always scans linearly. The
    /// prefix probe is `is_primary_sorted`, so hybrid rows still bisect.
    pub fn visit_threshold<F>(&self, src_vid: u32, lower: Option<u32>, upper: Option<u32>, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let in_range = |endpoint: u32| -> bool {
            if let Some(lo) = lower {
                if endpoint < lo {
                    return false;
                }
            }
            if let Some(hi) = upper {
                if endpoint > hi {
                    return false;
                }
            }
            true
        };
        let (start, end) = self.primary_window(src_idx);
        let endpoints = &self.endpoints[start..end];
        let edge_ids = &self.edge_ids[start..end];
        if self.is_primary_sorted(src_vid) && endpoints.len() > 1 {
            let lo = match lower {
                Some(lo) => endpoints.partition_point(|e| *e < lo),
                None => 0,
            };
            let hi = match upper {
                Some(hi) => endpoints.partition_point(|e| *e <= hi),
                None => endpoints.len(),
            };
            for (endpoint, eid) in endpoints[lo..hi].iter().zip(&edge_ids[lo..hi]) {
                if *eid != INVALID_EDGE_ID.0 {
                    if !f(self.make_nbr(*endpoint, EdgeId(*eid))) {
                        return;
                    }
                }
            }
        } else {
            for (endpoint, eid) in endpoints.iter().zip(edge_ids.iter()) {
                if *eid != INVALID_EDGE_ID.0 && in_range(*endpoint) {
                    if !f(self.make_nbr(*endpoint, EdgeId(*eid))) {
                        return;
                    }
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    let eid = chunk.edge_ids[i];
                    if eid != INVALID_EDGE_ID.0 && in_range(chunk.endpoints[i]) {
                        if !f(self.make_nbr(chunk.endpoints[i], EdgeId(eid))) {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    pub fn fill_threshold_into(
        &self,
        src_vid: u32,
        lower: Option<u32>,
        upper: Option<u32>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        self.visit_threshold(src_vid, lower, upper, |nbr| {
            out.push(nbr);
            true
        });
    }

    /// Borrowed walk over every live entry of the table without allocating.
    pub fn iter_all(&self) -> PureAllIter<'_> {
        let cap = self.vertex_capacity() as u32;
        let row = self.iter_row(0);
        PureAllIter {
            csr: self,
            cap,
            vid: 0,
            row,
        }
    }

    pub(crate) fn rebuild_live_set_for_vertex(&mut self, vid: u32) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            self.live_sets.remove(vid);
            return;
        }
        let mut positioned = Vec::new();
        let (start, end) = self.primary_window(idx);
        for (slot, &eid) in self.edge_ids[start..end].iter().enumerate() {
            if eid != INVALID_EDGE_ID.0 {
                positioned.push((
                    self.endpoints[start + slot],
                    EdgePosition::Primary { slot: slot as u32 },
                ));
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for (chunk_idx, chunk) in chunks.iter().enumerate() {
                for (slot_idx, &eid) in chunk.edge_ids.iter().enumerate() {
                    if eid != INVALID_EDGE_ID.0 {
                        positioned.push((
                            chunk.endpoints[slot_idx],
                            EdgePosition::Overflow {
                                chunk: chunk_idx as u32,
                                slot: slot_idx as u32,
                            },
                        ));
                    }
                }
            }
        }
        if positioned.len() <= LIVE_SET_WIDTH_BOUND {
            self.live_sets.remove(vid);
        } else {
            self.live_sets
                .insert(vid, PureLiveKeySet::from_positions(positioned));
        }
    }

    fn row_live_scan(&self, vid: u32, endpoint: u32) -> (bool, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (false, 0);
        }
        let mut present = false;
        let mut live = 0usize;
        let (start, end) = self.primary_window(idx);
        for i in start..end {
            if self.edge_ids[i] != INVALID_EDGE_ID.0 {
                live += 1;
                if self.endpoints[i] == endpoint {
                    present = true;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for chunk in chunks {
                for (i, &eid) in chunk.edge_ids.iter().enumerate() {
                    if eid != INVALID_EDGE_ID.0 {
                        live += 1;
                        if chunk.endpoints[i] == endpoint {
                            present = true;
                        }
                    }
                }
            }
        }
        (present, live)
    }

    pub(crate) fn track_live_insert(&mut self, vid: u32, endpoint: u32, position: EdgePosition) {
        if self.live_sets.get(vid).is_some() {
            self.live_sets.insert_key(vid, endpoint, position);
            return;
        }
        // Narrow rows stay set-free until the physical row width passes the
        // bound. The width check touches only lengths, not entries: counting
        // live entries instead would cost a full row walk per insert, so the
        // physical gate is kept deliberately. Rebuilds drop the set again
        // when the live width is still narrow, so tombstone-heavy rows may
        // rescan on later inserts until maintenance compacts them.
        let mut width = 0usize;
        if (vid as usize) < self.vertex_capacity() {
            width = self.rows.degrees[vid as usize] as usize;
            if let Some(chunks) = self.overflow_chunks.get(vid) {
                width += chunks.iter().map(|chunk| chunk.len()).sum::<usize>();
            }
        }
        if width > LIVE_SET_WIDTH_BOUND {
            self.rebuild_live_set_for_vertex(vid);
        }
    }

    pub(crate) fn track_live_remove(&mut self, vid: u32, endpoint: u32) {
        if self.live_sets.get(vid).is_none() {
            return;
        }
        self.live_sets.remove_key(vid, &endpoint);
    }

    fn dump_columns(out: &mut Vec<u8>, values: &[u32]) {
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for &v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn dump_columns_u64(out: &mut Vec<u8>, values: &[u64]) {
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for &v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn load_columns_u32(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u32>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.saturating_mul(4);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "pure CSR u32 column too short",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&data[*offset..*offset + 4]);
            *offset += 4;
            out.push(u32::from_le_bytes(buf));
        }
        Ok(out)
    }

    fn load_columns_u64(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u64>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.saturating_mul(8);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "pure CSR u64 column too short",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&data[*offset..*offset + 8]);
            *offset += 8;
            out.push(u64::from_le_bytes(buf));
        }
        Ok(out)
    }
}

impl CsrBase for PureTopologyCsr {
    fn vertex_capacity(&self) -> usize {
        PureTopologyCsr::vertex_capacity(self)
    }

    fn edge_count(&self) -> u64 {
        self.edge_count
    }

    fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&(self.rows.adj_offsets.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(self.endpoints.len() as u64).to_le_bytes());

        Self::dump_columns(out, &self.rows.degrees);
        Self::dump_columns(out, &self.endpoints);
        Self::dump_columns_u64(out, &self.edge_ids);

        for vid in 0..self.rows.adj_offsets.len() {
            let chunks = self.overflow_chunks.get(vid as u32);
            out.extend_from_slice(&(chunks.map_or(0, Vec::len) as u32).to_le_bytes());
            if let Some(chunks) = chunks {
                for chunk in chunks {
                    out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                    Self::dump_columns(out, &chunk.endpoints);
                    Self::dump_columns_u64(out, &chunk.edge_ids);
                }
            }
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 24 {
            return Err(StorageError::deserialize_error(
                "PureTopologyCsr data too short for header",
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "Pure CSR dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;

        let mut offset = 0usize;

        let vertex_capacity = read_u64_le(data, &mut offset)? as usize;
        let edge_count = read_u64_le(data, &mut offset)?;
        let primary_len = read_u64_le(data, &mut offset)? as usize;

        let degrees = Self::load_columns_u32(data, &mut offset)?;
        let endpoints = Self::load_columns_u32(data, &mut offset)?;
        let edge_ids = Self::load_columns_u64(data, &mut offset)?;

        if degrees.len() != vertex_capacity {
            return Err(StorageError::deserialize_error(
                "PureTopologyCsr degrees length mismatch",
            ));
        }
        if endpoints.len() != primary_len || edge_ids.len() != primary_len {
            return Err(StorageError::deserialize_error(
                "PureTopologyCsr neighbor column length mismatch",
            ));
        }

        let mut overflow_chunks = PureOverflowStorage::new();
        let mut overflow_capacity = 0usize;
        let mut recomputed: u64 = 0;
        for &eid in &edge_ids {
            if eid != INVALID_EDGE_ID.0 {
                recomputed += 1;
            }
        }

        for vid in 0..vertex_capacity {
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let chunk_len = read_u32_le(data, &mut offset)? as usize;
                let chunk_endpoints = Self::load_columns_u32(data, &mut offset)?;
                let chunk_edge_ids = Self::load_columns_u64(data, &mut offset)?;
                if chunk_endpoints.len() != chunk_len || chunk_edge_ids.len() != chunk_len {
                    return Err(StorageError::deserialize_error(
                        "PureTopologyCsr overflow chunk column length mismatch",
                    ));
                }
                for &eid in &chunk_edge_ids {
                    if eid != INVALID_EDGE_ID.0 {
                        recomputed += 1;
                    }
                }
                overflow_capacity = overflow_capacity.saturating_add(chunk_len);
                chunks.push(PureOverflowChunk {
                    endpoints: chunk_endpoints,
                    edge_ids: chunk_edge_ids,
                });
            }
            if !chunks.is_empty() {
                overflow_chunks.insert(vid as u32, chunks);
            }
        }

        if recomputed != edge_count {
            return Err(StorageError::deserialize_error(format!(
                "PureTopologyCsr edge count mismatch: stored={}, recomputed={}",
                edge_count, recomputed
            )));
        }

        self.rows.adj_offsets.resize(vertex_capacity, 0);
        self.rows.degrees.resize(vertex_capacity, 0);
        self.rows.primary_capacities.resize(vertex_capacity, 0);

        let mut running_offset = 0u32;
        for vid in 0..vertex_capacity {
            self.rows.adj_offsets[vid] = running_offset;
            self.rows.degrees[vid] = degrees[vid];
            self.rows.primary_capacities[vid] = degrees[vid];
            running_offset += degrees[vid];
        }

        self.endpoints = endpoints;
        self.edge_ids = edge_ids;
        self.total_edge_capacity = self.endpoints.len().saturating_add(overflow_capacity);
        self.overflow_chunks = overflow_chunks;
        self.edge_count = edge_count;

        self.live_sets.clear();
        self.live_sets.ensure_capacity(vertex_capacity);
        self.rebuild_live_sets();

        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in pure CSR payload",
            ));
        }

        Ok(())
    }
}

impl MutableCsrTrait for PureTopologyCsr {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        self.insert_edge_returning_position(src_vid, dst, edge_id)
            .map(|_| ())
    }

    fn delete_edge(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        if edge_id == INVALID_EDGE_ID {
            return Ok(false);
        }

        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }

        let found = {
            let (start, end) = self.primary_window(src_idx);
            self.edge_ids[start..end]
                .iter()
                .enumerate()
                .find_map(|(i, &eid)| {
                    if EdgeId(eid) == edge_id {
                        Some((start + i, self.endpoints[start + i]))
                    } else {
                        None
                    }
                })
        };
        if let Some((idx, endpoint)) = found {
            self.edge_ids[idx] = INVALID_EDGE_ID.0;
            self.edge_count -= 1;
            self.track_live_remove(src_vid, endpoint);
            return Ok(true);
        }

        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            let endpoint =
                self.overflow_chunks.get(src_vid).unwrap()[chunk_idx].endpoints[edge_idx];
            if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
                chunks[chunk_idx].edge_ids[edge_idx] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint);
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        let mut noop = |_: EdgeId| {};
        self.delete_edge_by_dst_reporting(src_vid, dst, ts, &mut noop)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        _ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        let (decoded_endpoint, _decoded_rank) = dst.decode_edge_endpoint();
        let target_endpoint = decoded_endpoint.as_int64().unwrap_or(0) as u32;
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let mut deleted = 0usize;

        let (start, end) = self.primary_window(src_idx);
        for i in start..end {
            if self.endpoints[i] == target_endpoint && self.edge_ids[i] != INVALID_EDGE_ID.0 {
                let eid = EdgeId(self.edge_ids[i]);
                self.edge_ids[i] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                on_deleted(eid);
                deleted += 1;
            }
        }

        if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
            for chunk in chunks.iter_mut() {
                let to_delete: Vec<(usize, u64)> = chunk
                    .edge_ids
                    .iter()
                    .enumerate()
                    .filter(|(i, &eid)| {
                        chunk.endpoints[*i] == target_endpoint && EdgeId(eid) != INVALID_EDGE_ID
                    })
                    .map(|(i, &eid)| (i, eid))
                    .collect();
                for (i, eid) in to_delete {
                    chunk.edge_ids[i] = INVALID_EDGE_ID.0;
                    self.edge_count -= 1;
                    on_deleted(EdgeId(eid));
                    deleted += 1;
                }
            }
        }

        if deleted > 0 {
            self.track_live_remove(src_vid, target_endpoint);
        }

        deleted
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        _ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        let (decoded_endpoint, _decoded_rank) = dst.decode_edge_endpoint();
        let target_endpoint = decoded_endpoint.as_int64().unwrap_or(0) as u32;
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let mut deleted = 0usize;

        let (start, end) = self.primary_window(src_idx);
        for (i, idx) in (start..end).enumerate() {
            if self.endpoints[idx] == target_endpoint && self.edge_ids[idx] != INVALID_EDGE_ID.0 {
                let eid = EdgeId(self.edge_ids[idx]);
                self.edge_ids[idx] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                on_deleted(eid, Some(EdgePosition::Primary { slot: i as u32 }));
                deleted += 1;
            }
        }

        if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
            for (chunk_idx, chunk) in chunks.iter_mut().enumerate() {
                let to_delete: Vec<(usize, u64)> = chunk
                    .edge_ids
                    .iter()
                    .enumerate()
                    .filter(|(i, &eid)| {
                        chunk.endpoints[*i] == target_endpoint && EdgeId(eid) != INVALID_EDGE_ID
                    })
                    .map(|(i, &eid)| (i, eid))
                    .collect();
                for (slot_idx, eid) in to_delete {
                    chunk.edge_ids[slot_idx] = INVALID_EDGE_ID.0;
                    self.edge_count -= 1;
                    on_deleted(
                        EdgeId(eid),
                        Some(EdgePosition::Overflow {
                            chunk: chunk_idx as u32,
                            slot: slot_idx as u32,
                        }),
                    );
                    deleted += 1;
                }
            }
        }

        if deleted > 0 {
            self.track_live_remove(src_vid, target_endpoint);
        }

        deleted
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        let mut found = None;
        self.visit_physical_with_position(src_vid, |position, nbr| {
            if nbr.edge_id == edge_id {
                found = Some((position, nbr));
                false
            } else {
                true
            }
        });
        found
    }

    fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        match position {
            EdgePosition::Primary { slot } => {
                let src_idx = src_vid as usize;
                if src_idx >= self.vertex_capacity() {
                    return Ok(false);
                }
                if slot as usize >= self.rows.degrees[src_idx] as usize {
                    return Ok(false);
                }
                let idx = self.rows.adj_offsets[src_idx] as usize + slot as usize;
                if idx >= self.edge_ids.len() || EdgeId(self.edge_ids[idx]) != expected {
                    return Ok(false);
                }
                if self.edge_ids[idx] == INVALID_EDGE_ID.0 {
                    return Ok(false);
                }
                let endpoint = self.endpoints[idx];
                self.edge_ids[idx] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint);
                Ok(true)
            }
            EdgePosition::Overflow { chunk, slot } => {
                let Some(chunks) = self.overflow_chunks.get_mut(src_vid) else {
                    return Ok(false);
                };
                let Some(c) = chunks.get_mut(chunk as usize) else {
                    return Ok(false);
                };
                if slot as usize >= c.len() {
                    return Ok(false);
                }
                if c.edge_id_at(slot as usize) != Some(expected) {
                    return Ok(false);
                }
                if c.edge_ids[slot as usize] == INVALID_EDGE_ID.0 {
                    return Ok(false);
                }
                let endpoint = c.endpoints[slot as usize];
                c.edge_ids[slot as usize] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint);
                Ok(true)
            }
        }
    }

    fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        match position {
            EdgePosition::Primary { slot } => {
                let src_idx = src_vid as usize;
                if src_idx >= self.vertex_capacity() {
                    return false;
                }
                if slot as usize >= self.rows.degrees[src_idx] as usize {
                    return false;
                }
                let idx = self.rows.adj_offsets[src_idx] as usize + slot as usize;
                if idx >= self.edge_ids.len() {
                    return false;
                }
                if self.edge_ids[idx] != INVALID_EDGE_ID.0 {
                    return false;
                }
                self.edge_ids[idx] = expected.0;
                self.edge_count += 1;
                let endpoint = self.endpoints[idx];
                self.track_live_insert(src_vid, endpoint, EdgePosition::Primary { slot });
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                let Some(chunks) = self.overflow_chunks.get_mut(src_vid) else {
                    return false;
                };
                let Some(c) = chunks.get_mut(chunk as usize) else {
                    return false;
                };
                if slot as usize >= c.len() {
                    return false;
                }
                if c.edge_ids[slot as usize] != INVALID_EDGE_ID.0 {
                    return false;
                }
                let endpoint = c.endpoints[slot as usize];
                c.edge_ids[slot as usize] = expected.0;
                self.edge_count += 1;
                self.track_live_insert(src_vid, endpoint, EdgePosition::Overflow { chunk, slot });
                true
            }
        }
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        if offset < 0 {
            return Ok(false);
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }
        if offset as usize >= self.rows.degrees[src_idx] as usize {
            return Ok(false);
        }
        let idx = self.rows.adj_offsets[src_idx] as usize + offset as usize;
        if idx >= self.edge_ids.len() {
            return Ok(false);
        }
        if self.edge_ids[idx] == INVALID_EDGE_ID.0 {
            return Ok(false);
        }
        let endpoint = self.endpoints[idx];
        self.edge_ids[idx] = INVALID_EDGE_ID.0;
        self.edge_count -= 1;
        self.track_live_remove(src_vid, endpoint);
        Ok(true)
    }

    fn revert_delete_by_offset(&mut self, _src_vid: u32, _offset: i32, _ts: Timestamp) -> bool {
        // Deletion overwrites the edge id with the unassignable sentinel, so
        // an offset-only revert cannot recover the erased identity. Callers
        // needing restore must keep the edge id and use the positioned path.
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        if offset < 0 {
            return None;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.rows.primary_capacities[src_idx] == 0 {
            return None;
        }
        if offset as usize >= self.rows.degrees[src_idx] as usize {
            return None;
        }
        let idx = self.rows.adj_offsets[src_idx] as usize + offset as usize;
        let endpoint = *self.endpoints.get(idx)?;
        let edge_id = EdgeId(*self.edge_ids.get(idx)?);
        Some(self.make_nbr(endpoint, edge_id))
    }

    /// Locate the first live edge by endpoint without consulting snapshots.
    ///
    /// Wide rows answer from the endpoint location index: a present key
    /// addresses its slot directly, an absent key returns without scanning.
    /// Narrow rows without an index fall through to the linear walk.
    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (decoded_endpoint, _decoded_rank) = dst.decode_edge_endpoint();
        let target_endpoint = decoded_endpoint.as_int64().unwrap_or(0) as u32;

        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }

        if let Some(set) = self.live_sets.get(src_vid) {
            let Some(position) = set.position(&target_endpoint) else {
                return None;
            };
            let nbr = match position {
                EdgePosition::Primary { slot } => {
                    let base = self.rows.adj_offsets[src_idx] as usize;
                    let idx = base + slot as usize;
                    let endpoint = *self.endpoints.get(idx)?;
                    let edge_id = EdgeId(*self.edge_ids.get(idx)?);
                    self.make_nbr(endpoint, edge_id)
                }
                EdgePosition::Overflow { chunk, slot } => {
                    let chunks = self.overflow_chunks.get(src_vid)?;
                    let c = chunks.get(chunk as usize)?;
                    let endpoint = c.endpoint_at(slot as usize)?;
                    let edge_id = c.edge_id_at(slot as usize)?;
                    self.make_nbr(endpoint, edge_id)
                }
            };
            // Position mismatch means the row moved without a rebuild, which
            // must not happen; fall back to the scan instead of answering
            // from the wrong slot.
            if nbr.endpoint == target_endpoint
                && nbr.edge_id != INVALID_EDGE_ID
                && nbr.delete_ts == Timestamp::MAX
            {
                return Some(nbr);
            }
        }

        let (start, end) = self.primary_window(src_idx);
        for i in start..end {
            if self.endpoints[i] == target_endpoint && self.edge_ids[i] != INVALID_EDGE_ID.0 {
                return Some(self.make_nbr(self.endpoints[i], EdgeId(self.edge_ids[i])));
            }
        }

        if let Some(single) = self.overflow_chunks.single_chunk(src_vid) {
            for i in 0..single.len() {
                if single.endpoints[i] == target_endpoint && single.edge_ids[i] != INVALID_EDGE_ID.0
                {
                    return Some(self.make_nbr(single.endpoints[i], EdgeId(single.edge_ids[i])));
                }
            }
            return None;
        }

        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if chunk.endpoints[i] == target_endpoint
                        && chunk.edge_ids[i] != INVALID_EDGE_ID.0
                    {
                        return Some(self.make_nbr(chunk.endpoints[i], EdgeId(chunk.edge_ids[i])));
                    }
                }
            }
        }

        None
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_physical_into(src_vid, &mut out);
        out
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        out.clear();
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        out.reserve(end - start);
        for i in start..end {
            let edge_id = EdgeId(self.edge_ids[i]);
            if edge_id == INVALID_EDGE_ID {
                continue;
            }
            out.push(self.make_nbr(self.endpoints[i], edge_id));
        }
        if let Some(single) = self.overflow_chunks.single_chunk(src_vid) {
            out.reserve(single.len());
            for i in 0..single.len() {
                let edge_id = EdgeId(single.edge_ids[i]);
                if edge_id == INVALID_EDGE_ID {
                    continue;
                }
                out.push(self.make_nbr(single.endpoints[i], edge_id));
            }
            return;
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                out.reserve(chunk.len());
                for i in 0..chunk.len() {
                    let edge_id = EdgeId(chunk.edge_ids[i]);
                    if edge_id == INVALID_EDGE_ID {
                        continue;
                    }
                    out.push(self.make_nbr(chunk.endpoints[i], edge_id));
                }
            }
        }
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return false;
        }
        if self.rows.degrees[idx] > 0 {
            return true;
        }
        self.overflow_chunks
            .get(vid)
            .is_some_and(|chunks| chunks.iter().any(|c| !c.is_empty()))
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let (start, end) = self.primary_window(src_idx);
        self.edge_ids[start..end]
            .iter()
            .any(|&eid| EdgeId(eid) == edge_id)
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        let found = {
            let (start, end) = self.primary_window(src_idx);
            self.edge_ids[start..end]
                .iter()
                .enumerate()
                .find_map(|(i, &eid)| {
                    if EdgeId(eid) == edge_id {
                        Some((start + i, self.endpoints[start + i]))
                    } else {
                        None
                    }
                })
        };
        if let Some((idx, _endpoint)) = found {
            let degree = self.rows.degrees[src_idx] as usize;
            let base = self.rows.adj_offsets[src_idx] as usize;
            self.endpoints
                .copy_within(base + idx - base + 1..base + degree, base + idx - base);
            self.edge_ids
                .copy_within(base + idx - base + 1..base + degree, base + idx - base);
            self.rows.degrees[src_idx] -= 1;
            self.sub_capacity(1);
            self.edge_count -= 1;
            self.rebuild_live_set_for_vertex(src_vid);
            return true;
        }

        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            let detached = if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
                chunks[chunk_idx].remove(edge_idx);
                if chunks[chunk_idx].is_empty() {
                    let removed = chunks.remove(chunk_idx);
                    Some((removed.capacity(), chunks.is_empty()))
                } else {
                    None
                }
            } else {
                return false;
            };
            self.edge_count -= 1;
            if let Some((freed, emptied)) = detached {
                self.sub_capacity(freed);
                if emptied {
                    self.overflow_chunks.remove(src_vid);
                }
            }
            self.rebuild_live_set_for_vertex(src_vid);
            return true;
        }

        false
    }

    fn revert_delete_by_edge_id(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        // Deleted slots hold the sentinel instead of the edge id, so an
        // id-keyed scan cannot locate them. Restoration uses the positioned
        // path with the expected id supplied by the caller.
        false
    }

    /// Timestamp-filtered point lookup.
    ///
    /// Pure rows store no timestamps: [`Self::make_nbr`] stamps every live
    /// entry identically, so the timestamp carries no information here and
    /// this entry shares the physical indexed path exactly instead of
    /// duplicating its scan. Wide rows therefore get the same absent-key
    /// short-circuit as [`Self::get_edge_physical`].
    fn get_edge(&self, src_vid: u32, dst: VertexId, _ts: Timestamp) -> Option<Nbr> {
        self.get_edge_physical(src_vid, dst)
    }

    fn edges_of(&self, src_vid: u32, _ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }

        let (start, end) = self.primary_window(src_idx);
        let overflow_len = self
            .overflow_chunks
            .get(src_vid)
            .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum::<usize>())
            .unwrap_or(0);
        let mut result = Vec::with_capacity((end - start) + overflow_len);

        for i in start..end {
            if self.edge_ids[i] != INVALID_EDGE_ID.0 {
                result.push(self.make_nbr(self.endpoints[i], EdgeId(self.edge_ids[i])));
            }
        }

        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if chunk.edge_ids[i] != INVALID_EDGE_ID.0 {
                        result.push(self.make_nbr(chunk.endpoints[i], EdgeId(chunk.edge_ids[i])));
                    }
                }
            }
        }

        result
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        _cutoff: Timestamp,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }

        let mut removed = 0usize;

        let base = self.rows.adj_offsets[idx] as usize;
        let degree = self.rows.degrees[idx] as usize;
        let mut keep = 0usize;
        for i in 0..degree {
            let idx_i = base + i;
            if self.edge_ids[idx_i] == INVALID_EDGE_ID.0 {
                // Holes carry no edge identity: drop them silently instead
                // of reporting the sentinel to the authority above.
                removed += 1;
            } else {
                if keep != i {
                    self.endpoints[base + keep] = self.endpoints[idx_i];
                    self.edge_ids[base + keep] = self.edge_ids[idx_i];
                }
                keep += 1;
            }
        }
        self.rows.degrees[idx] = keep as u32;
        self.rows.primary_capacities[idx] = keep as u32;

        if self.overflow_chunks.get(vid).is_some() {
            let chunks = self.overflow_chunks.remove(vid).unwrap_or_default();
            let freed: usize = chunks.iter().map(|chunk| chunk.capacity()).sum();
            self.sub_capacity(freed);
            let mut kept_ep: Vec<u32> = Vec::new();
            let mut kept_eid: Vec<u64> = Vec::new();
            for chunk in &chunks {
                for i in 0..chunk.len() {
                    let eid = chunk.edge_ids[i];
                    if eid == INVALID_EDGE_ID.0 {
                        removed += 1;
                    } else {
                        kept_ep.push(chunk.endpoints[i]);
                        kept_eid.push(eid);
                    }
                }
            }
            if !kept_ep.is_empty() {
                let single = PureOverflowChunk::consolidated(&kept_ep, &kept_eid);
                let added = single.capacity();
                self.add_capacity(added);
                self.overflow_chunks.insert(vid, vec![single]);
            }
            self.rebuild_live_set_for_vertex(vid);
        } else if removed > 0 {
            self.rebuild_live_set_for_vertex(vid);
        }

        removed
    }

    fn reclaimable_count(&self, _vid: u32, _cutoff: Timestamp) -> usize {
        0
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (0, 0, 0);
        }
        let mut alive = 0usize;
        let mut dead = 0usize;
        let (start, end) = self.primary_window(idx);
        for i in start..end {
            if self.edge_ids[i] != INVALID_EDGE_ID.0 {
                alive += 1;
            } else {
                dead += 1;
            }
        }
        let mut capacity = self.rows.primary_capacities[idx] as usize;
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            capacity += chunks.iter().map(|chunk| chunk.capacity()).sum::<usize>();
            for chunk in chunks {
                for &eid in &chunk.edge_ids {
                    if eid != INVALID_EDGE_ID.0 {
                        alive += 1;
                    } else {
                        dead += 1;
                    }
                }
            }
        }
        (alive, dead, capacity)
    }

    fn row_gap(&self, vid: u32) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        self.rows.primary_capacities[idx].saturating_sub(self.rows.degrees[idx]) as usize
    }

    fn row_density(&self, vid: u32) -> f32 {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 1.0;
        }
        let cap = self.rows.primary_capacities[idx] as f32;
        if cap == 0.0 {
            return 1.0;
        }
        self.rows.degrees[idx] as f32 / cap
    }

    fn used_memory_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.endpoints.len() * 4
            + self.edge_ids.len() * 8
            + self.rows.adj_offsets.len() * 4 * 3
            + self.overflow_chunks.index_bytes()
            + self.overflow_chunks.total_entry_count() * (4 + 8)
            + self.live_sets.index_bytes()
            + self.live_sets.heap_bytes_total()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_row_absent_key_short_circuits() {
        let mut csr = PureTopologyCsr::with_capacity(16, 64);
        for dst in 0..12u32 {
            csr.insert_edge(
                0,
                VertexId::edge_endpoint_key(dst, 0),
                EdgeId(dst as u64),
                1,
            )
            .expect("insert");
        }
        assert!(
            csr.live_sets.get(0).is_some(),
            "wide row must carry an index"
        );
        let absent = VertexId::edge_endpoint_key(900, 0);
        assert!(csr.get_edge_physical(0, absent).is_none());
        assert!(csr.get_edge(0, absent, Timestamp::MAX).is_none());
        let present = VertexId::edge_endpoint_key(3, 0);
        assert!(csr.get_edge_physical(0, present).is_some());
        assert!(csr.get_edge(0, present, Timestamp::MAX).is_some());
    }

    #[test]
    fn count_index_capacity_stay_consistent() {
        let mut csr = PureTopologyCsr::with_overflow_chunk_edges(8, 32, 4);
        for dst in 0..10u32 {
            csr.insert_edge(
                0,
                VertexId::edge_endpoint_key(dst, 0),
                EdgeId(dst as u64),
                0,
            )
            .expect("insert");
        }
        csr.delete_edge_by_offset(0, 0, 0).expect("delete");
        let live: usize = {
            let mut buf = Vec::new();
            csr.fill_physical_into(0, &mut buf);
            buf.into_iter()
                .filter(|nbr| nbr.edge_id != INVALID_EDGE_ID)
                .count()
        };
        assert_eq!(csr.edge_count(), live as u64);
        assert!(csr.total_edge_capacity >= csr.endpoints.len());
        assert!(csr.total_edge_capacity >= live);
        let mut rebuilt = 0usize;
        if let Some(chunks) = csr.overflow_chunks.get(0) {
            rebuilt += chunks.iter().map(|chunk| chunk.len()).sum::<usize>();
        }
        let (start, end) = csr.primary_window(0);
        rebuilt += end - start;
        assert!(csr.total_edge_capacity >= rebuilt);
        assert!(!csr.revert_delete_by_offset(0, 0, 0));
        assert!(!csr.revert_delete_by_edge_id(0, EdgeId(999), 0));
    }

    #[test]
    fn positioned_revert_restores_deleted_slot() {
        let mut csr = PureTopologyCsr::with_capacity(8, 16);
        csr.insert_edge(0, VertexId::edge_endpoint_key(1, 0), EdgeId(10), 0)
            .expect("insert");
        csr.insert_edge(0, VertexId::edge_endpoint_key(2, 0), EdgeId(11), 0)
            .expect("insert");
        let position = EdgePosition::Primary { slot: 0 };
        assert!(csr
            .delete_edge_at_position(0, position, EdgeId(10), 0)
            .expect("positioned delete"));
        assert_eq!(csr.edge_count(), 1);
        assert!(csr.revert_delete_at_position(0, position, EdgeId(10), 0));
        assert_eq!(csr.edge_count(), 2);
        assert!(csr
            .get_edge_physical(0, VertexId::edge_endpoint_key(1, 0))
            .is_some());
    }

    #[test]
    fn sort_row_orders_live_prefix_and_threshold_bisects() {
        let mut csr = PureTopologyCsr::with_capacity(4, 16);
        for (endpoint, edge) in [(30, 1), (10, 2), (20, 3)] {
            csr.insert_edge(0, VertexId::edge_endpoint_key(endpoint, 0), EdgeId(edge), 0)
                .expect("insert");
        }
        assert!(!csr.is_row_sorted(0));
        assert!(csr.sort_row(0));
        assert!(csr.is_row_sorted(0));
        let mut ranged = Vec::new();
        csr.fill_threshold_into(0, Some(15), Some(25), &mut ranged);
        assert_eq!(ranged.len(), 1);
        assert_eq!(ranged[0].endpoint, 20);
    }
}

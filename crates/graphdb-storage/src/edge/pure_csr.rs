#![allow(dead_code)]
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
//! Reads assemble [`Nbr`] on the fly with `rank = 0`, `create_ts = 0`,
//! `delete_ts = Timestamp::MAX`.  No MVCC state is stored or checked.

use std::collections::HashMap;

use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::csr_shared::{
    grown_vertex_capacity, OverflowTable, SegmentedTable, DEFAULT_VERTEX_CAPACITY,
};
use super::csr_trait::{CsrBase, MutableCsrTrait};
use super::{EdgePosition, Nbr};

use crate::persistence::{read_u32_le, read_u64_le};

const INVALID_EDGE_ID: EdgeId = EdgeId(u64::MAX);

/// Version 2 covers the payload with a trailing CRC32 trailer verified on
/// load; version 1 payloads without the trailer are rejected by marker.
const PURE_CSR_FORMAT_VERSION: u32 = 2;

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
    pub(crate) adj_offsets: Vec<u32>,
    pub(crate) degrees: Vec<u32>,
    pub(crate) primary_capacities: Vec<u32>,
    pub(crate) endpoints: Vec<u32>,
    pub(crate) edge_ids: Vec<u64>,
    pub(crate) overflow_chunks: PureOverflowStorage,
    pub(crate) overflow_chunk_edges: usize,
    pub(crate) live_sets: PureLiveSetStorage,
    pub(crate) edge_count: u64,
    pub(crate) total_edge_capacity: usize,
}

impl Clone for PureTopologyCsr {
    fn clone(&self) -> Self {
        Self {
            adj_offsets: self.adj_offsets.clone(),
            degrees: self.degrees.clone(),
            primary_capacities: self.primary_capacities.clone(),
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
            adj_offsets: vec![0; vertex_cap],
            degrees: vec![0; vertex_cap],
            primary_capacities: vec![0; vertex_cap],
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
        self.adj_offsets.len()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    pub(crate) fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity() {
            return;
        }
        let tail = self.endpoints.len() as u32;
        self.adj_offsets.resize(new_vertex_capacity, tail);
        self.degrees.resize(new_vertex_capacity, 0);
        self.primary_capacities.resize(new_vertex_capacity, 0);
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
        self.adj_offsets[src_idx] = block_offset as u32;
        self.primary_capacities[src_idx] = DEFAULT_VERTEX_DEGREE as u32;
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
        let start = self.adj_offsets[src_idx] as usize;
        let end = start
            .saturating_add(self.degrees[src_idx] as usize)
            .min(self.endpoints.len())
            .min(self.edge_ids.len());
        (start.min(end), end)
    }

    pub(crate) fn make_nbr(&self, endpoint: u32, edge_id: EdgeId) -> Nbr {
        Nbr {
            endpoint,
            rank: 0,
            edge_id,
            create_ts: 0,
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

    pub(crate) fn overflow_total_entries(&self) -> usize {
        self.overflow_chunks.total_entry_count()
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

        if self.primary_capacities[src_idx] == 0 {
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

        let degree = self.degrees[src_idx] as usize;
        if degree < self.primary_capacities[src_idx] as usize {
            let base = self.adj_offsets[src_idx] as usize;
            self.endpoints[base + degree] = endpoint;
            self.edge_ids[base + degree] = edge_id.0;
            self.degrees[src_idx] += 1;
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
        self.degrees.fill(0);
        self.endpoints.clear();
        self.edge_ids.clear();
        self.overflow_chunks.clear();
        self.live_sets.clear();
        self.total_edge_capacity = self
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
            if !f(self.make_nbr(endpoint, edge_id)) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    let endpoint = chunk.endpoints[i];
                    let edge_id = EdgeId(chunk.edge_ids[i]);
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

    fn live_key_count(&self, vid: u32) -> usize {
        if let Some(set) = self.live_sets.get(vid) {
            return set.len();
        }
        self.row_live_count(vid)
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

    fn row_live_count(&self, vid: u32) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        let mut live = 0usize;
        let (start, end) = self.primary_window(idx);
        for i in start..end {
            if self.edge_ids[i] != INVALID_EDGE_ID.0 {
                live += 1;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for chunk in chunks {
                for &eid in &chunk.edge_ids {
                    if eid != INVALID_EDGE_ID.0 {
                        live += 1;
                    }
                }
            }
        }
        live
    }

    pub(crate) fn track_live_insert(&mut self, vid: u32, endpoint: u32, position: EdgePosition) {
        if self.live_sets.get(vid).is_some() {
            self.live_sets.insert_key(vid, endpoint, position);
            return;
        }
        let mut width = 0usize;
        if (vid as usize) < self.vertex_capacity() {
            width = self.degrees[vid as usize] as usize;
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
        out.extend_from_slice(&PURE_CSR_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.adj_offsets.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(self.endpoints.len() as u64).to_le_bytes());

        Self::dump_columns(out, &self.degrees);
        Self::dump_columns(out, &self.endpoints);
        Self::dump_columns_u64(out, &self.edge_ids);

        for vid in 0..self.adj_offsets.len() {
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
        if data.len() < 28 {
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

        let version = read_u32_le(data, &mut offset)?;
        if version != PURE_CSR_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported pure CSR format version: {}",
                version
            )));
        }
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

        self.adj_offsets.resize(vertex_capacity, 0);
        self.degrees.resize(vertex_capacity, 0);
        self.primary_capacities.resize(vertex_capacity, 0);

        let mut running_offset = 0u32;
        for vid in 0..vertex_capacity {
            self.adj_offsets[vid] = running_offset;
            self.degrees[vid] = degrees[vid];
            self.primary_capacities[vid] = degrees[vid];
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
                if slot as usize >= self.degrees[src_idx] as usize {
                    return Ok(false);
                }
                let idx = self.adj_offsets[src_idx] as usize + slot as usize;
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
                if slot as usize >= self.degrees[src_idx] as usize {
                    return false;
                }
                let idx = self.adj_offsets[src_idx] as usize + slot as usize;
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
        if offset as usize >= self.degrees[src_idx] as usize {
            return Ok(false);
        }
        let idx = self.adj_offsets[src_idx] as usize + offset as usize;
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
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        if offset < 0 {
            return None;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.primary_capacities[src_idx] == 0 {
            return None;
        }
        if offset as usize >= self.degrees[src_idx] as usize {
            return None;
        }
        let idx = self.adj_offsets[src_idx] as usize + offset as usize;
        let endpoint = *self.endpoints.get(idx)?;
        let edge_id = EdgeId(*self.edge_ids.get(idx)?);
        Some(self.make_nbr(endpoint, edge_id))
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (decoded_endpoint, _decoded_rank) = dst.decode_edge_endpoint();
        let target_endpoint = decoded_endpoint.as_int64().unwrap_or(0) as u32;

        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }

        if let Some(set) = self.live_sets.get(src_vid) {
            if let Some(position) = set.position(&target_endpoint) {
                let nbr = match position {
                    EdgePosition::Primary { slot } => {
                        let base = self.adj_offsets[src_idx] as usize;
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
                if nbr.endpoint == target_endpoint
                    && nbr.edge_id != INVALID_EDGE_ID
                    && nbr.delete_ts == Timestamp::MAX
                {
                    return Some(nbr);
                }
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
            out.push(self.make_nbr(self.endpoints[i], EdgeId(self.edge_ids[i])));
        }
        if let Some(single) = self.overflow_chunks.single_chunk(src_vid) {
            out.reserve(single.len());
            for i in 0..single.len() {
                out.push(self.make_nbr(single.endpoints[i], EdgeId(single.edge_ids[i])));
            }
            return;
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                out.reserve(chunk.len());
                for i in 0..chunk.len() {
                    out.push(self.make_nbr(chunk.endpoints[i], EdgeId(chunk.edge_ids[i])));
                }
            }
        }
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return false;
        }
        if self.degrees[idx] > 0 {
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
            let degree = self.degrees[src_idx] as usize;
            let base = self.adj_offsets[src_idx] as usize;
            self.endpoints
                .copy_within(base + idx - base + 1..base + degree, base + idx - base);
            self.edge_ids
                .copy_within(base + idx - base + 1..base + degree, base + idx - base);
            self.degrees[src_idx] -= 1;
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
        false
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, _ts: Timestamp) -> Option<Nbr> {
        let (decoded_endpoint, _decoded_rank) = dst.decode_edge_endpoint();
        let target_endpoint = decoded_endpoint.as_int64().unwrap_or(0) as u32;

        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
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

        let base = self.adj_offsets[idx] as usize;
        let degree = self.degrees[idx] as usize;
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
        self.degrees[idx] = keep as u32;
        self.primary_capacities[idx] = keep as u32;

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
        let mut overflow_entries = 0usize;
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for chunk in chunks {
                for &eid in &chunk.edge_ids {
                    overflow_entries += 1;
                    if eid != INVALID_EDGE_ID.0 {
                        alive += 1;
                    } else {
                        dead += 1;
                    }
                }
            }
        }
        let total = (end - start) + overflow_entries;
        (total, alive, dead)
    }

    fn row_gap(&self, vid: u32) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        self.primary_capacities[idx].saturating_sub(self.degrees[idx]) as usize
    }

    fn row_density(&self, vid: u32) -> f32 {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 1.0;
        }
        let cap = self.primary_capacities[idx] as f32;
        if cap == 0.0 {
            return 1.0;
        }
        self.degrees[idx] as f32 / cap
    }

    fn used_memory_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.endpoints.len() * 4
            + self.edge_ids.len() * 8
            + self.adj_offsets.len() * 4 * 3
            + self.overflow_chunks.index_bytes()
            + self.overflow_chunks.total_entry_count() * (4 + 8)
            + self.live_sets.index_bytes()
            + self.live_sets.heap_bytes_total()
    }
}

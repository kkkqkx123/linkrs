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
//! edges holds no slots in `nbr_list`, and overflow chunks are stored sparsely in a
//! HashMap keyed by vertex id. This keeps the per-row fixed cost to 12 bytes
//! (offset + degree + capacity) and eliminates HashMap fragmentation.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::persistence::{read_u32_le, read_u64_le};
use graphdb_core::{StorageError, StorageResult};

use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId};

pub mod iter;
pub mod overflow;
pub mod serialization;

pub use iter::{MutableCsrIterator, VertexEdgesIter};
pub use overflow::OverflowStorage;
pub(crate) use serialization::{read_nbr, write_nbr};

use overflow::MAX_OVERFLOW_CHUNKS_PER_VERTEX;
use serialization::MUTABLE_CSR_FORMAT_VERSION;

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const DEFAULT_EDGE_CAPACITY: usize = 4096;
const DEFAULT_VERTEX_DEGREE: usize = 4;
const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;
const VERTEX_GROWTH_FACTOR: f64 = 1.25;

/// Target density for packed rows: live entries per unit of reserved row
/// capacity. Rebuilds size rows to `ceil(live / PACKED_CSR_DENSITY)` so
/// everyday writes land in row gaps before spilling to overflow.
pub(crate) const PACKED_CSR_DENSITY: f32 = 0.8;

pub struct MutableCsr {
    nbr_list: Vec<Nbr>,
    adj_offsets: Vec<u32>,
    degrees: Vec<u32>,
    primary_capacities: Vec<u32>,

    overflow_chunks: OverflowStorage,
    overflow_chunk_edges: usize,
    /// Live endpoint set for overflow vertices: (endpoint, rank) of edges
    /// whose `delete_ts == MAX`. Enables O(1) duplicate detection for
    /// high-degree vertices instead of scanning all overflow blocks.
    overflow_live_sets: HashMap<u32, HashSet<(u32, i64)>>,
    /// Live endpoint set for primary rows, mirroring the overflow sets.
    /// Checked first on insert so high-degree vertices never pay a primary
    /// linear scan when the set is present; rebuilt on load and compact.
    primary_live_sets: HashMap<u32, HashSet<(u32, i64)>>,

    edge_count: AtomicU64,
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
            overflow_live_sets: self.overflow_live_sets.clone(),
            primary_live_sets: self.primary_live_sets.clone(),
            edge_count: AtomicU64::new(self.edge_count.load(Ordering::Relaxed)),
            total_edge_capacity: self.total_edge_capacity,
        }
    }
}

impl fmt::Debug for MutableCsr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutableCsr")
            .field("vertex_capacity", &self.vertex_capacity())
            .field("total_edge_capacity", &self.total_edge_capacity)
            .field("edge_count", &self.edge_count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl MutableCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY, DEFAULT_EDGE_CAPACITY)
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
            nbr_list: Vec::with_capacity(edge_cap),
            adj_offsets: vec![0; vertex_cap],
            degrees: vec![0; vertex_cap],
            primary_capacities: vec![0; vertex_cap],
            overflow_chunks: OverflowStorage::new(),
            overflow_chunk_edges: overflow_chunk_edges.max(1),
            overflow_live_sets: HashMap::new(),
            primary_live_sets: HashMap::new(),
            edge_count: AtomicU64::new(0),
            total_edge_capacity: 0,
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.adj_offsets.len()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    /// Resize vertex capacity (requires exclusive access)
    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity() {
            return;
        }

        let tail = self.nbr_list.len() as u32;
        self.adj_offsets.resize(new_vertex_capacity, tail);
        self.degrees.resize(new_vertex_capacity, 0);
        self.primary_capacities.resize(new_vertex_capacity, 0);
    }

    /// Ensure vertex capacity (grows if needed)
    pub fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity() {
            let new_capacity =
                ((min_capacity as f64 * VERTEX_GROWTH_FACTOR).ceil() as usize).max(min_capacity);
            self.resize(new_capacity);
        }
    }

    /// Get overflow chunks for a vertex.
    pub fn get_overflow_chunks(&self, vid: u32) -> Option<&Vec<Vec<Nbr>>> {
        self.overflow_chunks.get(&vid)
    }

    fn rebuild_overflow_live_sets(&mut self) {
        self.overflow_live_sets.clear();
        for (vid, chunks) in self.overflow_chunks.iter() {
            let mut set = HashSet::new();
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
                        set.insert((nbr.endpoint, nbr.rank));
                    }
                }
            }
            if !set.is_empty() {
                self.overflow_live_sets.insert(*vid, set);
            }
        }
    }

    fn track_overflow_live_insert(&mut self, vid: u32, endpoint: u32, rank: i64) {
        self.overflow_live_sets
            .entry(vid)
            .or_default()
            .insert((endpoint, rank));
    }

    fn track_overflow_live_remove(&mut self, vid: u32, endpoint: u32, rank: i64) {
        if let Some(set) = self.overflow_live_sets.get_mut(&vid) {
            set.remove(&(endpoint, rank));
            if set.is_empty() {
                self.overflow_live_sets.remove(&vid);
            }
        }
    }

    fn rebuild_primary_live_sets(&mut self) {
        self.primary_live_sets.clear();
        for vid in 0..self.vertex_capacity() {
            let degree = self.degrees[vid] as usize;
            let offset = self.adj_offsets[vid] as usize;
            let mut set = HashSet::new();
            for i in 0..degree {
                if let Some(nbr) = self.nbr_list.get(offset + i) {
                    if nbr.delete_ts == Timestamp::MAX {
                        set.insert((nbr.endpoint, nbr.rank));
                    }
                }
            }
            if !set.is_empty() {
                self.primary_live_sets.insert(vid as u32, set);
            }
        }
    }

    fn rebuild_live_sets(&mut self) {
        self.rebuild_overflow_live_sets();
        self.rebuild_primary_live_sets();
    }

    fn track_primary_live_insert(&mut self, vid: u32, endpoint: u32, rank: i64) {
        self.primary_live_sets
            .entry(vid)
            .or_default()
            .insert((endpoint, rank));
    }

    fn track_primary_live_remove(&mut self, vid: u32, endpoint: u32, rank: i64) {
        if let Some(set) = self.primary_live_sets.get_mut(&vid) {
            set.remove(&(endpoint, rank));
            if set.is_empty() {
                self.primary_live_sets.remove(&vid);
            }
        }
    }

    fn rebuild_primary_live_set_for_vertex(&mut self, vid: u32) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            self.primary_live_sets.remove(&vid);
            return;
        }
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        let mut set = HashSet::new();
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.delete_ts == Timestamp::MAX {
                    set.insert((nbr.endpoint, nbr.rank));
                }
            }
        }
        if set.is_empty() {
            self.primary_live_sets.remove(&vid);
        } else {
            self.primary_live_sets.insert(vid, set);
        }
    }

    fn compact_overflow_for_vertex(&mut self, vid: u32) {
        let Some(chunks) = self.overflow_chunks.get(&vid).cloned() else {
            return;
        };
        let mut live: Vec<Nbr> = Vec::new();
        for chunk in &chunks {
            for nbr in chunk {
                if nbr.delete_ts == Timestamp::MAX {
                    live.push(*nbr);
                }
            }
        }
        if live.is_empty() {
            // Remove empty overflow entry entirely to reclaim metadata.
            self.overflow_chunks.remove(&vid);
            self.overflow_live_sets.remove(&vid);
            return;
        }
        // Repack live entries into fresh chunks.
        let mut new_chunks: Vec<Vec<Nbr>> = Vec::new();
        for chunk in live.chunks(self.overflow_chunk_edges) {
            let mut v = Vec::with_capacity(self.overflow_chunk_edges);
            v.extend_from_slice(chunk);
            new_chunks.push(v);
        }
        // Update capacity accounting: old capacity vs new.
        let old_cap: usize = chunks.iter().map(|c| c.capacity()).sum();
        let new_cap: usize = new_chunks.iter().map(|c| c.capacity()).sum();
        self.total_edge_capacity = self
            .total_edge_capacity
            .saturating_sub(old_cap)
            .saturating_add(new_cap);
        if let Some(slot) = self.overflow_chunks.get_mut(&vid) {
            *slot = new_chunks;
        }
        // Rebuild live set for this vertex.
        let mut set = HashSet::new();
        if let Some(new_chunks_ref) = self.overflow_chunks.get(&vid) {
            for chunk in new_chunks_ref {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
                        set.insert((nbr.endpoint, nbr.rank));
                    }
                }
            }
        }
        if set.is_empty() {
            self.overflow_live_sets.remove(&vid);
        } else {
            self.overflow_live_sets.insert(vid, set);
        }
    }

    /// Allocate the primary block of `DEFAULT_VERTEX_DEGREE` slots for a vertex
    /// on its first edge. Zero-degree vertices hold no slots in `nbr_list`.
    fn allocate_primary_block(&mut self, src_idx: usize) {
        let block_offset = self.nbr_list.len();
        self.nbr_list.resize(
            block_offset + DEFAULT_VERTEX_DEGREE,
            Nbr::new(0, 0, EdgeId(0)),
        );
        self.adj_offsets[src_idx] = block_offset as u32;
        self.primary_capacities[src_idx] = DEFAULT_VERTEX_DEGREE as u32;
        self.total_edge_capacity = self
            .total_edge_capacity
            .saturating_add(DEFAULT_VERTEX_DEGREE);
    }

    fn append_overflow(&mut self, src_vid: u32, nbr: Nbr) {
        let chunks = self.overflow_chunks.get_or_create(src_vid);
        let needs_chunk = chunks
            .last()
            .is_none_or(|chunk| chunk.len() >= self.overflow_chunk_edges);
        if needs_chunk {
            chunks.push(Vec::with_capacity(self.overflow_chunk_edges));
            self.total_edge_capacity = self
                .total_edge_capacity
                .saturating_add(self.overflow_chunk_edges);
        }
        if let Some(chunk) = chunks.last_mut() {
            chunk.push(nbr);
        }
        if nbr.delete_ts == Timestamp::MAX {
            self.track_overflow_live_insert(src_vid, nbr.endpoint, nbr.rank);
        }
        // Per-vertex overflow compaction: if chunk count exceeds threshold,
        // reclaim dead entries and repack to bound scan cost.
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            if chunks.len() > MAX_OVERFLOW_CHUNKS_PER_VERTEX {
                let dead = chunks
                    .iter()
                    .flat_map(|c| c.iter())
                    .filter(|nbr| nbr.delete_ts != Timestamp::MAX)
                    .count();
                if dead > 0 {
                    self.compact_overflow_for_vertex(src_vid);
                } else if chunks.len() > MAX_OVERFLOW_CHUNKS_PER_VERTEX * 2 {
                    log::warn!(
                        "MutableCsr vertex {} overflow chunks {} exceeds limit without dead entries; consider compaction",
                        src_vid,
                        chunks.len()
                    );
                }
            }
        }
    }

    /// Insert an edge with automatic capacity expansion
    pub fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (decoded_vid, decoded_rank) = dst.decode_edge_endpoint();
        let decoded_endpoint = decoded_vid.as_u64().unwrap_or(0) as u32;

        let src_idx = src_vid as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        // Lazy primary block allocation on first edge
        if self.primary_capacities[src_idx] == 0 {
            self.allocate_primary_block(src_idx);
        }

        // Duplicate check via O(1) live sets first; fallback to scans only
        // when a set is missing for this vertex.
        if let Some(set) = self.primary_live_sets.get(&src_vid) {
            if set.contains(&(decoded_endpoint, decoded_rank)) {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
        } else {
            let degree = self.degrees[src_idx] as usize;
            let base = self.adj_offsets[src_idx] as usize;
            for i in 0..degree {
                let nbr = &self.nbr_list[base + i];
                if nbr.endpoint == decoded_endpoint
                    && nbr.rank == decoded_rank
                    && nbr.delete_ts == Timestamp::MAX
                {
                    return Err(StorageError::edge_already_exists(format!(
                        "{} -> {:?}",
                        src_vid, dst
                    )));
                }
            }
        }
        // Overflow duplicate check via O(1) live set; fallback to scan if
        // set is missing (e.g., after manual load before rebuild).
        if let Some(set) = self.overflow_live_sets.get(&src_vid) {
            if set.contains(&(decoded_endpoint, decoded_rank)) {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
        } else if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.endpoint == decoded_endpoint
                        && nbr.rank == decoded_rank
                        && nbr.delete_ts == Timestamp::MAX
                    {
                        return Err(StorageError::edge_already_exists(format!(
                            "{} -> {:?}",
                            src_vid, dst
                        )));
                    }
                }
            }
        }

        // Record create_ts in the Nbr before writing
        let nbr_with_ts = Nbr::with_create_ts(decoded_endpoint, decoded_rank, edge_id, ts);

        // Write to primary if space available and overflow not yet allocated
        let degree = self.degrees[src_idx] as usize;
        let base = self.adj_offsets[src_idx] as usize;
        if self.overflow_chunks.get(&src_vid).is_none_or(Vec::is_empty)
            && degree < self.primary_capacities[src_idx] as usize
        {
            self.nbr_list[base + degree] = nbr_with_ts;
            self.degrees[src_idx] += 1;
            self.track_primary_live_insert(src_vid, decoded_endpoint, decoded_rank);
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        self.append_overflow(src_vid, nbr_with_ts);
        self.edge_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn scan_overflow_for_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(usize, usize)> {
        self.overflow_chunks
            .get(&src_vid)?
            .iter()
            .enumerate()
            .find_map(|(chunk_idx, chunk)| {
                chunk
                    .iter()
                    .position(|nbr| nbr.edge_id == edge_id)
                    .map(|edge_idx| (chunk_idx, edge_idx))
            })
    }

    fn scan_overflow_for_dst(&self, src_vid: u32, dst: VertexId) -> Vec<(usize, usize)> {
        let (decoded_vid, decoded_rank) = dst.decode_edge_endpoint();
        let decoded_endpoint = decoded_vid.as_u64().unwrap_or(0) as u32;
        let mut result = Vec::new();
        let Some(chunks) = self.overflow_chunks.get(&src_vid) else {
            return result;
        };
        for (chunk_idx, chunk) in chunks.iter().enumerate() {
            for (edge_idx, nbr) in chunk.iter().enumerate() {
                if nbr.endpoint == decoded_endpoint && nbr.rank == decoded_rank {
                    result.push((chunk_idx, edge_idx));
                }
            }
        }
        result
    }

    /// Delete an edge by edge_id.
    ///
    /// Returns `Ok(true)` when deleted, `Ok(false)` when the edge does not
    /// exist or is not deletable at `ts`, and
    /// `Err(StorageError::write_write_conflict)` when the edge was already
    /// deleted at a different timestamp (write-write conflict at the storage
    /// layer, surfaced immediately instead of a silent `false`).
    pub fn delete_edge(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.edge_id == edge_id {
                if nbr.delete_ts != Timestamp::MAX {
                    if nbr.delete_ts != ts {
                        return Err(StorageError::write_write_conflict(format!(
                            "edge {:?} already deleted at ts={}, attempted delete at ts={}",
                            edge_id, nbr.delete_ts, ts
                        )));
                    }
                    // Idempotent re-delete at the same timestamp.
                    return Ok(false);
                }
                let create_ts = nbr.create_ts;
                if create_ts <= ts {
                    let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                    self.track_primary_live_remove(src_vid, endpoint, rank);
                    return Ok(true);
                }
                // Cannot delete an edge that is not yet created at `ts`.
                return Ok(false);
            }
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            // Capture endpoint/rank before mutable borrow ends for live set update.
            let (endpoint, rank) = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                let n = &chunks[chunk_idx][edge_idx];
                (n.endpoint, n.rank)
            };
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                if nbr.delete_ts != Timestamp::MAX {
                    if nbr.delete_ts != ts {
                        return Err(StorageError::write_write_conflict(format!(
                            "edge {:?} already deleted at ts={}, attempted delete at ts={}",
                            edge_id, nbr.delete_ts, ts
                        )));
                    }
                    return Ok(false);
                }
                let create_ts = nbr.create_ts;
                if create_ts <= ts {
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                    self.track_overflow_live_remove(src_vid, endpoint, rank);
                    return Ok(true);
                }
                return Ok(false);
            }
        }

        Ok(false)
    }

    /// Delete edges by destination vertex with full-match semantics.
    ///
    /// Deletes every live match and returns the deleted count so table
    /// rollback can reconcile by count. One call deletes the whole match.
    pub fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        let (decoded_vid, decoded_rank) = dst.decode_edge_endpoint();
        let decoded_endpoint = decoded_vid.as_u64().unwrap_or(0) as u32;
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let mut deleted = 0usize;

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.endpoint == decoded_endpoint
                && nbr.rank == decoded_rank
                && nbr.delete_ts == Timestamp::MAX
            {
                let create_ts = nbr.create_ts;
                if create_ts <= ts {
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                    self.track_primary_live_remove(src_vid, decoded_endpoint, decoded_rank);
                    deleted += 1;
                }
            }
        }

        // Scan overflow
        let indices = self.scan_overflow_for_dst(src_vid, dst);
        // Collect endpoints for set removal before mutable borrow.
        let mut overflow_deleted_endpoints: Vec<(u32, i64)> = Vec::new();
        if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
            for (chunk_idx, edge_idx) in indices {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                if nbr.delete_ts == Timestamp::MAX {
                    let create_ts = nbr.create_ts;
                    if create_ts <= ts {
                        let ep = nbr.endpoint;
                        let rk = nbr.rank;
                        nbr.delete_ts = ts;
                        self.edge_count.fetch_sub(1, Ordering::Relaxed);
                        overflow_deleted_endpoints.push((ep, rk));
                        deleted += 1;
                    }
                }
            }
        }
        for (ep, rk) in overflow_deleted_endpoints {
            self.track_overflow_live_remove(src_vid, ep, rk);
        }

        deleted
    }

    pub fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if offset < 0 {
            return Ok(false);
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.primary_capacities[src_idx] == 0 {
            return Ok(false);
        }
        if offset as usize >= self.degrees[src_idx] as usize {
            return Ok(false);
        }
        let idx = self.adj_offsets[src_idx] as usize + offset as usize;
        if idx >= self.nbr_list.len() {
            return Ok(false);
        }
        let nbr = &mut self.nbr_list[idx];
        if nbr.delete_ts == Timestamp::MAX {
            let create_ts = nbr.create_ts;
            if create_ts <= ts {
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                self.track_primary_live_remove(src_vid, endpoint, rank);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Revert a deleted edge by offset position in the primary block.
    ///
    /// Only reverts deletions that occurred at or before the given timestamp.
    /// This maintains MVCC semantics during transaction rollback: we can only
    /// undo deletions that happened before the rollback point.
    pub fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if offset < 0 {
            return false;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.primary_capacities[src_idx] == 0 {
            return false;
        }
        if offset as usize >= self.degrees[src_idx] as usize {
            return false;
        }

        let base_offset = self.adj_offsets[src_idx] as usize;
        let idx = base_offset + offset as usize;

        if idx >= self.nbr_list.len() {
            return false;
        }

        let nbr = &mut self.nbr_list[idx];
        // Only revert deletions that happened at or before rollback time.
        // Prevents rolling back deletions that occur after the rollback point.
        if nbr.delete_ts < Timestamp::MAX && nbr.delete_ts <= ts {
            let (endpoint, rank) = (nbr.endpoint, nbr.rank);
            nbr.delete_ts = Timestamp::MAX;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            self.track_primary_live_insert(src_vid, endpoint, rank);
            return true;
        }
        false
    }

    /// Read-only view of one primary slot without mutating state.
    ///
    /// Used to verify that a caller-supplied offset still addresses the
    /// expected edge before a destructive offset write runs.
    pub fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
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
        self.nbr_list.get(idx).copied()
    }

    /// Locate one edge by endpoint without consulting timestamps.
    ///
    /// Physical addressing only; visibility is decided by the version
    /// authority above this layer.
    pub fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (decoded_vid, decoded_rank) = dst.decode_edge_endpoint();
        let decoded_endpoint = decoded_vid.as_u64().unwrap_or(0) as u32;
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.endpoint == decoded_endpoint && nbr.rank == decoded_rank {
                    return Some(*nbr);
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.endpoint == decoded_endpoint && nbr.rank == decoded_rank {
                        return Some(*nbr);
                    }
                }
            }
        }
        None
    }

    /// Every physically stored entry of one vertex without timestamp filtering.
    ///
    /// Visibility is decided by the version authority above this layer.
    pub fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        let mut out = Vec::new();
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                out.push(*nbr);
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                out.extend_from_slice(chunk);
            }
        }
        out
    }

    /// Visit every physically stored entry of one vertex without allocating.
    ///
    /// The visitor returns false to stop early. Visibility is decided by the
    /// version authority above this layer.
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if !f(*nbr) {
                    return;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if !f(*nbr) {
                        return;
                    }
                }
            }
        }
    }

    /// Whether the primary row of one vertex holds `edge_id`.
    ///
    /// Used to distinguish a stale offset (edge lives in primary at a
    /// different offset, must fail) from an overflow row (no offset can
    /// address it, may fall back to the edge-id path).
    pub fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if self.nbr_list.get(offset + i).is_some_and(|nbr| nbr.edge_id == edge_id) {
                return true;
            }
        }
        false
    }

    /// Whether one vertex holds any physically stored entry.
    pub fn has_physical_entries(&self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return false;
        }
        if self.degrees[idx] > 0 {
            return true;
        }
        self.overflow_chunks
            .get(&vid)
            .is_some_and(|chunks| chunks.iter().any(|c| !c.is_empty()))
    }

    /// Physically remove an edge by edge id from primary or overflow.
    ///
    /// Reclaims the slot and updates degree/edge count; no tombstone trace is
    /// left behind. Used to roll back the out-direction when the in-direction
    /// insertion fails.
    pub fn remove_edge(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if self.nbr_list[offset + i].edge_id == edge_id {
                let was_live = self.nbr_list[offset + i].delete_ts == Timestamp::MAX;
                let (endpoint, rank) = {
                    let n = &self.nbr_list[offset + i];
                    (n.endpoint, n.rank)
                };
                // Shift left to close the gap, then decrement the degree.
                for j in i..degree - 1 {
                    self.nbr_list[offset + j] = self.nbr_list[offset + j + 1];
                }
                self.degrees[src_idx] -= 1;
                if was_live {
                    self.track_primary_live_remove(src_vid, endpoint, rank);
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                }
                return true;
            }
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            // Capture live status before removal for set maintenance.
            let was_live = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                chunks[chunk_idx][edge_idx].delete_ts == Timestamp::MAX
            };
            let (endpoint, rank) = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                let n = &chunks[chunk_idx][edge_idx];
                (n.endpoint, n.rank)
            };
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                chunks[chunk_idx].remove(edge_idx);
                // Clean up empty chunk vectors to keep per-vertex chunk count bounded.
                if chunks[chunk_idx].is_empty() {
                    chunks.remove(chunk_idx);
                    self.total_edge_capacity = self
                        .total_edge_capacity
                        .saturating_sub(self.overflow_chunk_edges);
                    if chunks.is_empty() {
                        // Drop the per-vertex entry so later lookups stay constant time.
                        self.overflow_chunks.remove(&src_vid);
                        self.overflow_live_sets.remove(&src_vid);
                        if was_live {
                            self.edge_count.fetch_sub(1, Ordering::Relaxed);
                        }
                        return true;
                    }
                }
                if was_live {
                    self.track_overflow_live_remove(src_vid, endpoint, rank);
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                }
                return true;
            }
        }

        false
    }

    /// Revert a deletion of an edge by edge id.
    ///
    /// Restores `delete_ts` to MAX when the entry was deleted at or before the
    /// given timestamp. Used to roll back the out-direction when the
    /// in-direction deletion fails.
    pub fn revert_delete_by_edge_id(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.edge_id == edge_id && nbr.delete_ts != Timestamp::MAX && nbr.delete_ts <= ts {
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = Timestamp::MAX;
                self.edge_count.fetch_add(1, Ordering::Relaxed);
                self.track_primary_live_insert(src_vid, endpoint, rank);
                return true;
            }
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            let (endpoint, rank) = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                let n = &chunks[chunk_idx][edge_idx];
                (n.endpoint, n.rank)
            };
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                if nbr.delete_ts != Timestamp::MAX && nbr.delete_ts <= ts {
                    nbr.delete_ts = Timestamp::MAX;
                    self.edge_count.fetch_add(1, Ordering::Relaxed);
                    self.track_overflow_live_insert(src_vid, endpoint, rank);
                    return true;
                }
            }
        }

        false
    }

    /// Get edges of a vertex at a given timestamp
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }

        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;

        let total_valid_primary = self.count_valid_primary(src_idx, ts);
        let total_valid_overflow = self.count_valid_overflow(src_vid, ts);
        let mut result = Vec::with_capacity(total_valid_primary + total_valid_overflow);

        for i in 0..degree {
            let nbr = &self.nbr_list[offset + i];
            if nbr.is_alive_at(ts) {
                result.push(*nbr);
            }
        }

        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.is_alive_at(ts) {
                        result.push(*nbr);
                    }
                }
            }
        }

        result
    }

    /// Iterate edges of a vertex without collecting into a Vec.
    /// Test-only row-stamp filtered iterator; production scans go through the version authority.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> VertexEdgesIter<'_> {
        VertexEdgesIter::new(self, src_vid, ts)
    }

    fn count_valid_primary(&self, src_idx: usize, ts: Timestamp) -> usize {
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        let mut count = 0;
        for i in 0..degree {
            let nbr = &self.nbr_list[offset + i];
            if nbr.is_alive_at(ts) {
                count += 1;
            }
        }
        count
    }

    fn count_valid_overflow(&self, src_vid: u32, ts: Timestamp) -> usize {
        self.overflow_chunks
            .get(&src_vid)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|nbr| nbr.is_alive_at(ts))
            .count()
    }

    /// Get a specific edge
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (decoded_vid, decoded_rank) = dst.decode_edge_endpoint();
        let decoded_endpoint = decoded_vid.as_u64().unwrap_or(0) as u32;
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &self.nbr_list[offset + i];
            if nbr.endpoint == decoded_endpoint && nbr.rank == decoded_rank && nbr.is_alive_at(ts) {
                return Some(*nbr);
            }
        }

        // Scan overflow
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.endpoint == decoded_endpoint
                        && nbr.rank == decoded_rank
                        && nbr.is_alive_at(ts)
                    {
                        return Some(*nbr);
                    }
                }
            }
        }

        None
    }

    /// Clear all edges
    pub fn clear(&mut self) {
        self.degrees.fill(0);
        self.overflow_chunks.clear();
        self.overflow_live_sets.clear();
        self.primary_live_sets.clear();
        self.total_edge_capacity = self
            .primary_capacities
            .iter()
            .map(|cap| *cap as usize)
            .sum();
        self.edge_count.store(0, Ordering::Relaxed);
    }

    /// Create iterator over all edges
    pub fn iter(&self, ts: Timestamp) -> MutableCsrIterator<'_> {
        MutableCsrIterator::new(self, ts)
    }

    /// Create an iterator over all physically present edges, including
    /// entries marked as deleted (delete_ts != MAX). Used when rebuilding the
    /// CSR so tombstoned entries survive remapping.
    pub fn iter_all(&self) -> MutableCsrIterator<'_> {
        MutableCsrIterator::new_all(self)
    }

    /// Dump to bytes
    ///
    /// Format:
    /// - format_version (u32)
    /// - vertex_capacity (u64)
    /// - edge_count (u64)
    /// - total_edge_capacity (u64)
    /// - adj_offsets (u32 * vertex_capacity)
    /// - degrees (u32 * vertex_capacity)
    /// - primary_capacities (u32 * vertex_capacity)
    /// - overflow_chunk_edges (u64)
    /// - primary neighbor list
    /// - per-vertex overflow chunks
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();

        result.extend_from_slice(&MUTABLE_CSR_FORMAT_VERSION.to_le_bytes());
        result.extend_from_slice(&(self.adj_offsets.len() as u64).to_le_bytes());
        result.extend_from_slice(&self.edge_count.load(Ordering::Relaxed).to_le_bytes());
        result.extend_from_slice(&(self.nbr_list.len() as u64).to_le_bytes());
        result.extend_from_slice(&(self.overflow_chunk_edges as u64).to_le_bytes());

        for &offset in &self.adj_offsets {
            result.extend_from_slice(&offset.to_le_bytes());
        }

        for &degree in &self.degrees {
            result.extend_from_slice(&degree.to_le_bytes());
        }

        for &cap in &self.primary_capacities {
            result.extend_from_slice(&cap.to_le_bytes());
        }

        for nbr in &self.nbr_list {
            write_nbr(&mut result, nbr);
        }

        for vid in 0..self.adj_offsets.len() {
            let chunks = self.overflow_chunks.get(&(vid as u32));
            result.extend_from_slice(&(chunks.map_or(0, Vec::len) as u32).to_le_bytes());
            if let Some(chunks) = chunks {
                for chunk in chunks {
                    result.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                    for nbr in chunk {
                        write_nbr(&mut result, nbr);
                    }
                }
            }
        }

        result
    }

    /// Load from bytes
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 36 {
            return Err(StorageError::deserialize_error(
                "CSR data too short for header",
            ));
        }

        let mut offset = 0usize;

        let format_version = read_u32_le(data, &mut offset)?;
        if format_version != MUTABLE_CSR_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported mutable CSR format version: {format_version}"
            )));
        }
        let vertex_capacity = read_u64_le(data, &mut offset)? as usize;
        let edge_count = read_u64_le(data, &mut offset)?;
        let primary_edge_capacity = read_u64_le(data, &mut offset)? as usize;
        let overflow_chunk_edges = read_u64_le(data, &mut offset)? as usize;
        if overflow_chunk_edges == 0 {
            return Err(StorageError::deserialize_error(
                "Mutable CSR overflow chunk size must be greater than zero",
            ));
        }

        let mut adj_offsets = Vec::with_capacity(vertex_capacity);
        for _ in 0..vertex_capacity {
            adj_offsets.push(read_u32_le(data, &mut offset)?);
        }

        let mut degrees = Vec::with_capacity(vertex_capacity);
        for _ in 0..vertex_capacity {
            degrees.push(read_u32_le(data, &mut offset)?);
        }

        let mut primary_capacities = Vec::with_capacity(vertex_capacity);
        for _ in 0..vertex_capacity {
            primary_capacities.push(read_u32_le(data, &mut offset)?);
        }

        let mut nbr_list = Vec::with_capacity(primary_edge_capacity);
        for _ in 0..primary_edge_capacity {
            nbr_list.push(read_nbr(data, &mut offset)?);
        }

        let mut overflow_chunks = OverflowStorage::new();
        let mut overflow_capacity = 0usize;
        for vid in 0..vertex_capacity {
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let chunk_len = read_u32_le(data, &mut offset)? as usize;
                if chunk_len > overflow_chunk_edges {
                    return Err(StorageError::deserialize_error(
                        "Mutable CSR overflow chunk exceeds configured chunk size",
                    ));
                }
                let mut chunk = Vec::with_capacity(overflow_chunk_edges);
                for _ in 0..chunk_len {
                    chunk.push(read_nbr(data, &mut offset)?);
                }
                overflow_capacity = overflow_capacity.saturating_add(overflow_chunk_edges);
                chunks.push(chunk);
            }
            if !chunks.is_empty() {
                overflow_chunks.insert(vid as u32, chunks);
            }
        }

        self.total_edge_capacity = primary_edge_capacity.saturating_add(overflow_capacity);
        self.adj_offsets = adj_offsets;
        self.degrees = degrees;
        self.primary_capacities = primary_capacities;
        self.overflow_chunks = overflow_chunks;
        self.overflow_chunk_edges = overflow_chunk_edges;
        self.nbr_list = nbr_list;
        self.edge_count.store(edge_count, Ordering::Relaxed);
        self.rebuild_live_sets();

        Ok(())
    }

    /// Compact CSR, dropping entries eligible under
    /// `Visibility::is_gc_eligible` and merging overflow into primary.
    /// Removed entries are reported via the callback for tombstone promotion;
    /// with `cutoff == MAX` nothing is dropped.
    pub fn compact_with_ts(&mut self, cutoff: Timestamp, reserve_ratio: f32) -> usize {
        self.compact_with_ts_reporting(cutoff, reserve_ratio, &mut |_, _| {})
    }

    /// Compact with per-edge removal reporting (`on_edge_removed` receives
    /// each dropped edge id and delete timestamp).
    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        // Without an active snapshot cutoff no deletion may be dropped.
        let removals_enabled = cutoff < Timestamp::MAX;

        // Compact individual vertex data (primary + overflow)
        // and compute new layout.
        let mut new_offsets = Vec::with_capacity(self.vertex_capacity());
        let mut new_degrees = Vec::with_capacity(self.vertex_capacity());
        let mut new_capacities = Vec::with_capacity(self.vertex_capacity());
        let mut new_edges = Vec::<Nbr>::new();
        let mut removed_count = 0usize;

        for vid in 0..self.vertex_capacity() {
            let start = self.adj_offsets[vid] as usize;
            let degree = self.degrees[vid] as usize;

            new_offsets.push(new_edges.len());

            // Collect active edges from primary (not deleted)
            for i in 0..degree {
                let nbr = &self.nbr_list[start + i];
                if nbr.delete_ts != Timestamp::MAX
                    && removals_enabled
                    && crate::mvcc_visibility::Visibility::is_gc_eligible(nbr.delete_ts, cutoff)
                {
                    on_edge_removed(nbr.edge_id, nbr.delete_ts);
                    removed_count += 1;
                } else {
                    new_edges.push(*nbr);
                }
            }

            // Collect active edges from overflow
            if let Some(chunks) = self.overflow_chunks.get(&(vid as u32)) {
                for chunk in chunks {
                    for nbr in chunk {
                        if nbr.delete_ts != Timestamp::MAX
                            && removals_enabled
                            && crate::mvcc_visibility::Visibility::is_gc_eligible(
                                nbr.delete_ts,
                                cutoff,
                            )
                        {
                            on_edge_removed(nbr.edge_id, nbr.delete_ts);
                            removed_count += 1;
                        } else {
                            new_edges.push(*nbr);
                        }
                    }
                }
            }

            let valid = new_edges.len() - new_offsets[vid];
            new_degrees.push(valid as u32);
            // Guard against reserve_ratio >= 1.0 (division by zero would yield
            // infinity, saturating the cast to u32::MAX and exploding the
            // rebuilt CSR allocation). Treat it as "no reserve".
            let new_cap = if valid > 0 {
                if reserve_ratio < 1.0 {
                    ((valid as f32 / (1.0 - reserve_ratio)).ceil() as u32).max(1)
                } else {
                    (valid as u32).max(1)
                }
            } else {
                0
            };
            new_capacities.push(new_cap);
        }

        // Rebuild nbr_list as flat CSR (no overflow)
        let new_total_edge_capacity: usize = new_capacities.iter().map(|&c| c as usize).sum();
        let mut new_nbr_list = Vec::with_capacity(new_total_edge_capacity);
        let mut final_offsets = Vec::with_capacity(self.vertex_capacity());

        for vid in 0..self.vertex_capacity() {
            final_offsets.push(new_nbr_list.len() as u32);
            let off = new_offsets[vid];
            let deg = new_degrees[vid] as usize;
            let cap = new_capacities[vid] as usize;

            new_nbr_list.extend_from_slice(&new_edges[off..off + deg]);
            // Fill remaining capacity with empty Nbr
            let remaining = cap - deg;
            if remaining > 0 {
                new_nbr_list.resize(new_nbr_list.len() + remaining, Nbr::new(0, 0, EdgeId(0)));
            }
        }

        self.nbr_list = new_nbr_list;
        self.adj_offsets = final_offsets;
        self.degrees = new_degrees;
        self.primary_capacities = new_capacities;
        self.total_edge_capacity = new_total_edge_capacity;

        self.overflow_chunks = OverflowStorage::new();
        self.overflow_live_sets.clear();
        self.rebuild_primary_live_sets();

        removed_count
    }

    /// Count entries of one vertex reclaimable at `cutoff`.
    ///
    /// Only tombstones eligible under the shared collection predicate
    /// count; tombstones pinned by older snapshots are left alone.
    pub fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        let mut count = 0;
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.delete_ts != Timestamp::MAX
                    && crate::mvcc_visibility::Visibility::is_gc_eligible(nbr.delete_ts, cutoff)
                {
                    count += 1;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.delete_ts != Timestamp::MAX
                        && crate::mvcc_visibility::Visibility::is_gc_eligible(nbr.delete_ts, cutoff)
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    /// Whether one vertex holds anything reclaimable at `cutoff`.
    pub fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        self.reclaimable_count(vid, cutoff) > 0
    }

    /// Physical entry census of one vertex: `(live, dead, capacity)`.
    ///
    /// `live` counts entries with no deletion stamp, `dead` counts
    /// tombstoned entries regardless of eligibility, and `capacity` is the
    /// reserved row capacity plus allocated overflow chunks.
    pub fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (0, 0, 0);
        }
        let mut live = 0usize;
        let mut dead = 0usize;
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.delete_ts == Timestamp::MAX {
                    live += 1;
                } else {
                    dead += 1;
                }
            }
        }
        let mut capacity = self.primary_capacities[idx] as usize;
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            capacity += chunks.len() * self.overflow_chunk_edges;
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
                        live += 1;
                    } else {
                        dead += 1;
                    }
                }
            }
        }
        (live, dead, capacity)
    }

    /// Reclaim one vertex in place, leaving every other row untouched.
    ///
    /// Eligible tombstones are dropped and reported through
    /// `on_edge_removed`; live entries and pinned tombstones are tightened
    /// to the front of their current slots. Row offsets and capacities of
    /// other vertices never move, so the work stays proportional to the
    /// degree of `vid` instead of the size of the table.
    pub fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        let mut removed = 0usize;

        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        let mut keep = 0usize;
        for i in 0..degree {
            let drop = self.nbr_list.get(offset + i).is_some_and(|nbr| {
                nbr.delete_ts != Timestamp::MAX
                    && crate::mvcc_visibility::Visibility::is_gc_eligible(nbr.delete_ts, cutoff)
            });
            if drop {
                let nbr = self.nbr_list[offset + i];
                on_edge_removed(nbr.edge_id, nbr.delete_ts);
                removed += 1;
            } else {
                if keep != i {
                    self.nbr_list[offset + keep] = self.nbr_list[offset + i];
                }
                keep += 1;
            }
        }
        self.degrees[idx] = keep as u32;

        if self.overflow_chunks.get(&vid).is_some() {
            let chunks = self.overflow_chunks.get(&vid).cloned().unwrap_or_default();
            let old_chunk_count = chunks.len();
            let mut kept: Vec<Nbr> = Vec::new();
            for chunk in &chunks {
                for nbr in chunk {
                    if nbr.delete_ts != Timestamp::MAX
                        && crate::mvcc_visibility::Visibility::is_gc_eligible(nbr.delete_ts, cutoff)
                    {
                        on_edge_removed(nbr.edge_id, nbr.delete_ts);
                        removed += 1;
                    } else {
                        kept.push(*nbr);
                    }
                }
            }
            if kept.is_empty() {
                self.overflow_chunks.remove(&vid);
                self.total_edge_capacity = self
                    .total_edge_capacity
                    .saturating_sub(old_chunk_count * self.overflow_chunk_edges);
                self.overflow_live_sets.remove(&vid);
            } else {
                let mut repacked: Vec<Vec<Nbr>> = Vec::new();
                for piece in kept.chunks(self.overflow_chunk_edges) {
                    let mut v = Vec::with_capacity(self.overflow_chunk_edges);
                    v.extend_from_slice(piece);
                    repacked.push(v);
                }
                let new_chunk_count = repacked.len();
                self.total_edge_capacity = self
                    .total_edge_capacity
                    .saturating_sub(old_chunk_count * self.overflow_chunk_edges)
                    .saturating_add(new_chunk_count * self.overflow_chunk_edges);
                if let Some(slot) = self.overflow_chunks.get_mut(&vid) {
                    *slot = repacked;
                }
                let mut set = HashSet::new();
                if let Some(current) = self.overflow_chunks.get(&vid) {
                    for chunk in current {
                        for nbr in chunk {
                            if nbr.delete_ts == Timestamp::MAX {
                                set.insert((nbr.endpoint, nbr.rank));
                            }
                        }
                    }
                }
                if set.is_empty() {
                    self.overflow_live_sets.remove(&vid);
                } else {
                    self.overflow_live_sets.insert(vid, set);
                }
            }
        }
        if removed > 0 {
            self.rebuild_primary_live_set_for_vertex(vid);
        }

        removed
    }

    /// Get used memory size (active edges only)
    pub fn used_memory_size(&self) -> usize {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        active_edges * std::mem::size_of::<Nbr>() + std::mem::size_of::<Self>()
    }

    /// Compute fragmentation ratio: reserved capacity over live edges.
    ///
    /// A ratio > 1.5 indicates moderate fragmentation; > 2.0 suggests
    /// collection. Returns 0.0 if no live edges. This whole-table ratio is
    /// an observation metric; the write path triggers on per-vertex
    /// reclaimable counts instead.
    pub fn fragmentation_ratio(&self) -> f32 {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        if active_edges == 0 {
            return 0.0;
        }
        self.total_edge_capacity as f32 / active_edges as f32
    }

    /// Estimate wasted memory due to fragmentation (in bytes)
    pub(crate) fn wasted_bytes_estimate(&self) -> usize {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        self.total_edge_capacity.saturating_sub(active_edges) * std::mem::size_of::<Nbr>()
    }

    /// Get detailed fragmentation statistics.
    ///
    /// Both counters derive from the live structures: dead entries are the
    /// physically stored entries minus live edges (primary tombstones plus
    /// overflow dead entries), and wasted capacity is the reserved capacity
    /// minus live edges (row gaps plus tombstone slots).
    pub fn get_fragmentation_stats(&self) -> super::FragmentationStats {
        let live_edges = self.edge_count.load(Ordering::Relaxed) as usize;

        let mut physical_entries = 0usize;
        for vid in 0..self.vertex_capacity() {
            physical_entries += self.degrees[vid] as usize;
        }
        for (_, chunks) in self.overflow_chunks.iter() {
            for chunk in chunks {
                physical_entries += chunk.len();
            }
        }

        let dead_entries = physical_entries.saturating_sub(live_edges);
        let wasted_capacity = self.total_edge_capacity.saturating_sub(live_edges);

        super::FragmentationStats::with_dead_info(
            self.total_edge_capacity,
            live_edges,
            dead_entries,
            wasted_capacity,
        )
    }
}

impl Default for MutableCsr {
    fn default() -> Self {
        Self::new()
    }
}

impl CsrBase for MutableCsr {
    fn vertex_capacity(&self) -> usize {
        MutableCsr::vertex_capacity(self)
    }

    fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    fn dump(&self) -> Vec<u8> {
        MutableCsr::dump(self)
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        MutableCsr::load(self, data)
    }
}

impl MutableCsrTrait for MutableCsr {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        MutableCsr::insert_edge(self, src_vid, dst, edge_id, ts)
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        MutableCsr::delete_edge(self, src_vid, edge_id, ts)
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        MutableCsr::delete_edge_by_dst(self, src_vid, dst, ts)
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        MutableCsr::delete_edge_by_offset(self, src_vid, offset, ts)
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        MutableCsr::revert_delete_by_offset(self, src_vid, offset, ts)
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        MutableCsr::nbr_at_offset(self, src_vid, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        MutableCsr::get_edge_physical(self, src_vid, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        MutableCsr::physical_edges_of(self, src_vid)
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        MutableCsr::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        MutableCsr::primary_contains(self, src_vid, edge_id)
    }

    fn remove_edge(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        MutableCsr::remove_edge(self, src_vid, edge_id)
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        MutableCsr::revert_delete_by_edge_id(self, src_vid, edge_id, ts)
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        MutableCsr::get_edge(self, src_vid, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        MutableCsr::edges_of(self, src_vid, ts)
    }

    fn compact_with_ts(&mut self, ts: Timestamp, reserve_ratio: f32) -> usize {
        MutableCsr::compact_with_ts(self, ts, reserve_ratio)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        MutableCsr::compact_vertex_with_reporting(self, vid, cutoff, on_edge_removed)
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        MutableCsr::reclaimable_count(self, vid, cutoff)
    }

    fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        MutableCsr::vertex_needs_compact(self, vid, cutoff)
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        MutableCsr::vertex_census(self, vid)
    }

    fn used_memory_size(&self) -> usize {
        MutableCsr::used_memory_size(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_insert_and_query() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        csr.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 1)
            .unwrap();

        assert!(csr
            .insert_edge(0u32, VertexId::from_int64(1), EdgeId(103), 1)
            .is_err());

        assert_eq!(csr.edge_count(), 3);
    }

    #[test]
    fn test_delete_edge() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();

        assert!(csr.delete_edge(0u32, EdgeId(100), 2).unwrap());

        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_double_delete_conflict() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 10)
            .unwrap();

        // First delete succeeds.
        assert!(csr.delete_edge(0u32, EdgeId(100), 100).unwrap());
        // Idempotent re-delete at the same timestamp is a no-op, not a conflict.
        assert!(!csr.delete_edge(0u32, EdgeId(100), 100).unwrap());
        // Deleting the same edge at a different timestamp is a write-write
        // conflict, surfaced at the storage write path.
        let err = csr.delete_edge(0u32, EdgeId(100), 200).unwrap_err();
        assert_eq!(
            err.kind(),
            graphdb_core::error::storage::StorageErrorKind::Conflict
        );

        // The edge is still logically deleted at the original timestamp.
        assert_eq!(csr.edges_of(0u32, 50).len(), 1);
        assert_eq!(csr.edges_of(0u32, 150).len(), 0);
    }

    #[test]
    fn test_dump_and_load() {
        let mut csr1 = MutableCsr::with_capacity(10, 100);

        csr1.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr1.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        csr1.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 1)
            .unwrap();

        let data = csr1.dump();

        let mut csr2 = MutableCsr::new();
        let _ = csr2.load(&data);

        assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
        assert_eq!(csr2.edge_count(), csr1.edge_count());
    }

    #[test]
    fn test_resize() {
        let mut csr = MutableCsr::with_capacity(2, 10);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(100u32, VertexId::from_int64(1), EdgeId(101), 1)
            .unwrap();

        assert!(csr.vertex_capacity() >= 101);
    }

    #[test]
    fn test_iterator() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        csr.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 1)
            .unwrap();

        let edges: Vec<_> = csr.iter(1).collect();
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_overflow_insert() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(4), EdgeId(103), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(5), EdgeId(104), 1)
            .unwrap();

        assert_eq!(csr.edge_count(), 5);

        let edges = csr.edges_of(0u32, 1);
        assert_eq!(edges.len(), 5);

        assert!(csr
            .insert_edge(0u32, VertexId::from_int64(5), EdgeId(105), 1)
            .is_err());

        assert!(csr.delete_edge(0u32, EdgeId(104), 2).unwrap());
    }

    #[test]
    fn test_overflow_dump_and_load() {
        let mut csr1 = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr1.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }

        let data = csr1.dump();

        let mut csr2 = MutableCsr::new();
        let _ = csr2.load(&data);

        assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
        assert_eq!(csr2.edge_count(), csr1.edge_count());
        assert_eq!(
            csr2.overflow_chunks
                .get(&0)
                .map_or(0, |chunks| { chunks.iter().map(Vec::len).sum::<usize>() }),
            2
        );
    }

    #[test]
    fn test_compact_with_ts_merges_overflow() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }

        csr.delete_edge(0u32, EdgeId(3), 5).unwrap();
        csr.delete_edge(0u32, EdgeId(5), 5).unwrap();
        csr.delete_edge(0u32, EdgeId(6), 5).unwrap();

        // Cutoff 6: deletions at 5 predate the cutoff, so they are removed.
        let removed = csr.compact_with_ts(6, 0.25);
        assert_eq!(removed, 3);

        assert!(csr.overflow_chunks.get(&0).is_none_or(Vec::is_empty));

        let edges = csr.edges_of(0u32, 3);
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_compact_with_ts_keeps_deleted_entries_without_cutoff() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=3 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }
        csr.delete_edge(0u32, EdgeId(2), 5).unwrap();

        // cutoff == MAX (no active snapshot): the deletion history must be
        // preserved for time-travel queries before the deletion.
        let removed = csr.compact_with_ts(Timestamp::MAX, 0.25);
        assert_eq!(removed, 0);

        assert_eq!(csr.edges_of(0u32, 3).len(), 3);
        assert_eq!(csr.edges_of(0u32, 6).len(), 2);

        // A real cutoff drops the entry again.
        let removed = csr.compact_with_ts(6, 0.25);
        assert_eq!(removed, 1);
        assert_eq!(csr.edges_of(0u32, 3).len(), 2);
    }

    #[test]
    fn test_compact_with_ts_reporting_reports_removed_edges() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=3 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }
        csr.delete_edge(0u32, EdgeId(2), 5).unwrap();

        let mut reported = Vec::new();
        let removed = csr.compact_with_ts_reporting(6, 0.25, &mut |edge_id, delete_ts| {
            reported.push((edge_id, delete_ts));
        });
        assert_eq!(removed, 1);
        assert_eq!(reported, vec![(EdgeId(2), 5)]);
    }

    #[test]
    fn test_compact_with_ts_guards_reserve_ratio_ge_one() {
        // reserve_ratio >= 1.0 used to produce valid / 0.0 = inf, saturating
        // the cast to u32::MAX per vertex and exploding the rebuilt CSR
        // allocation (OOM on ~800k+ edge partitions under background freeze).
        let mut csr = MutableCsr::with_capacity(4, 100);
        for i in 1..=6i64 {
            csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
                .unwrap();
        }
        csr.insert_edge(1u32, VertexId::from_int64(1), EdgeId(7), 1)
            .unwrap();

        let removed = csr.compact_with_ts(3, 1.0);
        assert_eq!(removed, 0);

        let capacity = csr.total_edge_capacity;
        assert!(
            capacity <= 7 + 4,
            "capacity must stay bounded, got {}",
            capacity
        );
        assert_eq!(csr.edges_of(0u32, 3).len(), 6);
        assert_eq!(csr.edges_of(1u32, 3).len(), 1);
    }

    #[test]
    fn test_compact_with_ts_zero_ratio_keeps_exact_degree() {
        let mut csr = MutableCsr::with_capacity(4, 100);
        for i in 1..=3i64 {
            csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
                .unwrap();
        }
        let removed = csr.compact_with_ts(3, 0.0);
        assert_eq!(removed, 0);
        assert_eq!(csr.total_edge_capacity, 3);
        assert_eq!(csr.edges_of(0u32, 3).len(), 3);
    }

    #[test]
    fn test_overflow_iterator() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }

        let all_edges: Vec<_> = csr.iter(1).collect();
        assert_eq!(all_edges.len(), 6);
    }

    #[test]
    fn test_supernode_overflow_uses_fixed_chunks_without_recopying() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(1, 4, 32);
        for i in 0..4_096u64 {
            csr.insert_edge(0, VertexId::from_int64(i as i64 + 1), EdgeId(i + 1), 1)
                .unwrap();
        }

        let chunks = csr.overflow_chunks.get(&0).expect("vertex 0 has overflow");
        assert!(chunks.iter().all(|chunk| chunk.capacity() == 32));
        assert!(chunks.iter().all(|chunk| chunk.len() <= 32));
        assert_eq!(csr.edges_of(0, 1).len(), 4_096);
    }

    #[test]
    fn test_zero_degree_rows_hold_no_slots() {
        let mut csr = MutableCsr::with_capacity(1024, 4096);
        assert_eq!(csr.total_edge_capacity, 0);

        // A single edge allocates exactly one primary block
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        assert_eq!(csr.total_edge_capacity, 4);

        // Sparse high vertex ids allocate blocks only for themselves
        csr.insert_edge(10_000u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        assert_eq!(csr.vertex_capacity(), 12_502);
        assert_eq!(csr.total_edge_capacity, 8);

        // Growth is proportional (1.25x), not power-of-two doubling
        assert_eq!(csr.vertex_capacity(), (10_001.0_f64 * 1.25).ceil() as usize);

        // Compact reclaims slots of rows whose edges were all removed
        csr.delete_edge(0u32, EdgeId(100), 2).unwrap();
        csr.compact_with_ts(3, 0.0);
        assert_eq!(csr.total_edge_capacity, 1);
        assert_eq!(csr.primary_capacities[0], 0);
    }

    #[test]
    fn test_fragmentation_ratio() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        // No edges - ratio should be 0.0
        assert_eq!(csr.fragmentation_ratio(), 0.0);

        // Insert edges to trigger overflow
        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }

        // After overflow, ratio should be > 1.0
        let ratio = csr.fragmentation_ratio();
        assert!(ratio > 1.0, "Expected ratio > 1.0, got {}", ratio);
    }

    #[test]
    fn test_wasted_bytes_estimate() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }

        let wasted = csr.wasted_bytes_estimate();
        let active = csr.edge_count() as usize;
        let total_capacity = csr.total_edge_capacity;

        // Wasted should be roughly (total - active) * sizeof(Nbr)
        let expected_wasted = (total_capacity - active) * std::mem::size_of::<Nbr>();
        assert_eq!(wasted, expected_wasted, "Wasted bytes estimate mismatch");
    }

    #[test]
    fn test_compact_reduces_fragmentation() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
        }

        let ratio_before = csr.fragmentation_ratio();
        assert!(
            ratio_before > 1.5,
            "Setup failed: insufficient fragmentation"
        );

        csr.compact_with_ts(1, 0.25);

        let ratio_after = csr.fragmentation_ratio();
        assert!(
            ratio_after <= ratio_before * 0.9,
            "Compact did not reduce fragmentation: before={}, after={}",
            ratio_before,
            ratio_after
        );
    }

    #[test]
    fn test_vertex_edges_iter_no_allocation() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        // Insert multiple edges for vertex 0
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(4), EdgeId(103), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(5), EdgeId(104), 1)
            .unwrap();

        // Test iter_edges_of yields same neighbors as edges_of without allocation
        let iter_neighbors: Vec<_> = csr
            .iter_edges_of(0u32, 1)
            .map(|nbr| nbr.to_vertex_id())
            .collect();
        let vec_neighbors: Vec<_> = csr
            .edges_of(0u32, 1)
            .iter()
            .map(|nbr| nbr.to_vertex_id())
            .collect();

        assert_eq!(iter_neighbors.len(), vec_neighbors.len());
        assert_eq!(iter_neighbors, vec_neighbors);
    }

    #[test]
    fn test_vertex_edges_iter_respects_timestamp() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 2)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 3)
            .unwrap();

        // Delete the second edge at ts=2
        csr.delete_edge(0u32, EdgeId(101), 2).unwrap();

        // At ts=1, only first edge should be visible
        let edges_ts1: Vec<_> = csr.iter_edges_of(0u32, 1).collect();
        assert_eq!(edges_ts1.len(), 1);
        assert_eq!(edges_ts1[0].edge_id, EdgeId(100));

        // At ts=2, first two edges are visible (but second is deleted)
        let edges_ts2: Vec<_> = csr.iter_edges_of(0u32, 2).collect();
        assert_eq!(edges_ts2.len(), 1);

        // At ts=3, all three are visible (but second is deleted)
        let edges_ts3: Vec<_> = csr.iter_edges_of(0u32, 3).collect();
        assert_eq!(edges_ts3.len(), 2);
    }

    #[test]
    fn test_overflow_storage_lookup() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        for vid in 0..5u32 {
            for i in 0..6 {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                    .unwrap();
            }
        }
        assert!(csr.get_overflow_chunks(0).is_some());
        assert!(csr.get_overflow_chunks(999).is_none());
    }

    #[test]
    fn test_overflow_get_chunks_transparent() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        for vid in 0..20u32 {
            for i in 0..6 {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                    .unwrap();
            }
        }
        // All chunks should still be accessible via get_overflow_chunks
        for vid in 0..20u32 {
            let chunks = csr.get_overflow_chunks(vid).expect("should have overflow");
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].len(), 2);
        }
        for vid in 0..20u32 {
            let edges = csr.edges_of(vid, 1);
            assert_eq!(edges.len(), 6);
        }
    }

    #[test]
    fn test_overflow_cleared_after_compact() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        for vid in 0..20u32 {
            for i in 0..8 {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                    .unwrap();
            }
        }
        assert!(!csr.overflow_chunks.is_empty());
        let mut removed = Vec::new();
        csr.compact_with_ts_reporting(2, 0.0, &mut |id, ts| removed.push((id, ts)));
        assert!(csr.overflow_chunks.is_empty());
    }

    #[test]
    fn test_compact_vertex_is_row_scoped() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        csr.insert_edge(5u32, VertexId::from_int64(6), EdgeId(102), 1)
            .unwrap();
        assert!(csr.delete_edge(0u32, EdgeId(100), 2).unwrap());

        assert_eq!(csr.reclaimable_count(0, 3), 1);
        assert_eq!(csr.reclaimable_count(5, 3), 0);
        assert!(csr.vertex_needs_compact(0, 3));
        assert!(!csr.vertex_needs_compact(5, 3));

        let mut reported = Vec::new();
        let removed =
            csr.compact_vertex_with_reporting(0, 3, &mut |id, ts| reported.push((id, ts)));
        assert_eq!(removed, 1);
        assert_eq!(reported, vec![(EdgeId(100), 2)]);

        // Target row reclaimed, other row untouched.
        assert_eq!(csr.reclaimable_count(0, 3), 0);
        assert_eq!(csr.edges_of(5, 3).len(), 1);
        assert_eq!(csr.edges_of(0, 3).len(), 1);
        let (live, dead, _) = csr.vertex_census(0);
        assert_eq!((live, dead), (1, 0));
    }

    #[test]
    fn test_compact_vertex_keeps_pinned_tombstones() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        assert!(csr.delete_edge(0u32, EdgeId(100), 10).unwrap());

        // Cutoff below the deletion stamp: nothing is eligible.
        assert_eq!(csr.reclaimable_count(0, 5), 0);
        assert!(!csr.vertex_needs_compact(0, 5));
        let removed = csr.compact_vertex_with_reporting(0, 5, &mut |_, _| {});
        assert_eq!(removed, 0);
        // The tombstone stays readable for older snapshots.
        assert_eq!(csr.edges_of(0, 9).len(), 1);
        assert_eq!(csr.edges_of(0, 10).len(), 0);
    }

    #[test]
    fn test_compact_vertex_repacks_overflow() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        for i in 0..6u64 {
            let dst = VertexId::from_int64(100 + i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i), 1).unwrap();
        }
        // 4 primary + 2 overflow.
        assert!(csr.get_overflow_chunks(0).is_some());
        assert!(csr.delete_edge(0u32, EdgeId(0), 2).unwrap());
        assert!(csr.delete_edge(0u32, EdgeId(5), 2).unwrap());

        let removed = csr.compact_vertex_with_reporting(0, 3, &mut |_, _| {});
        assert_eq!(removed, 2);
        assert_eq!(csr.edges_of(0, 3).len(), 4);
        assert_eq!(csr.reclaimable_count(0, 3), 0);
    }

    #[test]
    fn test_fragmentation_stats_report_dead_entries() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        for i in 0..3u64 {
            let dst = VertexId::from_int64(10 + i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i), 1).unwrap();
        }
        assert!(csr.delete_edge(0u32, EdgeId(0), 2).unwrap());

        let stats = csr.get_fragmentation_stats();
        assert_eq!(stats.reachable_edges, 2);
        assert_eq!(stats.dead_entries, 1);
        assert_eq!(
            stats.wasted_capacity,
            stats.total_capacity.saturating_sub(2)
        );
        let (live, dead, _) = csr.vertex_census(0);
        assert_eq!((live, dead), (2, 1));
    }

    #[test]
    fn test_remove_after_delete_does_not_double_count() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        assert!(csr.delete_edge(0u32, EdgeId(100), 2).unwrap());
        assert_eq!(csr.edge_count(), 1);
        assert!(csr.remove_edge(0u32, EdgeId(100)));
        assert_eq!(csr.edge_count(), 1);
        assert!(csr.remove_edge(0u32, EdgeId(101)));
        assert_eq!(csr.edge_count(), 0);
    }

    #[test]
    fn test_remove_after_delete_overflow_does_not_double_count() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        for i in 0..6u64 {
            csr.insert_edge(0u32, VertexId::from_int64(100 + i as i64), EdgeId(i), 1)
                .unwrap();
        }
        assert_eq!(csr.edge_count(), 6);
        assert!(csr.delete_edge(0u32, EdgeId(5), 2).unwrap());
        assert_eq!(csr.edge_count(), 5);
        assert!(csr.remove_edge(0u32, EdgeId(5)));
        assert_eq!(csr.edge_count(), 5);
    }

    #[test]
    fn test_offset_delete_rejects_out_of_degree() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        csr.insert_edge(1u32, VertexId::from_int64(2), EdgeId(101), 1)
            .unwrap();
        // Row 0 holds one live entry; offset 1 addresses reserved capacity.
        assert!(!csr.delete_edge_by_offset(0u32, 1, 2).unwrap());
        assert_eq!(csr.edges_of(0u32, 2).len(), 1);
        assert_eq!(csr.edges_of(1u32, 2).len(), 1);
        assert!(!csr.revert_delete_by_offset(0u32, 1, 2));
        // Valid offset still works.
        assert!(csr.delete_edge_by_offset(0u32, 0, 2).unwrap());
        assert_eq!(csr.edges_of(0u32, 2).len(), 0);
        assert!(csr.revert_delete_by_offset(0u32, 0, 2));
        assert_eq!(csr.edges_of(0u32, 2).len(), 1);
    }

    #[test]
    fn test_nbr_at_offset_views_primary_slot() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(0u32, VertexId::from_int64(7), EdgeId(100), 1)
            .unwrap();
        let slot = csr.nbr_at_offset(0u32, 0).expect("slot exists");
        assert_eq!(slot.edge_id, EdgeId(100));
        assert!(csr.nbr_at_offset(0u32, 1).is_none());
        assert!(csr.nbr_at_offset(0u32, -1).is_none());
    }

    #[test]
    fn test_physical_reads_ignore_timestamps() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 10)
            .unwrap();
        assert!(csr.delete_edge(0u32, EdgeId(100), 20).unwrap());
        assert!(csr
            .get_edge_physical(0u32, VertexId::from_int64(1))
            .is_some());
        assert_eq!(csr.physical_edges_of(0u32).len(), 1);
        assert!(csr.has_physical_entries(0u32));
        assert!(!csr.has_physical_entries(1u32));
    }
}

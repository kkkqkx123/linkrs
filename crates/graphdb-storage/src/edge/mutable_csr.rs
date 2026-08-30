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
//! sorted-vector map keyed by vertex id. This keeps the per-row fixed cost to 12 bytes
//! (offset + degree + capacity) and eliminates HashMap fragmentation.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::persistence::{read_u32_le, read_u64_le};
use graphdb_core::{StorageError, StorageResult};

use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId};

pub mod iter;
pub mod overflow;
pub mod region;
pub mod serialization;

pub use iter::{MutableCsrIterator, VertexEdgesIter};
pub use overflow::{OverflowIndex, OverflowIndexStats, OverflowStorage, SequentialRun};
pub use region::MutableCsrRegion;
pub(crate) use serialization::{read_nbr, write_nbr};

use overflow::MAX_OVERFLOW_CHUNKS_PER_VERTEX;
use serialization::MUTABLE_CSR_FORMAT_VERSION;

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const DEFAULT_EDGE_CAPACITY: usize = 4096;
const DEFAULT_VERTEX_DEGREE: usize = 4;
const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;
const VERTEX_GROWTH_FACTOR: f64 = 1.25;

pub struct MutableCsr {
    nbr_list: Vec<Nbr>,
    adj_offsets: Vec<u32>,
    degrees: Vec<u32>,
    primary_capacities: Vec<u32>,

    overflow_chunks: OverflowStorage,
    overflow_chunk_edges: usize,
    overflow_index: OverflowIndex,
    /// Live endpoint set for overflow vertices: (endpoint, rank) of edges
    /// whose `delete_ts == MAX`. Enables O(1) duplicate detection for
    /// high-degree vertices instead of scanning all overflow blocks.
    overflow_live_sets: HashMap<u32, HashSet<(u32, i64)>>,

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
            overflow_index: self.overflow_index.clone(),
            overflow_live_sets: self.overflow_live_sets.clone(),
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
            overflow_index: OverflowIndex::new(),
            overflow_live_sets: HashMap::new(),
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

    // ── Overflow Index (Sequential CSR Index) ──

    /// Rebuild overflow index, detecting sequential runs of vertices with uniform chunk counts.
    pub fn rebuild_overflow_index(&mut self) {
        self.overflow_index = OverflowIndex::rebuild_from_storage(&self.overflow_chunks);
    }

    /// Get overflow chunks for a vertex, transparent to sequential index.
    pub fn get_overflow_chunks(&self, vid: u32) -> Option<&Vec<Vec<Nbr>>> {
        self.overflow_chunks.get(&vid)
    }

    /// Access the overflow index metadata.
    pub fn overflow_index(&self) -> &OverflowIndex {
        &self.overflow_index
    }

    /// Check if a vertex belongs to a sequential run.
    pub fn is_overflow_sequential(&self, vid: u32) -> bool {
        self.overflow_index.is_sequential(vid)
    }

    /// Overflow index statistics.
    pub fn overflow_index_stats(&self) -> OverflowIndexStats {
        let total = self.overflow_chunks.len();
        let sequential_runs = self.overflow_index.len();
        let sequential_vertices: usize = self
            .overflow_index
            .sequential_runs()
            .iter()
            .map(|r| r.vertex_count as usize)
            .sum();
        let sparse_vertices = total.saturating_sub(sequential_vertices);
        let saved = self.overflow_index.metadata_bytes_saved(total);
        OverflowIndexStats {
            total_overflow_vertices: total,
            sequential_runs,
            sequential_vertices,
            sparse_vertices,
            metadata_bytes_saved: saved,
        }
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
            // We need to remove from sorted vector; find index and remove.
            if let Ok(idx) = self
                .overflow_chunks
                .entries
                .binary_search_by_key(&vid, |(k, _)| *k)
            {
                self.overflow_chunks.entries.remove(idx);
            }
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

    /// Compute per-region statistics for incremental freeze decisions.
    ///
    /// Each region covers `region_vertex_count` consecutive vertices. `edge_count`
    /// counts only edges visible at `visible_ts` (create_ts <= visible_ts if Some,
    /// otherwise all physical entries). `capacity` is the allocated slots in the
    /// region (primary + overflow), `density = edge_count / capacity` (0 if empty).
    pub fn regions_with_ts(
        &self,
        region_vertex_count: usize,
        visible_ts: Option<Timestamp>,
    ) -> Vec<MutableCsrRegion> {
        if region_vertex_count == 0 {
            return Vec::new();
        }
        let vc = self.vertex_capacity();
        if vc == 0 {
            return Vec::new();
        }
        let region_cnt = vc.div_ceil(region_vertex_count);
        let mut out = Vec::with_capacity(region_cnt);
        for rid in 0..region_cnt {
            let start = (rid * region_vertex_count) as u32;
            let end = ((rid + 1) * region_vertex_count).min(vc) as u32;
            let mut edge_count = 0u32;
            let mut deleted_count = 0u32;
            let mut capacity = 0u32;
            for vid in start..end {
                let idx = vid as usize;
                capacity += self.primary_capacities[idx];
                let degree = self.degrees[idx] as usize;
                let base = self.adj_offsets[idx] as usize;
                for i in 0..degree {
                    let nbr = &self.nbr_list[base + i];
                    let visible = match visible_ts {
                        Some(ts) => nbr.create_ts <= ts,
                        None => true,
                    };
                    if visible {
                        edge_count += 1;
                        if nbr.delete_ts != Timestamp::MAX {
                            deleted_count += 1;
                        }
                    }
                }
                if let Some(chunks) = self.overflow_chunks.get(&vid) {
                    for chunk in chunks {
                        capacity += chunk.capacity() as u32;
                        for nbr in chunk {
                            let visible = match visible_ts {
                                Some(ts) => nbr.create_ts <= ts,
                                None => true,
                            };
                            if visible {
                                edge_count += 1;
                                if nbr.delete_ts != Timestamp::MAX {
                                    deleted_count += 1;
                                }
                            }
                        }
                    }
                    // Each chunk already counted capacity, but we added per chunk capacity above; primary
                    // overflow_chunks capacity counted correctly. Avoid double count of total_edge_capacity's
                    // per-chunk allocation which is already included via chunk.capacity().
                }
            }
            // Normalize capacity: if zero (no primary allocated) use vertex count * DEFAULT degree as logical capacity
            let logical_capacity = if capacity == 0 {
                (end - start) * DEFAULT_VERTEX_DEGREE as u32
            } else {
                capacity
            };
            let density = if logical_capacity == 0 {
                0.0
            } else {
                edge_count as f32 / logical_capacity as f32
            };
            out.push(MutableCsrRegion {
                region_id: rid as u32,
                vertex_start: start,
                vertex_end: end,
                edge_count,
                deleted_count,
                capacity: logical_capacity,
                density,
            });
        }
        out
    }

    pub fn regions(&self, region_vertex_count: usize) -> Vec<MutableCsrRegion> {
        self.regions_with_ts(region_vertex_count, None)
    }

    /// Raw insert without duplicate checks, preserving delete_ts.
    fn insert_raw_nbr(&mut self, src_vid: u32, nbr: Nbr) {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }
        if self.primary_capacities[src_idx] == 0 {
            self.allocate_primary_block(src_idx);
        }
        let degree = self.degrees[src_idx] as usize;
        if self.overflow_chunks.get(&src_vid).is_none_or(Vec::is_empty)
            && degree < self.primary_capacities[src_idx] as usize
        {
            let base = self.adj_offsets[src_idx] as usize;
            self.nbr_list[base + degree] = nbr;
            self.degrees[src_idx] += 1;
            if nbr.delete_ts == Timestamp::MAX {
                self.edge_count.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        // overflow path
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
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            self.track_overflow_live_insert(src_vid, nbr.endpoint, nbr.rank);
        }
    }

    /// Drain entries belonging to the given region ids that are visible at `ts`,
    /// and rebuild the delta to retain the remaining entries (including those
    /// not yet visible at `ts`). Returns the drained entries for freezing.
    /// The remaining delta is compacted in-place.
    pub fn drain_regions(
        &mut self,
        region_ids: &std::collections::HashSet<u32>,
        region_vertex_count: usize,
        ts: Timestamp,
    ) -> Vec<(u32, Nbr, Timestamp)> {
        if region_ids.is_empty() || region_vertex_count == 0 {
            return Vec::new();
        }
        let mut frozen = Vec::new();
        let mut retained: Vec<(u32, Nbr, Timestamp)> = Vec::new();

        for (src_vid, nbr) in self.iter_all() {
            let src_u32 = src_vid.as_int64().unwrap_or(0) as u32;
            let create_ts = nbr.create_ts;
            let rid = (src_u32 as usize / region_vertex_count) as u32;
            let visible = create_ts <= ts;
            if visible && region_ids.contains(&rid) {
                frozen.push((src_u32, nbr, create_ts));
            } else {
                retained.push((src_u32, nbr, create_ts));
            }
        }

        if frozen.is_empty() {
            return frozen;
        }

        self.clear();
        // Rebuild retained entries preserving delete_ts and counts
        for (src_u32, nbr, _create_ts) in retained {
            self.insert_raw_nbr(src_u32, nbr);
        }
        frozen
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
        prop_offset: u32,
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

        // Duplicate check across both primary and overflow
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
        let nbr_with_ts =
            Nbr::with_create_ts_and_prop(decoded_endpoint, decoded_rank, edge_id, ts, prop_offset);

        // Write to primary if space available and overflow not yet allocated
        if self.overflow_chunks.get(&src_vid).is_none_or(Vec::is_empty)
            && degree < self.primary_capacities[src_idx] as usize
        {
            self.nbr_list[base + degree] = nbr_with_ts;
            self.degrees[src_idx] += 1;
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
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
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

    /// Delete edge by destination vertex
    pub fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> bool {
        let (decoded_vid, decoded_rank) = dst.decode_edge_endpoint();
        let decoded_endpoint = decoded_vid.as_u64().unwrap_or(0) as u32;
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        let mut deleted = false;

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
                    deleted = true;
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
                        deleted = true;
                    }
                }
            }
        }
        for (ep, rk) in overflow_deleted_endpoints {
            self.track_overflow_live_remove(src_vid, ep, rk);
        }

        deleted
    }

    pub fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if offset < 0 {
            return false;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.primary_capacities[src_idx] == 0 {
            return false;
        }
        let idx = self.adj_offsets[src_idx] as usize + offset as usize;
        if idx >= self.nbr_list.len() {
            return false;
        }
        let nbr = &mut self.nbr_list[idx];
        if nbr.delete_ts == Timestamp::MAX {
            let create_ts = nbr.create_ts;
            if create_ts <= ts {
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
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

        let base_offset = self.adj_offsets[src_idx] as usize;
        let idx = base_offset + offset as usize;

        if idx >= self.nbr_list.len() {
            return false;
        }

        let nbr = &mut self.nbr_list[idx];
        // Only revert deletions that happened at or before rollback time.
        // Prevents rolling back deletions that occur after the rollback point.
        if nbr.delete_ts < Timestamp::MAX && nbr.delete_ts <= ts {
            nbr.delete_ts = Timestamp::MAX;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
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
                // Shift left to close the gap, then decrement the degree.
                for j in i..degree - 1 {
                    self.nbr_list[offset + j] = self.nbr_list[offset + j + 1];
                }
                self.degrees[src_idx] -= 1;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
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
                        // Remove empty overflow entry from sorted vector.
                        if let Ok(idx) = self
                            .overflow_chunks
                            .entries
                            .binary_search_by_key(&src_vid, |(k, _)| *k)
                        {
                            self.overflow_chunks.entries.remove(idx);
                        }
                        self.overflow_live_sets.remove(&src_vid);
                        self.edge_count.fetch_sub(1, Ordering::Relaxed);
                        return true;
                    }
                }
                if was_live {
                    self.track_overflow_live_remove(src_vid, endpoint, rank);
                }
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
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
                nbr.delete_ts = Timestamp::MAX;
                self.edge_count.fetch_add(1, Ordering::Relaxed);
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
        for degree in &mut self.degrees {
            *degree = 0;
        }
        self.overflow_chunks.clear();
        self.overflow_index.clear();
        self.overflow_live_sets.clear();
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
        self.overflow_index = OverflowIndex::rebuild_from_storage(&self.overflow_chunks);
        self.overflow_chunk_edges = overflow_chunk_edges;
        self.nbr_list = nbr_list;
        self.edge_count.store(edge_count, Ordering::Relaxed);
        self.rebuild_overflow_live_sets();

        Ok(())
    }

    /// Compact CSR by removing deleted edges and reclaiming space.
    /// Merges overflow back into primary, restoring flat CSR layout.
    ///
    /// Only entries whose deletion predates the active-snapshot cutoff are
    /// physically removed (`delete_ts < cutoff` with `cutoff <
    /// Timestamp::MAX`). With no active snapshot (`cutoff == MAX`) every
    /// deleted entry is kept so time-travel queries before the deletion stay
    /// possible; without that protection the deletion history would be lost.
    /// The `reserve_ratio` parameter reserves space for future edges.
    pub fn compact_with_ts(&mut self, cutoff: Timestamp, reserve_ratio: f32) -> usize {
        self.compact_with_ts_reporting(cutoff, reserve_ratio, &mut |_, _| {})
    }

    /// Compact with per-edge removal reporting.
    ///
    /// Same semantics as `compact_with_ts`; `on_edge_removed` is invoked for
    /// every entry physically dropped, with its edge id and delete timestamp,
    /// so the caller can promote the deletion into the global tombstone layer.
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
                if nbr.delete_ts != Timestamp::MAX && removals_enabled && nbr.delete_ts < cutoff {
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
                            && nbr.delete_ts < cutoff
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
        self.overflow_index.clear();
        self.overflow_live_sets.clear();

        removed_count
    }

    /// Region-aware compact: only triggers a full rebuild when at least one
    /// region contains reclaimable deletions (`delete_ts < cutoff`). Clean
    /// regions are still rebuilt together (single flat CSR) but the method
    /// avoids work entirely when no region is dirty.
    pub fn compact_regions_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
        region_vertex_count: usize,
    ) -> usize {
        self.compact_regions_with_ts_reporting_calibrated(
            cutoff,
            reserve_ratio,
            on_edge_removed,
            region_vertex_count,
            None,
        )
    }

    /// Region-aware compact with calibrated deletion threshold.
    ///
    /// When `calibrated_deletion_ratio` is Some, a region is considered dirty
    /// only when its deletion ratio meets the calibrated threshold; otherwise
    /// any reclaimable deletion makes the region dirty (legacy behavior).
    pub fn compact_regions_with_ts_reporting_calibrated(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
        region_vertex_count: usize,
        calibrated_deletion_ratio: Option<f64>,
    ) -> usize {
        if region_vertex_count == 0 {
            return self.compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed);
        }
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let vc = self.vertex_capacity();
        if vc == 0 {
            return 0;
        }
        let region_cnt = vc.div_ceil(region_vertex_count);
        let mut dirty_regions = 0usize;
        for rid in 0..region_cnt {
            let start_v = rid * region_vertex_count;
            let end_v = ((rid + 1) * region_vertex_count).min(vc);
            let mut dirty = false;
            if let Some(threshold) = calibrated_deletion_ratio {
                let mut total_in_region = 0usize;
                let mut deleted_in_region = 0usize;
                for vid in start_v..end_v {
                    let degree = self.degrees[vid] as usize;
                    let off = self.adj_offsets[vid] as usize;
                    for i in 0..degree {
                        total_in_region += 1;
                        let nbr = &self.nbr_list[off + i];
                        if nbr.delete_ts != Timestamp::MAX && nbr.delete_ts < cutoff {
                            deleted_in_region += 1;
                        }
                    }
                    if let Some(chunks) = self.overflow_chunks.get(&(vid as u32)) {
                        for chunk in chunks {
                            for nbr in chunk {
                                total_in_region += 1;
                                if nbr.delete_ts != Timestamp::MAX && nbr.delete_ts < cutoff {
                                    deleted_in_region += 1;
                                }
                            }
                        }
                    }
                }
                if total_in_region > 0 {
                    let ratio = deleted_in_region as f64 / total_in_region as f64;
                    if ratio >= threshold {
                        dirty = true;
                    }
                }
            } else {
                let mut has_reclaimable = false;
                for vid in start_v..end_v {
                    let degree = self.degrees[vid] as usize;
                    let off = self.adj_offsets[vid] as usize;
                    for i in 0..degree {
                        let nbr = &self.nbr_list[off + i];
                        if nbr.delete_ts != Timestamp::MAX && nbr.delete_ts < cutoff {
                            has_reclaimable = true;
                            break;
                        }
                    }
                    if has_reclaimable {
                        break;
                    }
                    if let Some(chunks) = self.overflow_chunks.get(&(vid as u32)) {
                        for chunk in chunks {
                            for nbr in chunk {
                                if nbr.delete_ts != Timestamp::MAX && nbr.delete_ts < cutoff {
                                    has_reclaimable = true;
                                    break;
                                }
                            }
                            if has_reclaimable {
                                break;
                            }
                        }
                    }
                    if has_reclaimable {
                        break;
                    }
                }
                dirty = has_reclaimable;
            }
            if dirty {
                dirty_regions += 1;
            }
        }
        if dirty_regions == 0 {
            log::debug!(
                "MutableCsr region-aware compact skipped: no dirty region (regions={}, cutoff={})",
                region_cnt,
                cutoff
            );
            return 0;
        }
        log::debug!(
            "MutableCsr region-aware compact: {}/{} regions dirty, rebuilding",
            dirty_regions,
            region_cnt
        );
        self.compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed)
    }

    /// Get used memory size (active edges only)
    pub fn used_memory_size(&self) -> usize {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        active_edges * std::mem::size_of::<Nbr>() + std::mem::size_of::<Self>()
    }

    /// Look up the creation timestamp for an edge.
    pub fn create_ts_of(&self, edge_id: EdgeId) -> Option<Timestamp> {
        self.nbr_list
            .iter()
            .find(|nbr| nbr.edge_id == edge_id)
            .map(|nbr| nbr.create_ts)
    }

    /// Compute fragmentation ratio: nbr_list.len() / active_edges
    ///
    /// A ratio > 1.5 indicates moderate fragmentation; > 2.0 suggests compaction.
    /// Returns 0.0 if no active edges.
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

    /// Get detailed fragmentation statistics
    pub fn get_fragmentation_stats(&self) -> super::FragmentationStats {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;

        let zombie_blocks = 0;
        let mut total_wasted = 0;

        for vid in 0..self.vertex_capacity() {
            let primary_cap = self.primary_capacities[vid] as usize;
            let primary_degree = self.degrees[vid] as usize;
            total_wasted += primary_cap.saturating_sub(primary_degree);
            if let Some(chunks) = self.overflow_chunks.get(&(vid as u32)) {
                total_wasted += chunks
                    .iter()
                    .map(|chunk| chunk.capacity().saturating_sub(chunk.len()))
                    .sum::<usize>();
            }
        }

        super::FragmentationStats::with_zombie_info(
            self.total_edge_capacity,
            active_edges,
            zombie_blocks,
            total_wasted,
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
        prop_offset: u32,
    ) -> StorageResult<()> {
        MutableCsr::insert_edge(self, src_vid, dst, edge_id, ts, prop_offset)
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        MutableCsr::delete_edge(self, src_vid, edge_id, ts)
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> bool {
        MutableCsr::delete_edge_by_dst(self, src_vid, dst, ts)
    }

    fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        MutableCsr::delete_edge_by_offset(self, src_vid, offset, ts)
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        MutableCsr::revert_delete_by_offset(self, src_vid, offset, ts)
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

    fn used_memory_size(&self) -> usize {
        MutableCsr::used_memory_size(self)
    }

    fn create_ts_of(&self, edge_id: EdgeId) -> Option<Timestamp> {
        MutableCsr::create_ts_of(self, edge_id)
    }

    fn rebuild_create_ts(&mut self, iter: impl Iterator<Item = (EdgeId, Timestamp)>) {
        let mut map: std::collections::HashMap<EdgeId, Timestamp> = iter.collect();
        for nbr in self.nbr_list.iter_mut() {
            if let Some(ts) = map.remove(&nbr.edge_id) {
                nbr.create_ts = ts;
            }
        }
        for (_, chunks) in self.overflow_chunks.iter_mut() {
            for chunk in chunks.iter_mut() {
                for nbr in chunk.iter_mut() {
                    if let Some(ts) = map.remove(&nbr.edge_id) {
                        nbr.create_ts = ts;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_insert_and_query() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            1u32,
            VertexId::from_int64(3),
            EdgeId(102),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        assert!(csr
            .insert_edge(
                0u32,
                VertexId::from_int64(1),
                EdgeId(103),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE
            )
            .is_err());

        assert_eq!(csr.edge_count(), 3);
    }

    #[test]
    fn test_delete_edge() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        assert!(csr.delete_edge(0u32, EdgeId(100), 2).unwrap());

        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_double_delete_conflict() {
        let mut csr = MutableCsr::with_capacity(10, 100);
        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            10,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
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

        csr1.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr1.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr1.insert_edge(
            1u32,
            VertexId::from_int64(3),
            EdgeId(102),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
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

        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            100u32,
            VertexId::from_int64(1),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        assert!(csr.vertex_capacity() >= 101);
    }

    #[test]
    fn test_iterator() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            1u32,
            VertexId::from_int64(3),
            EdgeId(102),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        let edges: Vec<_> = csr.iter(1).collect();
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_overflow_insert() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(3),
            EdgeId(102),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(4),
            EdgeId(103),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(5),
            EdgeId(104),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        assert_eq!(csr.edge_count(), 5);

        let edges = csr.edges_of(0u32, 1);
        assert_eq!(edges.len(), 5);

        assert!(csr
            .insert_edge(
                0u32,
                VertexId::from_int64(5),
                EdgeId(105),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE
            )
            .is_err());

        assert!(csr.delete_edge(0u32, EdgeId(104), 2).unwrap());
    }

    #[test]
    fn test_overflow_dump_and_load() {
        let mut csr1 = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr1.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
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
            csr.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
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
            csr.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
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
            csr.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
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
            csr.insert_edge(
                0u32,
                VertexId::from_int64(i),
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
        }
        csr.insert_edge(
            1u32,
            VertexId::from_int64(1),
            EdgeId(7),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
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
            csr.insert_edge(
                0u32,
                VertexId::from_int64(i),
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
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
            csr.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
        }

        let all_edges: Vec<_> = csr.iter(1).collect();
        assert_eq!(all_edges.len(), 6);
    }

    #[test]
    fn test_supernode_overflow_uses_fixed_chunks_without_recopying() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(1, 4, 32);
        for i in 0..4_096u64 {
            csr.insert_edge(
                0,
                VertexId::from_int64(i as i64 + 1),
                EdgeId(i + 1),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
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
        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        assert_eq!(csr.total_edge_capacity, 4);

        // Sparse high vertex ids allocate blocks only for themselves
        csr.insert_edge(
            10_000u32,
            VertexId::from_int64(2),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
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
            csr.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
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
            csr.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
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
            csr.insert_edge(
                0u32,
                dst,
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
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
        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(101),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(3),
            EdgeId(102),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(4),
            EdgeId(103),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(5),
            EdgeId(104),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
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

        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(101),
            2,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr.insert_edge(
            0u32,
            VertexId::from_int64(3),
            EdgeId(102),
            3,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
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
    fn test_overflow_index_sequential_run_detection() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        // Create 20 vertices (0..20) each with exactly 2 overflow chunk allocations
        // Primary capacity is 4, so after 4 edges primary full, next edges go to overflow.
        // With chunk size 2, inserting 8 edges per vertex -> 4 primary + 4 overflow (2 chunks)
        for vid in 0..20u32 {
            for i in 0..8 {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(
                    vid,
                    dst,
                    EdgeId(vid as u64 * 10 + i as u64),
                    1,
                    crate::edge::property_schema::PROP_OFFSET_NONE,
                )
                .unwrap();
            }
        }
        csr.rebuild_overflow_index();
        let stats = csr.overflow_index_stats();
        assert_eq!(stats.sequential_runs, 1);
        assert_eq!(stats.sequential_vertices, 20);
        assert_eq!(stats.sparse_vertices, 0);
        // Verify sequential check
        assert!(csr.is_overflow_sequential(5));
        assert!(csr.is_overflow_sequential(19));
        let runs = csr.overflow_index().sequential_runs();
        assert_eq!(runs[0].start_vid, 0);
        assert_eq!(runs[0].vertex_count, 20);
        assert_eq!(runs[0].chunk_count, 2);
    }

    #[test]
    fn test_overflow_index_sparse_fallback() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        // 5 vertices with same pattern (< threshold) -> no sequential run
        for vid in 0..5u32 {
            for i in 0..6 {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(
                    vid,
                    dst,
                    EdgeId(vid as u64 * 10 + i as u64),
                    1,
                    crate::edge::property_schema::PROP_OFFSET_NONE,
                )
                .unwrap();
            }
        }
        // Add vertices with different chunk counts (non-uniform)
        for vid in 10..15u32 {
            let chunk_cnt = if vid % 2 == 0 { 1 } else { 2 };
            let edges_needed = 4 + chunk_cnt * 2;
            for i in 0..edges_needed {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(
                    vid,
                    dst,
                    EdgeId(1000 + vid as u64 * 10 + i as u64),
                    1,
                    crate::edge::property_schema::PROP_OFFSET_NONE,
                )
                .unwrap();
            }
        }
        csr.rebuild_overflow_index();
        let stats = csr.overflow_index_stats();
        // 5 vertices <16 threshold -> no run, mixed chunk counts -> no run
        assert_eq!(stats.sequential_runs, 0);
        assert!(!csr.is_overflow_sequential(0));
        // Sparse lookup still works
        assert!(csr.get_overflow_chunks(0).is_some());
        assert!(csr.get_overflow_chunks(10).is_some());
        assert!(csr.get_overflow_chunks(999).is_none());
    }

    #[test]
    fn test_overflow_index_get_chunks_transparent() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        for vid in 0..20u32 {
            for i in 0..6 {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(
                    vid,
                    dst,
                    EdgeId(vid as u64 * 10 + i as u64),
                    1,
                    crate::edge::property_schema::PROP_OFFSET_NONE,
                )
                .unwrap();
            }
        }
        csr.rebuild_overflow_index();
        // All chunks should still be accessible via get_overflow_chunks
        for vid in 0..20u32 {
            let chunks = csr.get_overflow_chunks(vid).expect("should have overflow");
            assert_eq!(chunks.len(), 1); // 2 overflow edges -> 1 chunk of size 2
            assert_eq!(chunks[0].len(), 2);
        }
        // Verify edges_of still works for sequential vertices
        for vid in 0..20u32 {
            let edges = csr.edges_of(vid, 1);
            assert_eq!(edges.len(), 6);
        }
    }

    #[test]
    fn test_overflow_index_rebuild_after_compact() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
        for vid in 0..20u32 {
            for i in 0..8 {
                let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
                csr.insert_edge(
                    vid,
                    dst,
                    EdgeId(vid as u64 * 10 + i as u64),
                    1,
                    crate::edge::property_schema::PROP_OFFSET_NONE,
                )
                .unwrap();
            }
        }
        csr.rebuild_overflow_index();
        assert_eq!(csr.overflow_index_stats().sequential_runs, 1);
        // Compact merges overflow back into primary, should clear index
        let mut removed = Vec::new();
        csr.compact_with_ts_reporting(2, 0.0, &mut |id, ts| removed.push((id, ts)));
        assert!(csr.overflow_index().is_empty());
        assert_eq!(csr.overflow_index_stats().total_overflow_vertices, 0);
    }

    #[test]
    fn test_mutable_csr_region_stats() {
        let mut csr = MutableCsr::with_capacity(4096, 4096);
        // Region 0: 10 edges dense, Region 1: 1 edge sparse, Region 2: empty
        for i in 0..10 {
            csr.insert_edge(
                0,
                VertexId::from_int64(i + 1),
                EdgeId(i as u64),
                1,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
        }
        csr.insert_edge(
            2048,
            VertexId::from_int64(100),
            EdgeId(100),
            1,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        let regions = csr.regions_with_ts(1024, Some(1));
        assert_eq!(regions.len(), 4);
        // Region 0 should have high edge count
        assert_eq!(regions[0].vertex_start, 0);
        assert_eq!(regions[0].edge_count, 10);
        // Region 2 should be sparse (1 edge in 1024 vertices)
        assert_eq!(regions[2].edge_count, 1);
        // Density is computed from capacity; just ensure non-zero
        assert!(regions[0].density >= 0.0);
        assert!(regions[2].density >= 0.0);
        // Region 1 empty
        assert_eq!(regions[1].edge_count, 0);
        assert_eq!(regions[3].edge_count, 0);
    }

    #[test]
    fn test_drain_regions_retains_low_density() {
        let mut csr = MutableCsr::with_capacity(4096, 4096);
        // Fill region 0 dense (20 edges across vertex 0), region 1 sparse (1 edge)
        for i in 0..20 {
            csr.insert_edge(
                0,
                VertexId::from_int64(i as i64 + 1),
                EdgeId(i as u64),
                10,
                crate::edge::property_schema::PROP_OFFSET_NONE,
            )
            .unwrap();
        }
        csr.insert_edge(
            2048,
            VertexId::from_int64(999),
            EdgeId(1000),
            10,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        assert_eq!(csr.edge_count(), 21);
        let mut selected = std::collections::HashSet::new();
        selected.insert(0); // freeze only region 0
        let frozen = csr.drain_regions(&selected, 1024, 10);
        assert_eq!(frozen.len(), 20);
        assert_eq!(csr.edge_count(), 1);
        // Remaining edge should be the sparse one in region 2 (vertex 2048)
        let remaining = csr.edges_of(2048, 10);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].edge_id, EdgeId(1000));
        // Drained region 0 should be empty
        assert!(csr.edges_of(0, 10).is_empty());
    }
}

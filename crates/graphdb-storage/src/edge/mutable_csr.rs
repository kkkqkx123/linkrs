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

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::persistence::{read_u32_le, read_u64_le};
use graphdb_core::{StorageError, StorageResult};

use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId};

fn write_nbr(out: &mut Vec<u8>, nbr: &Nbr) {
    out.extend_from_slice(&nbr.endpoint.to_le_bytes());
    out.extend_from_slice(&nbr.rank.to_le_bytes());
    out.extend_from_slice(&nbr.edge_id.to_le_bytes());
    out.extend_from_slice(&nbr.delete_ts.to_le_bytes());
    out.extend_from_slice(&nbr.prop_offset.to_le_bytes());
}

fn read_nbr(data: &[u8], offset: &mut usize) -> StorageResult<Nbr> {
    let endpoint = read_u32_le(data, offset)?;
    let rank = read_u64_le(data, offset)? as i64;
    let raw_edge_id = read_u64_le(data, offset)?;
    let delete_ts = read_u64_le(data, offset)?;
    let prop_offset = read_u32_le(data, offset)?;
    Ok(Nbr::with_timestamps_and_prop(
        endpoint,
        rank,
        EdgeId(raw_edge_id),
        delete_ts,
        prop_offset,
    ))
}

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const DEFAULT_EDGE_CAPACITY: usize = 4096;
const DEFAULT_VERTEX_DEGREE: usize = 4;
const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;
const MUTABLE_CSR_FORMAT_VERSION: u32 = 2;
const VERTEX_GROWTH_FACTOR: f64 = 1.25;

/// Minimum vertices in a sequential run to be considered for indexed compaction.
const SEQUENTIAL_RUN_THRESHOLD: usize = 16;

/// Sequential overflow run: contiguous vertex range with uniform chunk count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequentialRun {
    pub start_vid: u32,
    pub vertex_count: u32,
    pub chunk_count: usize,
}

/// Optimized overflow chunk index for sequential vertex ranges.
#[derive(Debug, Clone, Default)]
pub struct OverflowIndex {
    sequential_runs: Vec<SequentialRun>,
}

impl OverflowIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sequential_runs(&self) -> &[SequentialRun] {
        &self.sequential_runs
    }

    pub fn len(&self) -> usize {
        self.sequential_runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sequential_runs.is_empty()
    }

    pub fn clear(&mut self) {
        self.sequential_runs.clear();
    }

    /// Check if a vertex belongs to any sequential run.
    pub fn is_sequential(&self, vid: u32) -> bool {
        self.sequential_runs
            .iter()
            .any(|run| vid >= run.start_vid && vid < run.start_vid + run.vertex_count)
    }

    /// Find the sequential run containing `vid`, if any.
    pub fn find_run(&self, vid: u32) -> Option<&SequentialRun> {
        self.sequential_runs
            .iter()
            .find(|run| vid >= run.start_vid && vid < run.start_vid + run.vertex_count)
    }

    /// Build index from overflow_chunks map.
    pub fn rebuild_from_chunks(chunks: &HashMap<u32, Vec<Vec<Nbr>>>) -> Self {
        if chunks.is_empty() {
            return Self::default();
        }
        let mut vertices: Vec<(u32, usize)> =
            chunks.iter().map(|(vid, v)| (*vid, v.len())).collect();
        vertices.sort_by_key(|(vid, _)| *vid);

        let mut runs = Vec::new();
        let mut i = 0usize;
        while i < vertices.len() {
            let (vid, chunk_count) = vertices[i];
            let mut run_length = 1usize;
            while i + run_length < vertices.len() {
                let (next_vid, next_count) = vertices[i + run_length];
                if next_vid != vid + run_length as u32 || next_count != chunk_count {
                    break;
                }
                run_length += 1;
            }
            if run_length >= SEQUENTIAL_RUN_THRESHOLD {
                runs.push(SequentialRun {
                    start_vid: vid,
                    vertex_count: run_length as u32,
                    chunk_count,
                });
                i += run_length;
            } else {
                i += 1;
            }
        }
        Self {
            sequential_runs: runs,
        }
    }

    /// Build index from overflow storage (sorted-vector backend).
    pub fn rebuild_from_storage(storage: &OverflowStorage) -> Self {
        if storage.is_empty() {
            return Self::default();
        }
        let mut vertices: Vec<(u32, usize)> =
            storage.iter().map(|(vid, v)| (*vid, v.len())).collect();
        // storage is already sorted, but keep sort for safety if insertion ever unsorted
        vertices.sort_by_key(|(vid, _)| *vid);

        let mut runs = Vec::new();
        let mut i = 0usize;
        while i < vertices.len() {
            let (vid, chunk_count) = vertices[i];
            let mut run_length = 1usize;
            while i + run_length < vertices.len() {
                let (next_vid, next_count) = vertices[i + run_length];
                if next_vid != vid + run_length as u32 || next_count != chunk_count {
                    break;
                }
                run_length += 1;
            }
            if run_length >= SEQUENTIAL_RUN_THRESHOLD {
                runs.push(SequentialRun {
                    start_vid: vid,
                    vertex_count: run_length as u32,
                    chunk_count,
                });
                i += run_length;
            } else {
                i += 1;
            }
        }
        Self {
            sequential_runs: runs,
        }
    }

    /// Memory savings estimate for overflow metadata (in bytes).
    pub fn metadata_bytes_saved(&self, total_overflow_vertices: usize) -> usize {
        if self.sequential_runs.is_empty() {
            return 0;
        }
        let sequential_vertices: usize = self
            .sequential_runs
            .iter()
            .map(|r| r.vertex_count as usize)
            .sum();
        let sparse_vertices = total_overflow_vertices.saturating_sub(sequential_vertices);
        let before = total_overflow_vertices * (4 + 24);
        let after = sparse_vertices * (4 + 24) + self.sequential_runs.len() * 12;
        before.saturating_sub(after)
    }
}

/// Statistics for overflow index.
#[derive(Debug, Clone, Copy)]
pub struct OverflowIndexStats {
    pub total_overflow_vertices: usize,
    pub sequential_runs: usize,
    pub sequential_vertices: usize,
    pub sparse_vertices: usize,
    pub metadata_bytes_saved: usize,
}

/// Sorted-vector based overflow storage replacing `HashMap<u32, Vec<Vec<Nbr>>>`.
///
/// Stores overflow chunks sparsely but without HashMap fragmentation:
/// - Single contiguous `Vec` of `(vid, chunks)` sorted by `vid`
/// - Binary-search lookup O(log n) with better cache locality than hash probing
/// - No bucket array, no per-entry hash overhead, predictable memory layout
///
/// For the typical sparse-overflow case (few high-degree vertices) the
/// sorted-vector is both more memory-efficient and more cache-friendly.
/// Insertions are O(n) due to shifting, but overflow vertices are few
/// and insertions are amortized by the large `overflow_chunk_edges` (4096).
#[derive(Debug, Clone, Default)]
pub struct OverflowStorage {
    entries: Vec<(u32, Vec<Vec<Nbr>>)>,
}

impl OverflowStorage {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    #[inline]
    fn find_index(&self, vid: &u32) -> Result<usize, usize> {
        self.entries.binary_search_by_key(vid, |(k, _)| *k)
    }

    #[inline]
    pub fn get(&self, vid: &u32) -> Option<&Vec<Vec<Nbr>>> {
        match self.find_index(vid) {
            Ok(idx) => Some(&self.entries[idx].1),
            Err(_) => None,
        }
    }

    #[inline]
    pub fn get_mut(&mut self, vid: &u32) -> Option<&mut Vec<Vec<Nbr>>> {
        match self.find_index(vid) {
            Ok(idx) => Some(&mut self.entries[idx].1),
            Err(_) => None,
        }
    }

    /// Get mutable reference to the chunk list for `vid`, inserting an empty
    /// entry if absent while keeping the vector sorted.
    #[inline]
    pub fn get_or_create(&mut self, vid: u32) -> &mut Vec<Vec<Nbr>> {
        match self.find_index(&vid) {
            Ok(idx) => &mut self.entries[idx].1,
            Err(idx) => {
                self.entries.insert(idx, (vid, Vec::new()));
                &mut self.entries[idx].1
            }
        }
    }

    #[inline]
    pub fn insert(&mut self, vid: u32, chunks: Vec<Vec<Nbr>>) {
        match self.find_index(&vid) {
            Ok(idx) => self.entries[idx].1 = chunks,
            Err(idx) => self.entries.insert(idx, (vid, chunks)),
        }
    }

    #[inline]
    pub fn contains_key(&self, vid: &u32) -> bool {
        self.find_index(vid).is_ok()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, (u32, Vec<Vec<Nbr>>)> {
        self.entries.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, (u32, Vec<Vec<Nbr>>)> {
        self.entries.iter_mut()
    }

    /// Total number of Nbr entries across all overflow chunks.
    pub fn total_nbr_count(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, chunks)| chunks.iter().map(Vec::len).sum::<usize>())
            .sum()
    }

    /// Estimate of wasted capacity inside overflow chunks (capacity - len).
    pub fn wasted_capacity(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, chunks)| {
                chunks
                    .iter()
                    .map(|c| c.capacity().saturating_sub(c.len()))
                    .sum::<usize>()
            })
            .sum()
    }

    /// Check whether merging overflow back into primary would be beneficial.
    ///
    /// Returns true when the overflow storage is highly fragmented
    /// (wasted capacity ratio > 0.5) or when a single vertex has grown
    /// beyond the chunk threshold, indicating that a global compact would
    /// reclaim significant space.
    pub fn should_merge(&self, fragmentation_threshold: f32) -> bool {
        let total: usize = self.total_nbr_count();
        if total == 0 {
            return false;
        }
        let wasted = self.wasted_capacity();
        let ratio = wasted as f32 / (total + wasted) as f32;
        ratio > fragmentation_threshold
    }
}

/// Mutable CSR graph structure with two-level storage.
///
/// # Layout
///
/// Each vertex has:
/// - **Primary block**: contiguous slot in `nbr_list` (size = `primary_capacities[src_idx]`),
///   starting at `adj_offsets[src_idx]`. Active edges: `degrees[src_idx]`.
///   Blocks are allocated lazily on the first edge, so zero-degree vertices
///   occupy no data slots in `nbr_list`.
/// - **Overflow block**: contiguous region in `nbr_list` for edges beyond primary capacity,
///   stored as append-only blocks at the end of `nbr_list`.
///
/// When primary fills (`degrees == primary_capacities`), new edges go to overflow.
/// Overflow blocks are allocated via `expand_vertex_capacity()` which appends to `nbr_list`,
/// avoiding O(n) splice on the main array. Overflow chunk lists live in a sparse
/// sorted-vector (`OverflowStorage`) so rows without overflow carry no per-row cost
/// and HashMap fragmentation is eliminated.
///
/// `compact()` merges overflow back into primary, restoring flat CSR layout.
pub struct MutableCsr {
    nbr_list: Vec<Nbr>,
    adj_offsets: Vec<u32>,
    degrees: Vec<u32>,
    primary_capacities: Vec<u32>,

    overflow_chunks: OverflowStorage,
    overflow_chunk_edges: usize,
    overflow_index: OverflowIndex,

    create_ts_cache: HashMap<EdgeId, Timestamp>,

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
            create_ts_cache: self.create_ts_cache.clone(),
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
            create_ts_cache: HashMap::new(),
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
            .sequential_runs
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
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
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

        // Record create_ts in cache before writing the Nbr
        self.create_ts_cache.insert(edge_id, ts);

        // Write to primary if space available and overflow not yet allocated
        if self.overflow_chunks.get(&src_vid).is_none_or(Vec::is_empty)
            && degree < self.primary_capacities[src_idx] as usize
        {
            self.nbr_list[base + degree] =
                Nbr::with_prop_offset(decoded_endpoint, decoded_rank, edge_id, prop_offset);
            self.degrees[src_idx] += 1;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        self.append_overflow(
            src_vid,
            Nbr::with_prop_offset(decoded_endpoint, decoded_rank, edge_id, prop_offset),
        );
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
                let create_ts = self.create_ts_cache.get(&edge_id).copied().unwrap_or(0);
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
                let create_ts = self.create_ts_cache.get(&edge_id).copied().unwrap_or(0);
                if create_ts <= ts {
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
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
                let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
                if create_ts <= ts {
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                    deleted = true;
                }
            }
        }

        // Scan overflow
        let indices = self.scan_overflow_for_dst(src_vid, dst);
        if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
            for (chunk_idx, edge_idx) in indices {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                if nbr.delete_ts == Timestamp::MAX {
                    let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
                    if create_ts <= ts {
                        nbr.delete_ts = ts;
                        self.edge_count.fetch_sub(1, Ordering::Relaxed);
                        deleted = true;
                    }
                }
            }
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
            let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
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
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                chunks[chunk_idx].remove(edge_idx);
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
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                if nbr.delete_ts != Timestamp::MAX && nbr.delete_ts <= ts {
                    nbr.delete_ts = Timestamp::MAX;
                    self.edge_count.fetch_add(1, Ordering::Relaxed);
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
            let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
            if nbr.is_alive_at(ts, create_ts) {
                result.push(*nbr);
            }
        }

        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
                    if nbr.is_alive_at(ts, create_ts) {
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
            let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
            if nbr.is_alive_at(ts, create_ts) {
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
            .filter(|nbr| {
                let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
                nbr.is_alive_at(ts, create_ts)
            })
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
            let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
            if nbr.endpoint == decoded_endpoint
                && nbr.rank == decoded_rank
                && nbr.is_alive_at(ts, create_ts)
            {
                return Some(*nbr);
            }
        }

        // Scan overflow
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    let create_ts = self.create_ts_cache.get(&nbr.edge_id).copied().unwrap_or(0);
                    if nbr.endpoint == decoded_endpoint
                        && nbr.rank == decoded_rank
                        && nbr.is_alive_at(ts, create_ts)
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
        active_edges * std::mem::size_of::<Nbr>()
            + self.create_ts_cache.len()
                * (std::mem::size_of::<EdgeId>() + std::mem::size_of::<Timestamp>())
            + std::mem::size_of::<Self>()
    }

    /// Look up the creation timestamp for an edge.
    pub fn create_ts_of(&self, edge_id: EdgeId) -> Option<Timestamp> {
        self.create_ts_cache.get(&edge_id).copied()
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

    /// Get detailed fragmentation statistics (legacy compat)
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

/// Iterator over edges of a single vertex in MutableCsr.
///
/// Yields references to valid neighbors at a specific timestamp,
/// without allocating intermediate storage.
pub struct VertexEdgesIter<'a> {
    csr: &'a MutableCsr,
    ts: Timestamp,
    primary_idx: usize,
    primary_end: usize,
    overflow_chunks: Option<&'a Vec<Vec<Nbr>>>,
    overflow_chunk_idx: usize,
    overflow_edge_idx: usize,
}

impl<'a> VertexEdgesIter<'a> {
    /// Create iterator for all edges of a vertex at the given timestamp
    pub fn new(csr: &'a MutableCsr, src_vid: u32, ts: Timestamp) -> Self {
        let src_idx = src_vid as usize;
        if src_idx >= csr.vertex_capacity() {
            return Self {
                csr,
                ts,
                primary_idx: 0,
                primary_end: 0,
                overflow_chunks: None,
                overflow_chunk_idx: 0,
                overflow_edge_idx: 0,
            };
        }

        let degree = csr.degrees[src_idx] as usize;
        let offset = csr.adj_offsets[src_idx] as usize;
        Self {
            csr,
            ts,
            primary_idx: offset,
            primary_end: offset + degree,
            overflow_chunks: csr.overflow_chunks.get(&src_vid),
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
        }
    }
}

impl<'a> Iterator for VertexEdgesIter<'a> {
    type Item = &'a Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        // Scan primary block
        while self.primary_idx < self.primary_end {
            let nbr = &self.csr.nbr_list[self.primary_idx];
            self.primary_idx += 1;
            let create_ts = self
                .csr
                .create_ts_cache
                .get(&nbr.edge_id)
                .copied()
                .unwrap_or(0);
            if nbr.is_alive_at(self.ts, create_ts) {
                return Some(nbr);
            }
        }

        if let Some(chunks) = self.overflow_chunks {
            while self.overflow_chunk_idx < chunks.len() {
                let chunk = &chunks[self.overflow_chunk_idx];
                while self.overflow_edge_idx < chunk.len() {
                    let nbr = &chunk[self.overflow_edge_idx];
                    self.overflow_edge_idx += 1;
                    let create_ts = self
                        .csr
                        .create_ts_cache
                        .get(&nbr.edge_id)
                        .copied()
                        .unwrap_or(0);
                    if nbr.is_alive_at(self.ts, create_ts) {
                        return Some(nbr);
                    }
                }
                self.overflow_chunk_idx += 1;
                self.overflow_edge_idx = 0;
            }
        }

        None
    }
}

pub struct MutableCsrIterator<'a> {
    csr: &'a MutableCsr,
    ts: Timestamp,
    include_deleted: bool,
    current_vertex: usize,
    current_edge: usize,
    in_overflow: bool,
    overflow_chunks: Option<&'a Vec<Vec<Nbr>>>,
    overflow_chunk_idx: usize,
    overflow_edge_idx: usize,
}

impl<'a> MutableCsrIterator<'a> {
    pub fn new(csr: &'a MutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            include_deleted: false,
            current_vertex: 0,
            current_edge: 0,
            in_overflow: false,
            overflow_chunks: None,
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
        }
    }

    /// Iterator over every stored entry, including tombstoned ones.
    pub fn new_all(csr: &'a MutableCsr) -> Self {
        Self {
            csr,
            ts: 0,
            include_deleted: true,
            current_vertex: 0,
            current_edge: 0,
            in_overflow: false,
            overflow_chunks: None,
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
        }
    }
}

impl<'a> Iterator for MutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity() {
            let degree = self.csr.degrees[self.current_vertex] as usize;
            let offset = self.csr.adj_offsets[self.current_vertex] as usize;

            if !self.in_overflow {
                // Fresh vertex: (re)load its overflow chunk list
                if self.current_edge == 0 {
                    self.overflow_chunks =
                        self.csr.overflow_chunks.get(&(self.current_vertex as u32));
                    self.overflow_chunk_idx = 0;
                    self.overflow_edge_idx = 0;
                }
                // Scan primary
                while self.current_edge < degree {
                    let nbr = self.csr.nbr_list[offset + self.current_edge];
                    self.current_edge += 1;
                    let create_ts = self
                        .csr
                        .create_ts_cache
                        .get(&nbr.edge_id)
                        .copied()
                        .unwrap_or(0);
                    if self.include_deleted || nbr.is_alive_at(self.ts, create_ts) {
                        return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                    }
                }
                // Move to overflow phase
                self.in_overflow = true;
            }

            // Scan overflow
            if let Some(chunks) = self.overflow_chunks {
                while self.overflow_chunk_idx < chunks.len() {
                    let chunk = &chunks[self.overflow_chunk_idx];
                    while self.overflow_edge_idx < chunk.len() {
                        let nbr = chunk[self.overflow_edge_idx];
                        self.overflow_edge_idx += 1;
                        let create_ts = self
                            .csr
                            .create_ts_cache
                            .get(&nbr.edge_id)
                            .copied()
                            .unwrap_or(0);
                        if self.include_deleted || nbr.is_alive_at(self.ts, create_ts) {
                            return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                        }
                    }
                    self.overflow_chunk_idx += 1;
                    self.overflow_edge_idx = 0;
                }
            }

            // Move to next vertex
            self.current_vertex += 1;
            self.current_edge = 0;
            self.in_overflow = false;
            self.overflow_chunk_idx = 0;
            self.overflow_edge_idx = 0;
        }
        None
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

    fn rebuild_create_ts_cache(&mut self, iter: impl Iterator<Item = (EdgeId, Timestamp)>) {
        self.create_ts_cache.clear();
        self.create_ts_cache.extend(iter);
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
}

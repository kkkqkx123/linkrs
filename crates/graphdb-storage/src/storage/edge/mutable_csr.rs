//! Mutable CSR Implementation
//!
//! Two-level CSR with fixed-size overflow chunks for stable append cost.
//! Primary blocks are stored contiguously in `nbr_list` (flat CSR layout).
//! Each overflow allocation adds one chunk and never copies an existing chunk. This keeps
//! high-degree vertex growth linear and avoids the repeated doubling/copying behavior that
//! previously produced unreachable blocks in the primary neighbor array.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::{StorageError, StorageResult};
use crate::storage::persistence::{read_u32_le, read_u64_le};

use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId, INVALID_TIMESTAMP};

fn write_vertex_id(out: &mut Vec<u8>, id: VertexId) {
    let bytes = id.as_bytes();
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn read_vertex_id(data: &[u8], offset: &mut usize) -> StorageResult<VertexId> {
    if *offset >= data.len() {
        return Err(StorageError::deserialize_error(
            "CSR data too short for vertex id length",
        ));
    }

    let len = data[*offset] as usize;
    *offset += 1;
    if data.len().saturating_sub(*offset) < len {
        return Err(StorageError::deserialize_error(
            "CSR data too short for vertex id bytes",
        ));
    }

    let id = VertexId::from_bytes(data[*offset..*offset + len].to_vec());
    *offset += len;
    Ok(id)
}

fn write_nbr(out: &mut Vec<u8>, nbr: &Nbr) {
    write_vertex_id(out, nbr.neighbor);
    out.extend_from_slice(&nbr.edge_id.to_le_bytes());
    out.extend_from_slice(&nbr.prop_offset.to_le_bytes());
    out.extend_from_slice(&nbr.create_ts.to_le_bytes());
    out.extend_from_slice(&nbr.delete_ts.to_le_bytes());
}

fn read_nbr(data: &[u8], offset: &mut usize) -> StorageResult<Nbr> {
    let neighbor = read_vertex_id(data, offset)?;
    let raw_edge_id = read_u64_le(data, offset)?;
    let prop_offset = read_u32_le(data, offset)?;
    let create_ts = read_u64_le(data, offset)?;
    let delete_ts = read_u64_le(data, offset)?;
    Ok(Nbr::with_delete_ts(
        neighbor,
        EdgeId(raw_edge_id),
        prop_offset,
        create_ts,
        delete_ts,
    ))
}

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const DEFAULT_EDGE_CAPACITY: usize = 4096;
const DEFAULT_VERTEX_DEGREE: usize = 4;
const DEFAULT_OVERFLOW_CHUNK_EDGES: usize = 4096;
const MUTABLE_CSR_FORMAT_VERSION: u32 = 2;

/// Mutable CSR graph structure with two-level storage.
///
/// # Layout
///
/// Each vertex has:
/// - **Primary block**: contiguous slot in `nbr_list` (size = `primary_capacities[src_idx]`),
///   starting at `adj_offsets[src_idx]`. Active edges: `degrees[src_idx]`.
/// - **Overflow block**: contiguous region in `nbr_list` for edges beyond primary capacity,
///   stored as append-only blocks at the end of `nbr_list`.
///
/// When primary fills (`degrees == primary_capacities`), new edges go to overflow.
/// Overflow blocks are allocated via `expand_vertex_capacity()` which appends to `nbr_list`,
/// avoiding O(n) splice on the main array.
///
/// `compact()` merges overflow back into primary, restoring flat CSR layout.
pub struct MutableCsr {
    nbr_list: Vec<Nbr>,
    adj_offsets: Vec<u32>,
    degrees: Vec<u32>,
    primary_capacities: Vec<u32>,

    overflow_chunks: Vec<Vec<Vec<Nbr>>>,
    overflow_chunk_edges: usize,

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
        let edge_cap = edge_capacity.max(vertex_cap * DEFAULT_VERTEX_DEGREE);

        let initial_primary = DEFAULT_VERTEX_DEGREE;

        let mut nbr_list = Vec::with_capacity(edge_cap);
        let mut adj_offsets = Vec::with_capacity(vertex_cap);
        let mut primary_capacities = Vec::with_capacity(vertex_cap);

        let mut offset = 0usize;
        for _ in 0..vertex_cap {
            adj_offsets.push(offset as u32);
            primary_capacities.push(initial_primary as u32);
            offset += initial_primary;
        }

        nbr_list.resize(
            offset,
            Nbr::new(VertexId::from_int64(0), EdgeId(0), 0, INVALID_TIMESTAMP),
        );

        Self {
            nbr_list,
            adj_offsets,
            degrees: vec![0; vertex_cap],
            primary_capacities,
            overflow_chunks: vec![Vec::new(); vertex_cap],
            overflow_chunk_edges: overflow_chunk_edges.max(1),
            edge_count: AtomicU64::new(0),
            total_edge_capacity: offset,
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

        let old_capacity = self.vertex_capacity();
        let additional = new_vertex_capacity - old_capacity;

        let current_primary = if self.vertex_capacity() > 0 {
            self.primary_capacities[0] as usize
        } else {
            DEFAULT_VERTEX_DEGREE
        };

        let mut new_total_capacity = self.total_edge_capacity;
        for _ in 0..additional {
            self.adj_offsets.push(new_total_capacity as u32);
            self.primary_capacities.push(current_primary as u32);
            self.degrees.push(0);
            self.overflow_chunks.push(Vec::new());
            new_total_capacity += current_primary;
        }

        self.nbr_list.resize(
            new_total_capacity,
            Nbr::new(VertexId::from_int64(0), EdgeId(0), 0, INVALID_TIMESTAMP),
        );
        self.total_edge_capacity = new_total_capacity;
    }

    /// Ensure vertex capacity (grows if needed)
    pub fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity() {
            let new_capacity = min_capacity.next_power_of_two();
            self.resize(new_capacity);
        }
    }

    fn append_overflow(&mut self, src_idx: usize, nbr: Nbr) {
        let needs_chunk = self.overflow_chunks[src_idx]
            .last()
            .is_none_or(|chunk| chunk.len() >= self.overflow_chunk_edges);
        if needs_chunk {
            self.overflow_chunks[src_idx].push(Vec::with_capacity(self.overflow_chunk_edges));
            self.total_edge_capacity = self
                .total_edge_capacity
                .saturating_add(self.overflow_chunk_edges);
        }
        self.overflow_chunks[src_idx]
            .last_mut()
            .expect("overflow chunk was just allocated")
            .push(nbr);
    }

    /// Insert an edge with automatic capacity expansion
    pub fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        ts: Timestamp,
    ) -> bool {
        let src_idx = src_vid as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        // Duplicate check across both primary and overflow
        let degree = self.degrees[src_idx] as usize;
        let base = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &self.nbr_list[base + i];
            if nbr.neighbor == dst && nbr.delete_ts == Timestamp::MAX {
                return false;
            }
        }
        for chunk in &self.overflow_chunks[src_idx] {
            for nbr in chunk {
                if nbr.neighbor == dst && nbr.delete_ts == Timestamp::MAX {
                    return false;
                }
            }
        }

        // Write to primary if space available and overflow not yet allocated
        if self.overflow_chunks[src_idx].is_empty()
            && degree < self.primary_capacities[src_idx] as usize
        {
            self.nbr_list[base + degree] = Nbr::new(dst, edge_id, prop_offset, ts);
            self.degrees[src_idx] += 1;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return true;
        }

        self.append_overflow(src_idx, Nbr::new(dst, edge_id, prop_offset, ts));
        self.edge_count.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn scan_overflow_for_edge_id(&self, src_idx: usize, edge_id: EdgeId) -> Option<(usize, usize)> {
        self.overflow_chunks[src_idx]
            .iter()
            .enumerate()
            .find_map(|(chunk_idx, chunk)| {
                chunk
                    .iter()
                    .position(|nbr| nbr.edge_id == edge_id)
                    .map(|edge_idx| (chunk_idx, edge_idx))
            })
    }

    fn scan_overflow_for_dst(&self, src_idx: usize, dst: VertexId) -> Vec<(usize, usize)> {
        let mut result = Vec::new();
        for (chunk_idx, chunk) in self.overflow_chunks[src_idx].iter().enumerate() {
            for (edge_idx, nbr) in chunk.iter().enumerate() {
                if nbr.neighbor == dst {
                    result.push((chunk_idx, edge_idx));
                }
            }
        }
        result
    }

    /// Delete an edge by edge_id
    pub fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.edge_id == edge_id && nbr.delete_ts == Timestamp::MAX && nbr.create_ts <= ts {
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_idx, edge_id) {
            let nbr = &mut self.overflow_chunks[src_idx][chunk_idx][edge_idx];
            if nbr.delete_ts == Timestamp::MAX && nbr.create_ts <= ts {
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }

        false
    }

    /// Delete edge by destination vertex
    pub fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> bool {
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
            if nbr.neighbor == dst && nbr.delete_ts == Timestamp::MAX && nbr.create_ts <= ts {
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                deleted = true;
            }
        }

        // Scan overflow
        let indices = self.scan_overflow_for_dst(src_idx, dst);
        for (chunk_idx, edge_idx) in indices {
            let nbr = &mut self.overflow_chunks[src_idx][chunk_idx][edge_idx];
            if nbr.delete_ts == Timestamp::MAX && nbr.create_ts <= ts {
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                deleted = true;
            }
        }

        deleted
    }

    pub fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if offset < 0 {
            return false;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let idx = self.adj_offsets[src_idx] as usize + offset as usize;
        if idx >= self.nbr_list.len() {
            return false;
        }
        let nbr = &mut self.nbr_list[idx];
        if nbr.delete_ts == Timestamp::MAX && nbr.create_ts <= ts {
            nbr.delete_ts = ts;
            self.edge_count.fetch_sub(1, Ordering::Relaxed);
            return true;
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
        if src_idx >= self.vertex_capacity() {
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

    /// Get edges of a vertex at a given timestamp
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }

        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;

        let total_valid_primary = self.count_valid_primary(src_idx, ts);
        let total_valid_overflow = self.count_valid_overflow(src_idx, ts);
        let mut result = Vec::with_capacity(total_valid_primary + total_valid_overflow);

        for i in 0..degree {
            let nbr = &self.nbr_list[offset + i];
            if nbr.is_valid_at(ts) {
                result.push(*nbr);
            }
        }

        for chunk in &self.overflow_chunks[src_idx] {
            for nbr in chunk {
                if nbr.is_valid_at(ts) {
                    result.push(*nbr);
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
            if nbr.is_valid_at(ts) {
                count += 1;
            }
        }
        count
    }

    fn count_valid_overflow(&self, src_idx: usize, ts: Timestamp) -> usize {
        self.overflow_chunks[src_idx]
            .iter()
            .flatten()
            .filter(|nbr| nbr.is_valid_at(ts))
            .count()
    }

    /// Get a specific edge
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &self.nbr_list[offset + i];
            if nbr.neighbor == dst && nbr.is_valid_at(ts) {
                return Some(*nbr);
            }
        }

        // Scan overflow
        for chunk in &self.overflow_chunks[src_idx] {
            for nbr in chunk {
                if nbr.neighbor == dst && nbr.is_valid_at(ts) {
                    return Some(*nbr);
                }
            }
        }

        None
    }

    /// Clear all edges
    pub fn clear(&mut self) {
        for nbr in &mut self.nbr_list {
            *nbr = Nbr::new(VertexId::from_int64(0), EdgeId(0), 0, INVALID_TIMESTAMP);
        }
        for degree in &mut self.degrees {
            *degree = 0;
        }
        for chunks in &mut self.overflow_chunks {
            chunks.clear();
        }
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

        for chunks in &self.overflow_chunks {
            result.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
            for chunk in chunks {
                result.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                for nbr in chunk {
                    write_nbr(&mut result, nbr);
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

        let mut overflow_chunks = Vec::with_capacity(vertex_capacity);
        let mut overflow_capacity = 0usize;
        for _ in 0..vertex_capacity {
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
            overflow_chunks.push(chunks);
        }

        self.total_edge_capacity = primary_edge_capacity.saturating_add(overflow_capacity);
        self.adj_offsets = adj_offsets;
        self.degrees = degrees;
        self.primary_capacities = primary_capacities;
        self.overflow_chunks = overflow_chunks;
        self.overflow_chunk_edges = overflow_chunk_edges;
        self.nbr_list = nbr_list;
        self.edge_count.store(edge_count, Ordering::Relaxed);

        Ok(())
    }

    /// Compact CSR by removing deleted edges and reclaiming space.
    /// Merges overflow back into primary, restoring flat CSR layout.
    ///
    /// Removes all edges marked as deleted (delete_ts != Timestamp::MAX).
    /// The ts parameter reserves space for future edges.
    pub fn compact_with_ts(&mut self, _ts: Timestamp, reserve_ratio: f32) -> usize {
        // Phase 1: compact individual vertex data (primary + overflow)
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
                if nbr.delete_ts == Timestamp::MAX {
                    new_edges.push(*nbr);
                } else {
                    removed_count += 1;
                }
            }

            // Collect active edges from overflow
            for chunk in &self.overflow_chunks[vid] {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
                        new_edges.push(*nbr);
                    } else {
                        removed_count += 1;
                    }
                }
            }

            let valid = new_edges.len() - new_offsets[vid];
            new_degrees.push(valid as u32);
            let new_cap = ((valid as f32 / (1.0 - reserve_ratio)).ceil() as u32).max(1);
            new_capacities.push(new_cap);
        }

        // Phase 2: rebuild nbr_list as flat CSR (no overflow)
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
                new_nbr_list.resize(
                    new_nbr_list.len() + remaining,
                    Nbr::new(VertexId::from_int64(0), EdgeId(0), 0, INVALID_TIMESTAMP),
                );
            }
        }

        self.nbr_list = new_nbr_list;
        self.adj_offsets = final_offsets;
        self.degrees = new_degrees;
        self.primary_capacities = new_capacities;
        self.total_edge_capacity = new_total_edge_capacity;

        self.overflow_chunks = vec![Vec::new(); self.vertex_capacity()];

        removed_count
    }

    /// Get used memory size (active edges only)
    pub fn used_memory_size(&self) -> usize {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        active_edges * std::mem::size_of::<Nbr>() + std::mem::size_of::<Self>()
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
            total_wasted += self.overflow_chunks[vid]
                .iter()
                .map(|chunk| chunk.capacity().saturating_sub(chunk.len()))
                .sum::<usize>();
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
    overflow_chunk_idx: usize,
    overflow_edge_idx: usize,
    src_idx: usize,
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
                overflow_chunk_idx: 0,
                overflow_edge_idx: 0,
                src_idx,
            };
        }

        let degree = csr.degrees[src_idx] as usize;
        let offset = csr.adj_offsets[src_idx] as usize;
        Self {
            csr,
            ts,
            primary_idx: offset,
            primary_end: offset + degree,
            overflow_chunk_idx: 0,
            overflow_edge_idx: 0,
            src_idx,
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
            if nbr.is_valid_at(self.ts) {
                return Some(nbr);
            }
        }

        while self.overflow_chunk_idx < self.csr.overflow_chunks[self.src_idx].len() {
            let chunk = &self.csr.overflow_chunks[self.src_idx][self.overflow_chunk_idx];
            while self.overflow_edge_idx < chunk.len() {
                let nbr = &chunk[self.overflow_edge_idx];
                self.overflow_edge_idx += 1;
                if nbr.is_valid_at(self.ts) {
                    return Some(nbr);
                }
            }
            self.overflow_chunk_idx += 1;
            self.overflow_edge_idx = 0;
        }

        None
    }
}

pub struct MutableCsrIterator<'a> {
    csr: &'a MutableCsr,
    ts: Timestamp,
    current_vertex: usize,
    current_edge: usize,
    in_overflow: bool,
    overflow_chunk_idx: usize,
    overflow_edge_idx: usize,
}

impl<'a> MutableCsrIterator<'a> {
    pub fn new(csr: &'a MutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            current_vertex: 0,
            current_edge: 0,
            in_overflow: false,
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
                // Scan primary
                while self.current_edge < degree {
                    let nbr = self.csr.nbr_list[offset + self.current_edge];
                    self.current_edge += 1;
                    if nbr.is_valid_at(self.ts) {
                        return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                    }
                }
                // Move to overflow phase
                self.in_overflow = true;
                self.overflow_chunk_idx = 0;
                self.overflow_edge_idx = 0;
            }

            // Scan overflow
            while self.overflow_chunk_idx < self.csr.overflow_chunks[self.current_vertex].len() {
                let chunk = &self.csr.overflow_chunks[self.current_vertex][self.overflow_chunk_idx];
                while self.overflow_edge_idx < chunk.len() {
                    let nbr = chunk[self.overflow_edge_idx];
                    self.overflow_edge_idx += 1;
                    if nbr.is_valid_at(self.ts) {
                        return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                    }
                }
                self.overflow_chunk_idx += 1;
                self.overflow_edge_idx = 0;
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
        prop_offset: u32,
        ts: Timestamp,
    ) -> bool {
        MutableCsr::insert_edge(self, src_vid, dst, edge_id, prop_offset, ts)
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_insert_and_query() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        assert!(csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1));
        assert!(csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 0, 1));
        assert!(csr.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 0, 1));

        assert!(!csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(103), 0, 1));

        assert_eq!(csr.edge_count(), 3);
    }

    #[test]
    fn test_delete_edge() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1);
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 0, 1);

        assert!(csr.delete_edge(0u32, EdgeId(100), 2));

        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_dump_and_load() {
        let mut csr1 = MutableCsr::with_capacity(10, 100);

        csr1.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1);
        csr1.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 0, 1);
        csr1.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 0, 1);

        let data = csr1.dump();

        let mut csr2 = MutableCsr::new();
        let _ = csr2.load(&data);

        assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
        assert_eq!(csr2.edge_count(), csr1.edge_count());
    }

    #[test]
    fn test_resize() {
        let mut csr = MutableCsr::with_capacity(2, 10);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1);
        csr.insert_edge(100u32, VertexId::from_int64(1), EdgeId(101), 0, 1);

        assert!(csr.vertex_capacity() >= 101);
    }

    #[test]
    fn test_iterator() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1);
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 0, 1);
        csr.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 0, 1);

        let edges: Vec<_> = csr.iter(1).collect();
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_overflow_insert() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        assert!(csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1));
        assert!(csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 0, 1));
        assert!(csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 0, 1));
        assert!(csr.insert_edge(0u32, VertexId::from_int64(4), EdgeId(103), 0, 1));
        assert!(csr.insert_edge(0u32, VertexId::from_int64(5), EdgeId(104), 0, 1));

        assert_eq!(csr.edge_count(), 5);

        let edges = csr.edges_of(0u32, 1);
        assert_eq!(edges.len(), 5);

        assert!(!csr.insert_edge(0u32, VertexId::from_int64(5), EdgeId(105), 0, 1));

        assert!(csr.delete_edge(0u32, EdgeId(104), 2));
    }

    #[test]
    fn test_overflow_dump_and_load() {
        let mut csr1 = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr1.insert_edge(0u32, dst, EdgeId(i as u64), 0, 1);
        }

        let data = csr1.dump();

        let mut csr2 = MutableCsr::new();
        let _ = csr2.load(&data);

        assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
        assert_eq!(csr2.edge_count(), csr1.edge_count());
        assert_eq!(
            csr2.overflow_chunks[0].iter().map(Vec::len).sum::<usize>(),
            2
        );
    }

    #[test]
    fn test_compact_with_ts_merges_overflow() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 0, 1);
        }

        csr.delete_edge(0u32, EdgeId(3), 5);
        csr.delete_edge(0u32, EdgeId(5), 5);
        csr.delete_edge(0u32, EdgeId(6), 5);

        let removed = csr.compact_with_ts(3, 0.25);
        assert_eq!(removed, 3);

        assert!(csr.overflow_chunks[0].is_empty());

        let edges = csr.edges_of(0u32, 3);
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_overflow_iterator() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 0, 1);
        }

        let all_edges: Vec<_> = csr.iter(1).collect();
        assert_eq!(all_edges.len(), 6);
    }

    #[test]
    fn test_supernode_overflow_uses_fixed_chunks_without_recopying() {
        let mut csr = MutableCsr::with_overflow_chunk_edges(1, 4, 32);
        for i in 0..4_096u64 {
            assert!(csr.insert_edge(0, VertexId::from_int64(i as i64 + 1), EdgeId(i + 1), 0, 1,));
        }

        assert!(csr.overflow_chunks[0]
            .iter()
            .all(|chunk| chunk.capacity() == 32));
        assert!(csr.overflow_chunks[0].iter().all(|chunk| chunk.len() <= 32));
        assert_eq!(csr.edges_of(0, 1).len(), 4_096);
    }

    #[test]
    fn test_fragmentation_ratio() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        // No edges - ratio should be 0.0
        assert_eq!(csr.fragmentation_ratio(), 0.0);

        // Insert edges to trigger overflow
        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 0, 1);
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
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 0, 1);
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
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 0, 1);
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
        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1);
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 0, 1);
        csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 0, 1);
        csr.insert_edge(0u32, VertexId::from_int64(4), EdgeId(103), 0, 1);
        csr.insert_edge(0u32, VertexId::from_int64(5), EdgeId(104), 0, 1);

        // Test iter_edges_of yields same neighbors as edges_of without allocation
        let iter_neighbors: Vec<_> = csr.iter_edges_of(0u32, 1).map(|nbr| nbr.neighbor).collect();
        let vec_neighbors: Vec<_> = csr
            .edges_of(0u32, 1)
            .iter()
            .map(|nbr| nbr.neighbor)
            .collect();

        assert_eq!(iter_neighbors.len(), vec_neighbors.len());
        assert_eq!(iter_neighbors, vec_neighbors);
    }

    #[test]
    fn test_vertex_edges_iter_respects_timestamp() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 0, 1);
        csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 0, 2);
        csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 0, 3);

        // Delete the second edge at ts=2
        csr.delete_edge(0u32, EdgeId(101), 2);

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
}

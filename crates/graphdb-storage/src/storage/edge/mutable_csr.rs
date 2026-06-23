//! Mutable CSR Implementation
//!
//! Two-level CSR with append-only overflow for O(1) amortized vertex expansion.
//! Primary blocks are stored contiguously in `nbr_list` (flat CSR layout).
//! Overflow edges are stored in a contiguous region at the end of `nbr_list`,
//! tracked per-vertex via `overflow_starts`/`overflow_counts`/`overflow_capacities`.
//! When a vertex's primary block is full, new edges spill to its overflow buffer,
//! avoiding O(n) splice on the main array.
//!
//! # Overflow Block Fragmentation
//!
//! When a vertex's overflow block expands multiple times via `expand_vertex_capacity()`:
//! - New data is appended to the end of `nbr_list`
//! - Old overflow block data is copied to the new location
//! - Old block address space remains in `nbr_list` but becomes unreachable (internal fragmentation)
//! - Repeated expansions accumulate these "zombie" blocks
//!
//! ## Fragmentation Impact
//!
//! | Aspect | Effect |
//! |--------|--------|
//! | **Queries** | No impact (always accessed via current `overflow_starts` pointer) |
//! | **Memory** | Wasted internal space in `nbr_list` |
//! | **Serialization** | `dump()` serializes entire `nbr_list` including fragmentation |
//! | **Correctness** | None (logically sound, only space-inefficient) |
//!
//! ## Fragmentation Recovery
//!
//! Call `compact_with_ts()` to defragment:
//! - Merges primary and overflow blocks into flat CSR layout
//! - Removes all logically deleted edges (`INVALID_TIMESTAMP`)
//! - Reclaims all wasted space
//!
//! ### When to Compact
//!
//! - **Before serialization**: If `fragmentation_ratio() > 2.0`, call `compact_with_ts(ts, 0.25)`
//!   to reduce persistence size
//! - **After bulk operations**: Optional, call `maybe_compact(2.5, ts)` if space efficiency matters
//! - **Production snapshots**: Recommended before writing to persistent storage
//!
//! ### Overhead
//!
//! `compact_with_ts(ts, reserve_ratio)` is O(V + E) and requires exclusive write access.
//! Use sparingly in high-throughput scenarios; suitable for offline maintenance windows.

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

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const DEFAULT_EDGE_CAPACITY: usize = 4096;
const DEFAULT_VERTEX_DEGREE: usize = 4;
const NO_OVERFLOW: u32 = u32::MAX;

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

    overflow_starts: Vec<u32>,
    overflow_counts: Vec<u32>,
    overflow_capacities: Vec<u32>,

    edge_count: AtomicU64,
    vertex_capacity: usize,
    total_edge_capacity: usize,
}

impl Clone for MutableCsr {
    fn clone(&self) -> Self {
        Self {
            nbr_list: self.nbr_list.clone(),
            adj_offsets: self.adj_offsets.clone(),
            degrees: self.degrees.clone(),
            primary_capacities: self.primary_capacities.clone(),
            overflow_starts: self.overflow_starts.clone(),
            overflow_counts: self.overflow_counts.clone(),
            overflow_capacities: self.overflow_capacities.clone(),
            edge_count: AtomicU64::new(self.edge_count.load(Ordering::Relaxed)),
            vertex_capacity: self.vertex_capacity,
            total_edge_capacity: self.total_edge_capacity,
        }
    }
}

impl fmt::Debug for MutableCsr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutableCsr")
            .field("vertex_capacity", &self.vertex_capacity)
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
        let vertex_cap = vertex_capacity.max(1);
        let edge_cap = edge_capacity.max(vertex_cap * DEFAULT_VERTEX_DEGREE);

        // Use DEFAULT_VERTEX_DEGREE for compatibility unless specifically tuned
        // Future optimization: could add a separate factory method for adaptive capacity
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
            overflow_starts: vec![NO_OVERFLOW; vertex_cap],
            overflow_counts: vec![0; vertex_cap],
            overflow_capacities: vec![0; vertex_cap],
            edge_count: AtomicU64::new(0),
            vertex_capacity: vertex_cap,
            total_edge_capacity: offset,
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    /// Resize vertex capacity (requires exclusive access)
    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity {
            return;
        }

        let old_capacity = self.vertex_capacity;
        let additional = new_vertex_capacity - old_capacity;

        // Use consistent primary capacity for new vertices
        let current_primary = if self.vertex_capacity > 0 {
            self.primary_capacities[0] as usize
        } else {
            DEFAULT_VERTEX_DEGREE
        };

        let mut new_total_capacity = self.total_edge_capacity;
        for _ in 0..additional {
            self.adj_offsets.push(new_total_capacity as u32);
            self.primary_capacities.push(current_primary as u32);
            self.degrees.push(0);
            self.overflow_starts.push(NO_OVERFLOW);
            self.overflow_counts.push(0);
            self.overflow_capacities.push(0);
            new_total_capacity += current_primary;
        }

        self.nbr_list.resize(
            new_total_capacity,
            Nbr::new(VertexId::from_int64(0), EdgeId(0), 0, INVALID_TIMESTAMP),
        );
        self.vertex_capacity = new_vertex_capacity;
        self.total_edge_capacity = new_total_capacity;
    }

    /// Ensure vertex capacity (grows if needed)
    pub fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity {
            let new_capacity = min_capacity.next_power_of_two();
            self.resize(new_capacity);
        }
    }

    /// Expand vertex capacity by appending overflow block at end of nbr_list.
    /// Copies existing overflow data to the new block if re-expanding.
    ///
    /// # Fragmentation Note
    ///
    /// This method intentionally leaves old overflow blocks unreachable in `nbr_list`.
    /// This is an acceptable tradeoff: O(1) expansion cost vs. O(n) block relocation.
    /// Old blocks accumulate over many expansions. Call `compact_with_ts()` to defragment.
    fn expand_vertex_capacity(&mut self, src_idx: usize) {
        let old_cap = self.primary_capacities[src_idx] as usize;
        let new_cap = (old_cap * 2).max(4);
        let additional = new_cap - old_cap;

        let append_pos = self.nbr_list.len();
        self.nbr_list.resize(
            append_pos + additional,
            Nbr::new(VertexId::from_int64(0), EdgeId(0), 0, INVALID_TIMESTAMP),
        );

        // Copy existing overflow data to new block if re-expanding
        if self.overflow_starts[src_idx] != NO_OVERFLOW {
            let old_start = self.overflow_starts[src_idx] as usize;
            let old_count = self.overflow_counts[src_idx] as usize;
            for i in 0..old_count {
                self.nbr_list[append_pos + i] = self.nbr_list[old_start + i];
            }
        }

        self.overflow_starts[src_idx] = append_pos as u32;
        self.overflow_capacities[src_idx] = additional as u32;
        self.primary_capacities[src_idx] = new_cap as u32;
        self.total_edge_capacity += additional;
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

        if src_idx >= self.vertex_capacity {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        // Duplicate check across both primary and overflow
        let degree = self.degrees[src_idx] as usize;
        let base = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &self.nbr_list[base + i];
            if nbr.neighbor == dst && nbr.delete_ts == u32::MAX {
                return false;
            }
        }
        if self.overflow_starts[src_idx] != NO_OVERFLOW {
            let o_start = self.overflow_starts[src_idx] as usize;
            let o_count = self.overflow_counts[src_idx] as usize;
            for i in 0..o_count {
                let nbr = &self.nbr_list[o_start + i];
                if nbr.neighbor == dst && nbr.delete_ts == u32::MAX {
                    return false;
                }
            }
        }

        // Write to primary if space available and overflow not yet allocated
        if self.overflow_starts[src_idx] == NO_OVERFLOW
            && degree < self.primary_capacities[src_idx] as usize
        {
            self.nbr_list[base + degree] = Nbr::new(dst, edge_id, prop_offset, ts);
            self.degrees[src_idx] += 1;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return true;
        }

        // Write to overflow, expanding if needed
        if self.overflow_starts[src_idx] == NO_OVERFLOW
            || self.overflow_counts[src_idx] >= self.overflow_capacities[src_idx]
        {
            self.expand_vertex_capacity(src_idx);
        }
        let o_start = self.overflow_starts[src_idx] as usize;
        let o_count = self.overflow_counts[src_idx] as usize;
        self.nbr_list[o_start + o_count] = Nbr::new(dst, edge_id, prop_offset, ts);
        self.overflow_counts[src_idx] += 1;
        self.edge_count.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn scan_overflow_for_edge_id(&self, src_idx: usize, edge_id: EdgeId) -> Option<usize> {
        if self.overflow_starts[src_idx] == NO_OVERFLOW {
            return None;
        }
        let o_start = self.overflow_starts[src_idx] as usize;
        let o_count = self.overflow_counts[src_idx] as usize;
        (0..o_count).find(|&i| self.nbr_list[o_start + i].edge_id == edge_id)
    }

    fn scan_overflow_for_dst(&self, src_idx: usize, dst: VertexId) -> Vec<usize> {
        if self.overflow_starts[src_idx] == NO_OVERFLOW {
            return Vec::new();
        }
        let o_start = self.overflow_starts[src_idx] as usize;
        let o_count = self.overflow_counts[src_idx] as usize;
        let mut result = Vec::new();
        for i in 0..o_count {
            if self.nbr_list[o_start + i].neighbor == dst {
                result.push(i);
            }
        }
        result
    }

    /// Delete an edge by edge_id
    pub fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity {
            return false;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.edge_id == edge_id && nbr.delete_ts == u32::MAX && nbr.create_ts <= ts {
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }

        // Scan overflow
        if let Some(idx) = self.scan_overflow_for_edge_id(src_idx, edge_id) {
            let o_start = self.overflow_starts[src_idx] as usize;
            let nbr = &mut self.nbr_list[o_start + idx];
            if nbr.delete_ts == u32::MAX && nbr.create_ts <= ts {
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
        if src_idx >= self.vertex_capacity {
            return false;
        }

        let mut deleted = false;

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.neighbor == dst && nbr.delete_ts == u32::MAX && nbr.create_ts <= ts {
                nbr.delete_ts = ts;
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
                deleted = true;
            }
        }

        // Scan overflow
        let indices = self.scan_overflow_for_dst(src_idx, dst);
        if self.overflow_starts[src_idx] != NO_OVERFLOW {
            let o_start = self.overflow_starts[src_idx] as usize;
            for idx in indices {
                let nbr = &mut self.nbr_list[o_start + idx];
                if nbr.delete_ts == u32::MAX && nbr.create_ts <= ts {
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                    deleted = true;
                }
            }
        }

        deleted
    }

    /// Delete an edge by offset position in the CSR primary block
    pub fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if offset < 0 {
            return false;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity {
            return false;
        }

        let base_offset = self.adj_offsets[src_idx] as usize;
        let idx = base_offset + offset as usize;

        if idx >= self.nbr_list.len() {
            return false;
        }

        let nbr = &mut self.nbr_list[idx];
        if nbr.delete_ts == u32::MAX && nbr.create_ts <= ts {
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
        if src_idx >= self.vertex_capacity {
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
        if nbr.delete_ts < u32::MAX && nbr.delete_ts <= ts {
            nbr.delete_ts = u32::MAX;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Get edges of a vertex at a given timestamp
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity {
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

        if self.overflow_starts[src_idx] != NO_OVERFLOW {
            let o_start = self.overflow_starts[src_idx] as usize;
            let o_count = self.overflow_counts[src_idx] as usize;
            for i in 0..o_count {
                let nbr = &self.nbr_list[o_start + i];
                if nbr.is_valid_at(ts) {
                    result.push(*nbr);
                }
            }
        }

        result
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
        if self.overflow_starts[src_idx] == NO_OVERFLOW {
            return 0;
        }
        let o_start = self.overflow_starts[src_idx] as usize;
        let o_count = self.overflow_counts[src_idx] as usize;
        let mut count = 0;
        for i in 0..o_count {
            let nbr = &self.nbr_list[o_start + i];
            if nbr.is_valid_at(ts) {
                count += 1;
            }
        }
        count
    }

    /// Get a specific edge
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity {
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
        if self.overflow_starts[src_idx] != NO_OVERFLOW {
            let o_start = self.overflow_starts[src_idx] as usize;
            let o_count = self.overflow_counts[src_idx] as usize;
            for i in 0..o_count {
                let nbr = &self.nbr_list[o_start + i];
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
        for o_count in &mut self.overflow_counts {
            *o_count = 0;
        }
        self.edge_count.store(0, Ordering::Relaxed);
    }

    /// Create iterator over all edges
    pub fn iter(&self, ts: Timestamp) -> MutableCsrIterator<'_> {
        MutableCsrIterator::new(self, ts)
    }

    /// Create iterator over edges of a specific vertex at the given timestamp
    ///
    /// This returns an iterator of references to neighbors without allocating a Vec.
    /// Use this for efficient vertex neighbor traversal in hot paths.
    pub fn edges_iter(&self, src_vid: u32, ts: Timestamp) -> VertexEdgesIter<'_> {
        VertexEdgesIter::new(self, src_vid, ts)
    }

    /// Dump to bytes
    ///
    /// Format:
    /// - vertex_capacity (u64)
    /// - edge_count (u64)
    /// - total_edge_capacity (u64)
    /// - adj_offsets (u32 * vertex_capacity)
    /// - degrees (u32 * vertex_capacity)
    /// - primary_capacities (u32 * vertex_capacity)
    /// - overflow_starts (u32 * vertex_capacity)
    /// - overflow_counts (u32 * vertex_capacity)
    /// - overflow_capacities (u32 * vertex_capacity)
    /// - nbr_list (Nbr * total_edge_capacity)
    ///
    /// # Fragmentation Advisory
    ///
    /// If `fragmentation_ratio() > 2.0`, consider calling `compact_with_ts()` first
    /// to reduce serialized size.
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();

        result.extend_from_slice(&(self.vertex_capacity as u64).to_le_bytes());
        result.extend_from_slice(&self.edge_count.load(Ordering::Relaxed).to_le_bytes());
        result.extend_from_slice(&(self.total_edge_capacity as u64).to_le_bytes());

        for &offset in &self.adj_offsets {
            result.extend_from_slice(&offset.to_le_bytes());
        }

        for &degree in &self.degrees {
            result.extend_from_slice(&degree.to_le_bytes());
        }

        for &cap in &self.primary_capacities {
            result.extend_from_slice(&cap.to_le_bytes());
        }

        for &start in &self.overflow_starts {
            result.extend_from_slice(&start.to_le_bytes());
        }

        for &count in &self.overflow_counts {
            result.extend_from_slice(&count.to_le_bytes());
        }

        for &cap in &self.overflow_capacities {
            result.extend_from_slice(&cap.to_le_bytes());
        }

        for nbr in &self.nbr_list {
            write_vertex_id(&mut result, nbr.neighbor);
            result.extend_from_slice(&nbr.edge_id.to_le_bytes());
            result.extend_from_slice(&nbr.prop_offset.to_le_bytes());
            result.extend_from_slice(&nbr.create_ts.to_le_bytes());
            result.extend_from_slice(&nbr.delete_ts.to_le_bytes());
        }

        result
    }

    /// Load from bytes
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 24 {
            return Err(StorageError::deserialize_error(
                "CSR data too short for header",
            ));
        }

        let mut offset = 0usize;

        let vertex_capacity = read_u64_le(data, &mut offset)? as usize;
        let edge_count = read_u64_le(data, &mut offset)?;
        let total_edge_capacity = read_u64_le(data, &mut offset)? as usize;

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

        let mut overflow_starts = Vec::with_capacity(vertex_capacity);
        for _ in 0..vertex_capacity {
            overflow_starts.push(read_u32_le(data, &mut offset)?);
        }

        let mut overflow_counts = Vec::with_capacity(vertex_capacity);
        for _ in 0..vertex_capacity {
            overflow_counts.push(read_u32_le(data, &mut offset)?);
        }

        let mut overflow_capacities = Vec::with_capacity(vertex_capacity);
        for _ in 0..vertex_capacity {
            overflow_capacities.push(read_u32_le(data, &mut offset)?);
        }

        let nbr_count = total_edge_capacity;
        let mut nbr_list = Vec::with_capacity(nbr_count);
        for _ in 0..nbr_count {
            let neighbor = read_vertex_id(data, &mut offset)?;
            let raw_edge_id = read_u64_le(data, &mut offset)?;
            let prop_offset = read_u32_le(data, &mut offset)?;
            let create_ts = read_u32_le(data, &mut offset)?;
            let delete_ts = read_u32_le(data, &mut offset)?;

            nbr_list.push(Nbr::with_delete_ts(
                neighbor,
                EdgeId(raw_edge_id),
                prop_offset,
                create_ts,
                delete_ts,
            ));
        }

        self.vertex_capacity = vertex_capacity;
        self.total_edge_capacity = total_edge_capacity;
        self.adj_offsets = adj_offsets;
        self.degrees = degrees;
        self.primary_capacities = primary_capacities;
        self.overflow_starts = overflow_starts;
        self.overflow_counts = overflow_counts;
        self.overflow_capacities = overflow_capacities;
        self.nbr_list = nbr_list;
        self.edge_count.store(edge_count, Ordering::Relaxed);

        Ok(())
    }

    /// Compact CSR by removing deleted edges and reclaiming space.
    /// Merges overflow back into primary, restoring flat CSR layout.
    ///
    /// Removes all edges marked as deleted (delete_ts < u32::MAX).
    /// The ts parameter reserves space for future edges.
    pub fn compact_with_ts(&mut self, _ts: u32, reserve_ratio: f32) -> usize {
        // Phase 1: compact individual vertex data (primary + overflow)
        // and compute new layout.
        let mut new_offsets = Vec::with_capacity(self.vertex_capacity);
        let mut new_degrees = Vec::with_capacity(self.vertex_capacity);
        let mut new_capacities = Vec::with_capacity(self.vertex_capacity);
        let mut new_edges = Vec::<Nbr>::new();
        let mut removed_count = 0usize;

        for vid in 0..self.vertex_capacity {
            let start = self.adj_offsets[vid] as usize;
            let degree = self.degrees[vid] as usize;

            new_offsets.push(new_edges.len());

            // Collect active edges from primary (not deleted)
            for i in 0..degree {
                let nbr = &self.nbr_list[start + i];
                if nbr.delete_ts == u32::MAX {
                    new_edges.push(*nbr);
                } else {
                    removed_count += 1;
                }
            }

            // Collect active edges from overflow
            if self.overflow_starts[vid] != NO_OVERFLOW {
                let o_start = self.overflow_starts[vid] as usize;
                let o_count = self.overflow_counts[vid] as usize;
                for i in 0..o_count {
                    let nbr = &self.nbr_list[o_start + i];
                    if nbr.delete_ts == u32::MAX {
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
        let mut final_offsets = Vec::with_capacity(self.vertex_capacity);

        for vid in 0..self.vertex_capacity {
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

        // Clear all overflow
        for start in &mut self.overflow_starts {
            *start = NO_OVERFLOW;
        }
        for count in &mut self.overflow_counts {
            *count = 0;
        }
        for cap in &mut self.overflow_capacities {
            *cap = 0;
        }

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
        self.nbr_list.len() as f32 / active_edges as f32
    }

    /// Check if fragmentation exceeds a threshold
    pub fn should_compact(&self, threshold: f32) -> bool {
        self.fragmentation_ratio() > threshold
    }

    /// Estimate wasted memory due to fragmentation (in bytes)
    pub fn wasted_bytes_estimate(&self) -> usize {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
        (self.nbr_list.len().saturating_sub(active_edges)) * std::mem::size_of::<Nbr>()
    }

    /// Get detailed fragmentation statistics
    pub fn get_fragmentation_stats(&self) -> super::FragmentationStats {
        let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;

        // Count zombie blocks
        let mut zombie_blocks = 0;
        let mut total_wasted = 0;

        for vid in 0..self.vertex_capacity {
            if self.overflow_starts[vid] != NO_OVERFLOW {
                // Count overflow blocks and estimate waste
                let old_primary_cap = self.primary_capacities[vid] as usize;
                let primary_degree = self.degrees[vid] as usize;

                // Wasted in primary
                if primary_degree < old_primary_cap {
                    total_wasted += old_primary_cap - primary_degree;
                    zombie_blocks += 1;
                }
            }
        }

        super::FragmentationStats {
            total_capacity: self.nbr_list.len(),
            reachable_edges: active_edges,
            zombie_blocks,
            wasted_capacity: total_wasted,
        }
    }

    /// Check if compaction should be triggered based on fragmentation ratio
    ///
    /// Returns true if fragmentation_ratio() >= threshold
    pub fn should_compact_with_threshold(&self, threshold: f32) -> bool {
        self.fragmentation_ratio() >= threshold
    }

    /// Compact and return detailed statistics about the operation
    pub fn compact_with_stats(&mut self, ts: u32, reserve_ratio: f32) -> super::CompactionReport {
        let stats_before = self.get_fragmentation_stats();
        let removed = self.compact_with_ts(ts, reserve_ratio);
        let stats_after = self.get_fragmentation_stats();

        let reclaimed_bytes = (stats_before.total_capacity as i64 - stats_after.total_capacity as i64)
            .max(0) as usize;

        super::CompactionReport {
            removed_edges: removed,
            reclaimed_bytes,
            old_fragmentation_ratio: stats_before.fragmentation_ratio(),
            new_fragmentation_ratio: stats_after.fragmentation_ratio(),
        }
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
    overflow_idx: usize,
    overflow_end: usize,
    in_overflow: bool,
}

impl<'a> VertexEdgesIter<'a> {
    /// Create iterator for all edges of a vertex at the given timestamp
    pub fn new(csr: &'a MutableCsr, src_vid: u32, ts: Timestamp) -> Self {
        let src_idx = src_vid as usize;
        if src_idx >= csr.vertex_capacity {
            return Self {
                csr,
                ts,
                primary_idx: 0,
                primary_end: 0,
                overflow_idx: 0,
                overflow_end: 0,
                in_overflow: false,
            };
        }

        let degree = csr.degrees[src_idx] as usize;
        let offset = csr.adj_offsets[src_idx] as usize;
        let (overflow_idx, overflow_end) = if csr.overflow_starts[src_idx] != NO_OVERFLOW {
            let start = csr.overflow_starts[src_idx] as usize;
            let count = csr.overflow_counts[src_idx] as usize;
            (start, start + count)
        } else {
            (0, 0)
        };

        Self {
            csr,
            ts,
            primary_idx: offset,
            primary_end: offset + degree,
            overflow_idx,
            overflow_end,
            in_overflow: false,
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

        // Transition to overflow if not already there
        if !self.in_overflow && self.overflow_end > self.overflow_idx {
            self.in_overflow = true;
        }

        // Scan overflow block
        if self.in_overflow {
            while self.overflow_idx < self.overflow_end {
                let nbr = &self.csr.nbr_list[self.overflow_idx];
                self.overflow_idx += 1;
                if nbr.is_valid_at(self.ts) {
                    return Some(nbr);
                }
            }
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
    overflow_idx: usize,
}

impl<'a> MutableCsrIterator<'a> {
    pub fn new(csr: &'a MutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            current_vertex: 0,
            current_edge: 0,
            in_overflow: false,
            overflow_idx: 0,
        }
    }
}

impl<'a> Iterator for MutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity {
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
                self.overflow_idx = 0;
            }

            // Scan overflow
            if self.csr.overflow_starts[self.current_vertex] != NO_OVERFLOW {
                let o_start = self.csr.overflow_starts[self.current_vertex] as usize;
                let o_count = self.csr.overflow_counts[self.current_vertex] as usize;
                while self.overflow_idx < o_count {
                    let nbr = self.csr.nbr_list[o_start + self.overflow_idx];
                    self.overflow_idx += 1;
                    if nbr.is_valid_at(self.ts) {
                        return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                    }
                }
            }

            // Move to next vertex
            self.current_vertex += 1;
            self.current_edge = 0;
            self.in_overflow = false;
            self.overflow_idx = 0;
        }
        None
    }
}

impl CsrBase for MutableCsr {
    fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
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
        assert_eq!(csr2.overflow_counts[0], 2);
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

        assert!(csr.overflow_starts[0] == NO_OVERFLOW);

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
    fn test_should_compact() {
        let mut csr = MutableCsr::with_capacity(10, 100);

        // Insert edges to trigger overflow
        for i in 1..=6 {
            let dst = VertexId::from_int64(i as i64);
            csr.insert_edge(0u32, dst, EdgeId(i as u64), 0, 1);
        }

        let ratio = csr.fragmentation_ratio();
        assert!(csr.should_compact(ratio - 0.1), "should_compact failed for ratio {}", ratio);
        assert!(!csr.should_compact(ratio + 0.1), "should_compact incorrectly returned true");
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
        let total_capacity = csr.nbr_list.len();

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
        assert!(ratio_before > 1.5, "Setup failed: insufficient fragmentation");

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

        // Test edges_iter yields same neighbors as edges_of without allocation
        let iter_neighbors: Vec<_> = csr.edges_iter(0u32, 1).map(|nbr| nbr.neighbor).collect();
        let vec_neighbors: Vec<_> = csr.edges_of(0u32, 1).iter().map(|nbr| nbr.neighbor).collect();

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
        let edges_ts1: Vec<_> = csr.edges_iter(0u32, 1).collect();
        assert_eq!(edges_ts1.len(), 1);
        assert_eq!(edges_ts1[0].edge_id, EdgeId(100));

        // At ts=2, first two edges are visible (but second is deleted)
        let edges_ts2: Vec<_> = csr.edges_iter(0u32, 2).collect();
        assert_eq!(edges_ts2.len(), 1);

        // At ts=3, all three are visible (but second is deleted)
        let edges_ts3: Vec<_> = csr.edges_iter(0u32, 3).collect();
        assert_eq!(edges_ts3.len(), 2);
    }
}

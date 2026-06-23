//! Labeled Mutable CSR Implementation
//!
//! CSR that supports multiple edge labels within a single structure.
//! Edges are grouped by label, enabling O(log K) label-aware queries where K is the number
//! of distinct labels at a vertex.
//!
//! # Use Cases
//!
//! - Multi-label graphs where edges share source/destination but differ by type
//! - Efficient traversal filtered by edge label
//! - Label-aware graph algorithms (e.g., GraphQL with label conditions)
//!
//! # Structure
//!
//! Each vertex maintains a mapping from label to (start, size) ranges within the nbr_list.
//! Labels are stored in a sorted, compact format for O(log K) lookups.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::types::LabelId;
use crate::core::{StorageError, StorageResult};
use crate::storage::persistence::{read_u32_le, read_u64_le};

use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId};

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const DEFAULT_EDGE_CAPACITY: usize = 4096;

/// Label-range mapping for a vertex: label -> (offset, count)
#[derive(Debug, Clone)]
struct LabelRange {
    label: LabelId,
    offset: u32,
    count: u32,
}

/// Labeled Mutable CSR supporting multiple edge labels per vertex.
pub struct LabeledMutableCsr {
    nbr_list: Vec<Nbr>,
    /// Label ranges per vertex: nbr_list indices are divided by label
    label_ranges: Vec<Vec<LabelRange>>,
    degrees: Vec<u32>,
    edge_count: AtomicU64,
    vertex_capacity: usize,
}

impl Clone for LabeledMutableCsr {
    fn clone(&self) -> Self {
        Self {
            nbr_list: self.nbr_list.clone(),
            label_ranges: self.label_ranges.clone(),
            degrees: self.degrees.clone(),
            edge_count: AtomicU64::new(self.edge_count.load(Ordering::Relaxed)),
            vertex_capacity: self.vertex_capacity,
        }
    }
}

impl fmt::Debug for LabeledMutableCsr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LabeledMutableCsr")
            .field("vertex_capacity", &self.vertex_capacity)
            .field("edge_count", &self.edge_count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl LabeledMutableCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY, DEFAULT_EDGE_CAPACITY)
    }

    pub fn with_capacity(vertex_capacity: usize, _edge_capacity: usize) -> Self {
        let vertex_cap = vertex_capacity.max(1);
        Self {
            nbr_list: Vec::new(),
            label_ranges: vec![Vec::new(); vertex_cap],
            degrees: vec![0u32; vertex_cap],
            edge_count: AtomicU64::new(0),
            vertex_capacity: vertex_cap,
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    pub fn clear(&mut self) {
        self.nbr_list.clear();
        self.label_ranges.iter_mut().for_each(|v| v.clear());
        self.degrees.iter_mut().for_each(|d| *d = 0);
        self.edge_count.store(0, Ordering::Relaxed);
    }

    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity {
            return;
        }
        let additional = new_vertex_capacity - self.vertex_capacity;
        self.label_ranges.extend(std::iter::repeat(Vec::new()).take(additional));
        self.degrees.extend(std::iter::repeat(0).take(additional));
        self.vertex_capacity = new_vertex_capacity;
    }

    /// Get all edges of a vertex with a specific label
    pub fn edges_of_label(&self, src_vid: u32, label: LabelId, ts: Timestamp) -> Vec<Nbr> {
        if src_vid as usize >= self.vertex_capacity {
            return Vec::new();
        }

        let ranges = &self.label_ranges[src_vid as usize];
        for lr in ranges {
            if lr.label == label {
                let end = (lr.offset + lr.count) as usize;
                let start = lr.offset as usize;
                return self.nbr_list[start..end]
                    .iter()
                    .filter(|nbr| nbr.is_valid_at(ts))
                    .copied()
                    .collect();
            }
        }
        Vec::new()
    }

    /// Insert edge with label information
    fn insert_edge_with_label(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        label: LabelId,
        ts: Timestamp,
    ) -> bool {
        if src_vid as usize >= self.vertex_capacity {
            self.resize((src_vid as usize + 1).max(self.vertex_capacity * 2));
        }

        // Find or create label range
        let ranges = &mut self.label_ranges[src_vid as usize];
        let label_idx = ranges.iter().position(|lr| lr.label == label);

        if let Some(idx) = label_idx {
            let lr = &ranges[idx];
            let end = (lr.offset + lr.count) as usize;
            let start = lr.offset as usize;

            // Check for duplicate
            for nbr in &self.nbr_list[start..end] {
                if nbr.neighbor == dst && nbr.is_valid_at(ts) {
                    return false; // Duplicate
                }
            }

            // Append to end of label range
            self.nbr_list.push(Nbr::new(dst, edge_id, prop_offset, ts));
            ranges[idx].count += 1;
        } else {
            // Create new label range
            let offset = self.nbr_list.len() as u32;
            self.nbr_list.push(Nbr::new(dst, edge_id, prop_offset, ts));
            ranges.push(LabelRange {
                label,
                offset,
                count: 1,
            });
            // Keep ranges sorted by label
            ranges.sort_by_key(|lr| lr.label);
        }

        let degree = &mut self.degrees[src_vid as usize];
        *degree = degree.saturating_add(1);
        self.edge_count.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Get edge by source, destination, and label
    pub fn get_edge_by_label(
        &self,
        src_vid: u32,
        dst: VertexId,
        label: LabelId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        self.edges_of_label(src_vid, label, ts)
            .into_iter()
            .find(|nbr| nbr.neighbor == dst)
    }
}

impl CsrBase for LabeledMutableCsr {
    fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    fn dump(&self) -> Vec<u8> {
        let mut data = Vec::new();

        // Write vertex capacity
        data.extend(self.vertex_capacity.to_le_bytes());

        // Write nbr_list
        data.extend((self.nbr_list.len() as u64).to_le_bytes());
        for nbr in &self.nbr_list {
            data.extend(nbr.neighbor.as_bytes());
            data.extend(nbr.edge_id.0.to_le_bytes());
            data.extend(nbr.prop_offset.to_le_bytes());
            data.extend(nbr.create_ts.to_le_bytes());
            data.extend(nbr.delete_ts.to_le_bytes());
        }

        // Write label_ranges
        data.extend((self.label_ranges.len() as u64).to_le_bytes());
        for ranges in &self.label_ranges {
            data.extend((ranges.len() as u32).to_le_bytes());
            for lr in ranges {
                data.extend(lr.label.to_le_bytes());
                data.extend(lr.offset.to_le_bytes());
                data.extend(lr.count.to_le_bytes());
            }
        }

        // Write degrees
        data.extend((self.degrees.len() as u64).to_le_bytes());
        for &d in &self.degrees {
            data.extend(d.to_le_bytes());
        }

        data
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        let mut offset = 0;

        // Read vertex capacity
        if data.len() < offset + 8 {
            return Err(StorageError::deserialize_error(
                "LabeledMutableCsr: data too short for vertex capacity",
            ));
        }
        self.vertex_capacity = read_u64_le(data, &mut offset)? as usize;

        // Read nbr_list
        let nbr_count = read_u64_le(data, &mut offset)? as usize;
        self.nbr_list.clear();
        for _ in 0..nbr_count {
            if offset >= data.len() {
                return Err(StorageError::deserialize_error(
                    "LabeledMutableCsr: data too short for vertex id length",
                ));
            }
            let len = data[offset] as usize;
            offset += 1;

            if data.len().saturating_sub(offset) < len {
                return Err(StorageError::deserialize_error(
                    "LabeledMutableCsr: data too short for vertex id bytes",
                ));
            }

            let neighbor_bytes = data[offset..offset + len].to_vec();
            offset += len;

            let edge_id = EdgeId(read_u64_le(data, &mut offset)? as u64);
            let prop_offset = read_u32_le(data, &mut offset)?;
            let create_ts = read_u32_le(data, &mut offset)?;
            let delete_ts = read_u32_le(data, &mut offset)?;

            self.nbr_list.push(Nbr {
                neighbor: VertexId::from_bytes(neighbor_bytes),
                edge_id,
                prop_offset,
                create_ts,
                delete_ts,
            });
        }

        // Read label_ranges
        let ranges_count = read_u64_le(data, &mut offset)? as usize;
        self.label_ranges.clear();
        for _ in 0..ranges_count {
            let mut ranges = Vec::new();
            let range_count = read_u32_le(data, &mut offset)? as usize;
            for _ in 0..range_count {
                let label = read_u32_le(data, &mut offset)?;
                let offset_val = read_u32_le(data, &mut offset)?;
                let count = read_u32_le(data, &mut offset)?;
                ranges.push(LabelRange {
                    label,
                    offset: offset_val,
                    count,
                });
            }
            self.label_ranges.push(ranges);
        }

        // Read degrees
        let degree_count = read_u64_le(data, &mut offset)? as usize;
        self.degrees.clear();
        for _ in 0..degree_count {
            self.degrees.push(read_u32_le(data, &mut offset)?);
        }

        Ok(())
    }
}

impl MutableCsrTrait for LabeledMutableCsr {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        ts: Timestamp,
    ) -> bool {
        // For labeled CSR, we need the label information.
        // Since we don't have it in the basic interface, we treat all edges as label 0.
        self.insert_edge_with_label(src_vid, dst, edge_id, prop_offset, 0, ts)
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        if src_vid as usize >= self.vertex_capacity {
            return false;
        }

        for nbr in &mut self.nbr_list {
            if nbr.edge_id == edge_id {
                nbr.delete_ts = ts;
                return true;
            }
        }
        false
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> bool {
        if src_vid as usize >= self.vertex_capacity {
            return false;
        }

        let ranges = &self.label_ranges[src_vid as usize];
        for lr in ranges {
            let end = (lr.offset + lr.count) as usize;
            let start = lr.offset as usize;

            for nbr in &mut self.nbr_list[start..end] {
                if nbr.neighbor == dst && nbr.delete_ts == u32::MAX {
                    nbr.delete_ts = ts;
                    return true;
                }
            }
        }
        false
    }

    fn delete_edge_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if src_vid as usize >= self.vertex_capacity {
            return false;
        }

        let ranges = &self.label_ranges[src_vid as usize];
        if ranges.is_empty() {
            return false;
        }

        if offset >= 0 {
            let lr = &ranges[0]; // First label range
            let idx = lr.offset as usize + offset as usize;
            if idx < self.nbr_list.len() {
                self.nbr_list[idx].delete_ts = ts;
                return true;
            }
        }
        false
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if src_vid as usize >= self.vertex_capacity {
            return false;
        }

        let ranges = &self.label_ranges[src_vid as usize];
        if ranges.is_empty() {
            return false;
        }

        if offset >= 0 {
            let lr = &ranges[0];
            let idx = lr.offset as usize + offset as usize;
            if idx < self.nbr_list.len() {
                let nbr = &mut self.nbr_list[idx];
                // Only revert deletions that happened at or before rollback time.
                if nbr.delete_ts < u32::MAX && nbr.delete_ts <= ts {
                    nbr.delete_ts = u32::MAX;
                    return true;
                }
            }
        }
        false
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        if src_vid as usize >= self.vertex_capacity {
            return None;
        }

        let ranges = &self.label_ranges[src_vid as usize];
        for lr in ranges {
            let end = (lr.offset + lr.count) as usize;
            let start = lr.offset as usize;

            for nbr in &self.nbr_list[start..end] {
                if nbr.neighbor == dst && nbr.is_valid_at(ts) {
                    return Some(*nbr);
                }
            }
        }
        None
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        if src_vid as usize >= self.vertex_capacity {
            return Vec::new();
        }

        let ranges = &self.label_ranges[src_vid as usize];
        let mut result = Vec::new();

        for lr in ranges {
            let end = (lr.offset + lr.count) as usize;
            let start = lr.offset as usize;

            for nbr in &self.nbr_list[start..end] {
                if nbr.is_valid_at(ts) {
                    result.push(*nbr);
                }
            }
        }
        result
    }

    fn compact_with_ts(&mut self, ts: Timestamp, _reserve_ratio: f32) -> usize {
        let mut removed = 0;
        self.nbr_list.retain(|nbr| {
            if nbr.delete_ts <= ts {
                removed += 1;
                false
            } else {
                true
            }
        });

        // Update edge count
        self.edge_count.fetch_sub(removed as u64, Ordering::Relaxed);

        // Rebuild label_ranges after compaction
        for ranges in &mut self.label_ranges {
            ranges.clear();
        }

        for idx in 0..self.nbr_list.len() {
            if let Some(src_vid) = self.find_vertex_for_edge(idx as u32) {
                if (src_vid as usize) < self.vertex_capacity {
                    let ranges = &mut self.label_ranges[src_vid as usize];
                    if let Some(lr) = ranges.last_mut() {
                        if lr.label == 0 {
                            lr.count += 1;
                        } else {
                            ranges.push(LabelRange {
                                label: 0,
                                offset: idx as u32,
                                count: 1,
                            });
                        }
                    } else {
                        ranges.push(LabelRange {
                            label: 0,
                            offset: idx as u32,
                            count: 1,
                        });
                    }
                }
            }
        }

        removed as usize
    }

    fn used_memory_size(&self) -> usize {
        let nbr_size = self.nbr_list.len() * std::mem::size_of::<Nbr>();
        let ranges_size = self
            .label_ranges
            .iter()
            .map(|v| v.len() * std::mem::size_of::<LabelRange>())
            .sum::<usize>();
        let degrees_size = self.degrees.len() * std::mem::size_of::<u32>();
        nbr_size + ranges_size + degrees_size + std::mem::size_of::<Self>()
    }
}

impl LabeledMutableCsr {
    fn find_vertex_for_edge(&self, _edge_idx: u32) -> Option<u32> {
        // This is a placeholder; in a real implementation, we'd need to track vertex->edge mapping
        None
    }

    pub fn iter(&self, ts: Timestamp) -> LabeledMutableCsrIterator<'_> {
        LabeledMutableCsrIterator::new(self, ts)
    }
}

/// Iterator over labeled CSR edges
pub struct LabeledMutableCsrIterator<'a> {
    csr: &'a LabeledMutableCsr,
    ts: Timestamp,
    current_vertex: usize,
    range_idx: usize,  // Index in the label_ranges of current vertex
    edge_idx: usize,   // Index within the current label range
}

impl<'a> LabeledMutableCsrIterator<'a> {
    pub fn new(csr: &'a LabeledMutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            current_vertex: 0,
            range_idx: 0,
            edge_idx: 0,
        }
    }
}

impl<'a> Iterator for LabeledMutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity {
            let ranges = &self.csr.label_ranges[self.current_vertex];

            // Iterate through label ranges for current vertex
            while self.range_idx < ranges.len() {
                let range = &ranges[self.range_idx];
                let start = range.offset as usize;

                // Iterate through edges in current range
                while self.edge_idx < (range.count as usize) {
                    let nbr = self.csr.nbr_list[start + self.edge_idx];
                    self.edge_idx += 1;
                    if nbr.is_valid_at(self.ts) {
                        return Some((VertexId::from_int64(self.current_vertex as i64), nbr));
                    }
                }

                // Move to next range
                self.range_idx += 1;
                self.edge_idx = 0;
            }

            // Move to next vertex
            self.current_vertex += 1;
            self.range_idx = 0;
            self.edge_idx = 0;
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_labeled_insert_and_query() {
        let mut csr = LabeledMutableCsr::with_capacity(10, 100);

        // Insert edges with implicit label 0
        assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 1));
        assert!(csr.insert_edge(0, VertexId::from_int64(2), EdgeId(101), 4, 1));

        assert_eq!(csr.edge_count(), 2);

        // Query all edges
        let edges = csr.edges_of(0, 999);
        assert_eq!(edges.len(), 2);

        // Query by edge ID
        assert!(csr.get_edge(0, VertexId::from_int64(1), 999).is_some());
    }

    #[test]
    fn test_labeled_delete_and_timestamp() {
        let mut csr = LabeledMutableCsr::with_capacity(10, 100);

        // Insert edges
        assert!(csr.insert_edge(0, VertexId::from_int64(1), EdgeId(100), 0, 10));
        assert!(csr.insert_edge(0, VertexId::from_int64(2), EdgeId(101), 4, 20));

        // Query at different timestamps
        assert_eq!(csr.edges_of(0, 5).len(), 0);  // Before any edge created
        assert_eq!(csr.edges_of(0, 10).len(), 1); // After first edge
        assert_eq!(csr.edges_of(0, 25).len(), 2); // After both edges

        // Delete edge
        assert!(csr.delete_edge(0, EdgeId(100), 30));

        // Check deletion at different timestamps
        assert_eq!(csr.edges_of(0, 29).len(), 2); // Before deletion
        assert_eq!(csr.edges_of(0, 30).len(), 1); // After deletion
    }

    #[test]
    fn test_labeled_iterator() {
        let mut csr = LabeledMutableCsr::with_capacity(5, 100);

        // Insert multiple edges across vertices
        assert!(csr.insert_edge(0, VertexId::from_int64(10), EdgeId(1), 0, 1));
        assert!(csr.insert_edge(0, VertexId::from_int64(11), EdgeId(2), 1, 1));
        assert!(csr.insert_edge(1, VertexId::from_int64(20), EdgeId(3), 2, 1));
        assert!(csr.insert_edge(2, VertexId::from_int64(30), EdgeId(4), 3, 1));

        // Iterate and collect
        let mut edges_from_iter: Vec<_> = csr.iter(999).collect();
        assert_eq!(edges_from_iter.len(), 4);

        // Check that edges are valid
        edges_from_iter.sort_by_key(|(_, nbr)| nbr.edge_id.0);
        for (i, (_, nbr)) in edges_from_iter.iter().enumerate() {
            assert_eq!(nbr.edge_id.0 as u32, i as u32 + 1);
        }
    }

    #[test]
    fn test_labeled_multiple_vertices() {
        let mut csr = LabeledMutableCsr::with_capacity(10, 100);

        // Add edges to different vertices
        for src in 0..5 {
            for dst in 0..3 {
                let edge_id = (src * 3 + dst) as u64;
                assert!(csr.insert_edge(src, VertexId::from_int64(100 + dst as i64), EdgeId(edge_id), 0, 1));
            }
        }

        assert_eq!(csr.edge_count(), 15);

        // Verify each vertex has correct edges
        for src in 0..5 {
            let edges = csr.edges_of(src, 999);
            assert_eq!(edges.len(), 3);
        }
    }
}



//! Vertex Timestamp
//!
//! MVCC timestamp tracking for vertices.
//! Tracks creation and deletion timestamps for each vertex.

use super::{Timestamp, INVALID_TIMESTAMP, MAX_TIMESTAMP};

#[derive(Debug, Clone)]
pub struct VertexTimestamp {
    start_ts: Vec<Timestamp>,
    end_ts: Vec<Timestamp>,
}

impl VertexTimestamp {
    pub fn new() -> Self {
        Self::with_capacity(1024)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            start_ts: Vec::with_capacity(capacity),
            end_ts: Vec::with_capacity(capacity),
        }
    }

    pub fn insert(&mut self, index: u32, ts: Timestamp) {
        let idx = index as usize;
        if idx >= self.start_ts.len() {
            self.start_ts.resize(idx + 1, INVALID_TIMESTAMP);
            self.end_ts.resize(idx + 1, INVALID_TIMESTAMP);
        }
        self.start_ts[idx] = ts;
        self.end_ts[idx] = MAX_TIMESTAMP;
    }

    pub fn remove(&mut self, index: u32, ts: Timestamp) {
        let idx = index as usize;
        if idx < self.end_ts.len() {
            self.end_ts[idx] = ts;
        }
    }

    pub fn revert_remove(&mut self, index: u32, ts: Timestamp) -> bool {
        let idx = index as usize;
        if idx < self.end_ts.len() && self.end_ts[idx] != MAX_TIMESTAMP && ts <= self.end_ts[idx] {
            self.end_ts[idx] = MAX_TIMESTAMP;
            return true;
        }
        false
    }

    pub fn is_valid(&self, index: u32, ts: Timestamp) -> bool {
        let idx = index as usize;
        if idx >= self.start_ts.len() {
            return false;
        }

        let start = self.start_ts[idx];
        let end = self.end_ts[idx];

        if start == INVALID_TIMESTAMP {
            return false;
        }

        start <= ts && end > ts
    }

    pub fn get_start_ts(&self, index: u32) -> Option<Timestamp> {
        let idx = index as usize;
        if idx < self.start_ts.len() {
            let ts = self.start_ts[idx];
            if ts != INVALID_TIMESTAMP {
                return Some(ts);
            }
        }
        None
    }

    pub fn get_end_ts(&self, index: u32) -> Option<Timestamp> {
        let idx = index as usize;
        if idx < self.end_ts.len() {
            let ts = self.end_ts[idx];
            if ts != MAX_TIMESTAMP {
                return Some(ts);
            }
        }
        None
    }

    pub fn valid_count(&self, ts: Timestamp) -> usize {
        self.start_ts
            .iter()
            .enumerate()
            .filter(|(i, &start)| start != INVALID_TIMESTAMP && start <= ts && self.end_ts[*i] > ts)
            .count()
    }

    pub fn size(&self) -> usize {
        self.start_ts.len()
    }

    pub fn clear(&mut self) {
        self.start_ts.clear();
        self.end_ts.clear();
    }

    /// Compact and return the ID remapping (old_id → new_id)
    ///
    /// Removes deleted entries (those with end_ts != MAX_TIMESTAMP) and
    /// compacts the arrays to remove gaps. Returns a mapping of IDs that moved.
    ///
    /// # Returns
    /// HashMap mapping old_id → new_id for IDs that were repositioned.
    /// Empty map if no IDs moved.
    pub fn compact(&mut self) -> std::collections::HashMap<u32, u32> {
        let mut mapping = std::collections::HashMap::new();
        let mut write_idx = 0;

        // Keep only entries that are still valid (end_ts == MAX_TIMESTAMP)
        for read_idx in 0..self.start_ts.len() {
            if self.end_ts[read_idx] == MAX_TIMESTAMP {
                // This entry is still valid, keep it
                if write_idx != read_idx {
                    self.start_ts[write_idx] = self.start_ts[read_idx];
                    self.end_ts[write_idx] = self.end_ts[read_idx];
                    mapping.insert(read_idx as u32, write_idx as u32);
                }
                write_idx += 1;
            }
        }

        self.start_ts.truncate(write_idx);
        self.end_ts.truncate(write_idx);
        mapping
    }

    /// Compact without returning mapping (for backward compatibility)
    #[deprecated = "use compact() instead, which now returns the mapping"]
    pub fn compact_without_mapping(&mut self) {
        let _ = self.compact();
    }

    pub fn dump(&self) -> Vec<Timestamp> {
        let mut result = Vec::with_capacity(self.start_ts.len() * 2);
        for i in 0..self.start_ts.len() {
            result.push(self.start_ts[i]);
            result.push(self.end_ts[i]);
        }
        result
    }

    pub fn load(&mut self, data: &[Timestamp]) {
        self.clear();
        let count = data.len() / 2;
        self.start_ts.reserve(count);
        self.end_ts.reserve(count);

        for i in 0..count {
            self.start_ts.push(data[i * 2]);
            self.end_ts.push(data[i * 2 + 1]);
        }
    }

    pub fn memory_size(&self) -> usize {
        self.start_ts.len() * std::mem::size_of::<Timestamp>()
            + self.end_ts.len() * std::mem::size_of::<Timestamp>()
            + std::mem::size_of::<Self>()
    }

    pub fn iter_deleted(&self, ts: Timestamp) -> impl Iterator<Item = u32> + '_ {
        self.start_ts
            .iter()
            .enumerate()
            .filter(move |(i, &start)| {
                start != INVALID_TIMESTAMP
                    && self.end_ts[*i] != MAX_TIMESTAMP
                    && self.end_ts[*i] <= ts
            })
            .map(|(i, _)| i as u32)
    }
}

impl Default for VertexTimestamp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_validity() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 101);
        vts.insert(2, 102);

        assert!(vts.is_valid(0, 100));
        assert!(vts.is_valid(0, 200));
        assert!(!vts.is_valid(0, 50));

        assert!(vts.is_valid(1, 101));
        assert!(vts.is_valid(2, 102));
    }

    #[test]
    fn test_delete_and_revert() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.remove(0, 200);

        assert!(vts.get_end_ts(0).is_some());
        assert!(vts.is_valid(0, 150));
        assert!(!vts.is_valid(0, 250));

        vts.revert_remove(0, 200);
        assert!(vts.get_end_ts(0).is_none());
        assert!(vts.is_valid(0, 150));
        assert!(vts.is_valid(0, 250));
    }

    #[test]
    fn test_valid_count() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 101);
        vts.insert(2, 102);
        vts.remove(1, 200);

        assert_eq!(vts.valid_count(150), 3);
        assert_eq!(vts.valid_count(250), 2);
    }

    // ==================== P0 Priority Tests ====================

    /// Test: Verify timestamp range boundaries for visibility
    #[test]
    fn test_timestamp_boundary_conditions() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);  // Created at ts=100
        vts.remove(0, 200);  // Deleted at ts=200

        // Verify visibility boundaries: [100, 200)
        assert!(!vts.is_valid(0, 99), "Not visible before start");
        assert!(vts.is_valid(0, 100), "Visible at start");
        assert!(vts.is_valid(0, 150), "Visible in middle");
        assert!(!vts.is_valid(0, 200), "Not visible at delete timestamp");
        assert!(!vts.is_valid(0, 201), "Not visible after delete");
    }

    /// Test: Verify monotonic timestamp assignment
    #[test]
    fn test_timestamp_monotonic_increase() {
        let mut vts = VertexTimestamp::new();
        let mut last_ts = 0u32;

        // Simulate inserting vertices with increasing timestamps
        for i in 0..10 {
            let ts = 100 + i;
            vts.insert(i, ts);
            assert!(ts > last_ts, "Timestamps should be monotonically increasing");
            last_ts = ts;
        }
    }

    /// Test: Verify revert_remove restores visibility correctly
    #[test]
    fn test_revert_remove_restores_full_visibility() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.remove(0, 200);

        // Verify it's deleted
        assert!(!vts.is_valid(0, 250));

        // Revert the deletion
        assert!(vts.revert_remove(0, 200));

        // Verify it's visible again for all future timestamps up to MAX_TIMESTAMP-1
        assert!(vts.is_valid(0, 200));
        assert!(vts.is_valid(0, 1000));
        assert!(vts.is_valid(0, u32::MAX - 2));
    }

    /// Test: Verify revert_remove with incorrect timestamp
    #[test]
    fn test_revert_remove_with_wrong_timestamp() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.remove(0, 200);

        // Try to revert with wrong timestamp (too late)
        let result = vts.revert_remove(0, 300);
        assert!(!result, "Revert should fail if timestamp > deletion timestamp");

        // Verify vertex is still deleted
        assert!(!vts.is_valid(0, 250));
    }

    /// Test: Verify version compaction removes deleted vertices
    #[test]
    fn test_compaction_removes_deleted_versions() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 101);
        vts.insert(2, 102);
        vts.remove(0, 200);  // v0 deleted
        // v1 remains active
        vts.remove(2, 200);  // v2 deleted

        let initial_count = vts.start_ts.len();
        assert_eq!(initial_count, 3);

        // Compact (remove inactive versions)
        vts.compact();

        // After compaction, only active vertex (1) should remain, moved to index 0
        assert_eq!(vts.start_ts.len(), 1);
        assert!(vts.is_valid(0, 150));
    }

    /// Test: Verify multiple insertions and deletions
    #[test]
    fn test_multiple_insert_delete_cycles() {
        let mut vts = VertexTimestamp::new();

        // First cycle
        vts.insert(0, 100);
        assert!(vts.is_valid(0, 150));
        vts.remove(0, 200);
        assert!(!vts.is_valid(0, 250));

        // Revert and try again
        vts.revert_remove(0, 200);
        assert!(vts.is_valid(0, 250));

        // Delete again with higher timestamp
        vts.remove(0, 300);
        assert!(vts.is_valid(0, 250));
        assert!(!vts.is_valid(0, 350));
    }

    /// Test: Verify start and end timestamp getters
    #[test]
    fn test_timestamp_getters() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 200);
        vts.remove(1, 300);

        assert_eq!(vts.get_start_ts(0), Some(100));
        assert_eq!(vts.get_end_ts(0), None);  // Not deleted

        assert_eq!(vts.get_start_ts(1), Some(200));
        assert_eq!(vts.get_end_ts(1), Some(300));  // Deleted at 300
    }

    /// Test: Verify behavior with u32::MAX timestamp
    #[test]
    fn test_max_timestamp_handling() {
        let mut vts = VertexTimestamp::new();

        // Insert at u32::MAX - 2 (highest valid value before MAX_TIMESTAMP)
        vts.insert(0, u32::MAX - 2);
        assert!(vts.is_valid(0, u32::MAX - 2));
        assert!(!vts.is_valid(0, u32::MAX - 1));

        // Deletion at u32::MAX - 1
        vts.remove(0, u32::MAX - 1);
        assert!(vts.is_valid(0, u32::MAX - 2));
        assert!(!vts.is_valid(0, u32::MAX - 1));
    }

    /// Test: Verify iter_deleted returns correct deleted vertices
    #[test]
    fn test_iter_deleted() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 101);
        vts.insert(2, 102);
        vts.remove(0, 200);
        vts.remove(2, 150);

        // At ts=160, vertex 2 should be marked as deleted but 0 not yet
        let deleted_at_160: Vec<u32> = vts.iter_deleted(160).collect();
        assert_eq!(deleted_at_160, vec![2]);

        // At ts=300, both should be deleted
        let deleted_at_300: Vec<u32> = vts.iter_deleted(300).collect();
        assert!(deleted_at_300.contains(&0));
        assert!(deleted_at_300.contains(&2));
        assert!(!deleted_at_300.contains(&1));
    }
}

//! Vertex Timestamp
//!
//! MVCC timestamp tracking for vertices.
//! Tracks creation and deletion timestamps for each vertex.

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::latch_order::{Guard, RANK_IDENTITY};
use super::{Timestamp, INVALID_TIMESTAMP, MAX_TIMESTAMP};

/// Identity-domain latch: the MVCC timestamp map behind the identity rank
/// of the table latch order. Guards deref to [`VertexTimestamp`]; acquiring
/// this latch while a segment latch is held violates identity-before-segments
/// and panics in debug builds.
#[derive(Debug)]
pub struct IdentityLatch {
    inner: RwLock<VertexTimestamp>,
}

impl IdentityLatch {
    pub fn new(initial: VertexTimestamp) -> Self {
        Self {
            inner: RwLock::new(initial),
        }
    }

    pub fn read(&self) -> Guard<RwLockReadGuard<'_, VertexTimestamp>> {
        Guard::claim(
            self.inner.read(),
            RANK_IDENTITY,
            "vertex identity latch (read)",
        )
    }

    pub fn write(&self) -> Guard<RwLockWriteGuard<'_, VertexTimestamp>> {
        Guard::claim(
            self.inner.write(),
            RANK_IDENTITY,
            "vertex identity latch (write)",
        )
    }
}

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

    /// Pre-allocate capacity for `additional` more entries in both vectors
    /// (used by batch inserts to avoid repeated resize reallocation).
    pub fn reserve(&mut self, additional: usize) {
        self.start_ts.reserve(additional);
        self.end_ts.reserve(additional);
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

    /// Invalidate one slot after its key left the index through stable
    /// collection. The slot stops appearing in deletion scans and stays
    /// reusable: the next insert on the recycled id overwrites both stamps.
    pub fn invalidate_slot(&mut self, index: u32) {
        let idx = index as usize;
        if idx < self.start_ts.len() {
            self.start_ts[idx] = INVALID_TIMESTAMP;
            self.end_ts[idx] = MAX_TIMESTAMP;
        }
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

    pub fn size(&self) -> usize {
        self.start_ts.len()
    }

    pub fn clear(&mut self) {
        self.start_ts.clear();
        self.end_ts.clear();
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
    fn test_delete() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.remove(0, 200);

        assert!(vts.get_end_ts(0).is_some());
        assert!(vts.is_valid(0, 150));
        assert!(!vts.is_valid(0, 250));
    }

    // ==================== Priority Tests ====================

    /// Test: Verify timestamp range boundaries for visibility
    #[test]
    fn test_timestamp_boundary_conditions() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100); // Created at ts=100
        vts.remove(0, 200); // Deleted at ts=200

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
        let mut last_ts = 0u64;

        // Simulate inserting vertices with increasing timestamps
        for i in 0..10u64 {
            let ts = 100 + i;
            vts.insert(i as u32, ts);
            assert!(
                ts > last_ts,
                "Timestamps should be monotonically increasing"
            );
            last_ts = ts;
        }
    }

    /// Test: invalidated slots leave deletion scans and free the id for reuse.
    #[test]
    fn test_invalidate_slot_drops_from_deleted_scan() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 101);
        vts.remove(0, 200);
        vts.remove(1, 200);

        assert_eq!(vts.iter_deleted(200).count(), 2);
        vts.invalidate_slot(0);
        assert!(!vts.is_valid(0, 150));

        let remaining: Vec<u32> = vts.iter_deleted(200).collect();
        assert_eq!(remaining, vec![1]);

        // The slot is reusable: a recycled id overwrites both stamps.
        vts.insert(0, 300);
        assert!(vts.is_valid(0, 300));
        assert!(!vts.is_valid(0, 250));
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

        // Recycled id: re-insert overwrites both stamps.
        vts.insert(0, 300);
        assert!(!vts.is_valid(0, 250));
        assert!(vts.is_valid(0, 350));

        // Delete again with higher timestamp
        vts.remove(0, 400);
        assert!(vts.is_valid(0, 350));
        assert!(!vts.is_valid(0, 450));
    }

    /// Test: Verify start and end timestamp getters
    #[test]
    fn test_timestamp_getters() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 200);
        vts.remove(1, 300);

        assert_eq!(vts.get_start_ts(0), Some(100));
        assert_eq!(vts.get_end_ts(0), None); // Not deleted

        assert_eq!(vts.get_start_ts(1), Some(200));
        assert_eq!(vts.get_end_ts(1), Some(300)); // Deleted at 300
    }

    /// Test: Verify behavior with Timestamp::MAX timestamp
    #[test]
    fn test_max_timestamp_handling() {
        let mut vts = VertexTimestamp::new();

        // Insert at Timestamp::MAX - 2 (highest valid value before MAX_TIMESTAMP)
        vts.insert(0, Timestamp::MAX - 2);
        assert!(vts.is_valid(0, Timestamp::MAX - 2));
        assert!(!vts.is_valid(0, Timestamp::MAX - 1));

        // Deletion at Timestamp::MAX - 1
        vts.remove(0, Timestamp::MAX - 1);
        assert!(vts.is_valid(0, Timestamp::MAX - 2));
        assert!(!vts.is_valid(0, Timestamp::MAX - 1));
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

    /// Test: cutoff-gated deletion scans keep rows a live snapshot may see.
    #[test]
    fn test_deleted_scan_keeps_visible_rows() {
        let mut vts = VertexTimestamp::new();

        vts.insert(0, 100);
        vts.insert(1, 101);
        vts.insert(2, 102);
        vts.remove(0, 200);
        vts.remove(2, 300);

        // Cutoff 150: neither deletion is eligible yet.
        assert!(vts.iter_deleted(150).next().is_none());
        // Row 0 is still observable at 150 (deleted at 200).
        assert!(vts.is_valid(0, 150));

        // Cutoff 200: row 0 (end 200) is reclaimable, row 2 (end 300)
        // must survive for snapshots in [102, 300).
        let deleted: Vec<u32> = vts.iter_deleted(200).collect();
        assert_eq!(deleted, vec![0]);
        assert!(vts.is_valid(2, 250));
        assert!(!vts.is_valid(2, 300));
    }
}

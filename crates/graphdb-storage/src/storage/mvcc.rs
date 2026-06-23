//! Unified MVCC (Multi-Version Concurrency Control) infrastructure
//!
//! Provides a consistent interface for snapshot isolation across all storage tables
//! (VertexTable, EdgeTable, PropertyTable). Implements a tiered tombstone management
//! system for efficient garbage collection.

use crate::core::error::StorageResult;
use crate::core::types::storage_ids::Timestamp;
use std::collections::HashMap;

/// Snapshot handle for MVCC - identifies a consistent snapshot at a specific timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotHandle {
    /// Timestamp of the snapshot
    pub ts: Timestamp,
    /// Monotonically increasing handle to distinguish concurrent snapshots at the same timestamp
    pub id: u64,
}

impl SnapshotHandle {
    /// Create a new snapshot handle
    #[inline]
    pub fn new(ts: Timestamp, id: u64) -> Self {
        Self { ts, id }
    }
}

/// Unified MVCC interface that all storage tables must implement
///
/// Provides methods for registering and unregistering snapshots, tracking active snapshots,
/// and performing garbage collection on old versions.
pub trait MVCCTable {
    /// Register a new snapshot at the given timestamp
    ///
    /// Returns a SnapshotHandle that must be used to unregister the snapshot later.
    /// Each call increments the reference count for this timestamp.
    fn register_snapshot(&mut self, ts: Timestamp) -> StorageResult<SnapshotHandle>;

    /// Unregister a snapshot, allowing GC of related version data
    ///
    /// This decrements the reference count for the snapshot's timestamp.
    /// When the count reaches 0, the timestamp is removed from tracking.
    fn unregister_snapshot(&mut self, handle: SnapshotHandle) -> StorageResult<()>;

    /// Get the count of currently active snapshots
    fn active_snapshot_count(&self) -> usize;

    /// Get the minimum timestamp among all active snapshots
    ///
    /// Returns u32::MAX if no active snapshots exist.
    fn min_active_snapshot_ts(&self) -> Timestamp;

    /// Perform garbage collection on version data older than min_ts
    ///
    /// Returns the number of version entries cleaned up.
    fn gc(&mut self, min_ts: Timestamp) -> StorageResult<usize>;
}

/// Tombstone entry representing a deletion with its timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TombstoneEntry<T: Clone + Copy + Eq> {
    /// The key that was deleted
    pub key: T,
    /// Timestamp when the key was deleted
    pub delete_ts: Timestamp,
}

/// Hot/Cold tiered tombstone manager for efficient deletion tracking
///
/// Hot layer: frequently accessed, recent deletions (HashMap)
/// Cold layer: less frequently accessed, older deletions (sorted Vec with binary search)
///
/// This design balances O(1) insertion (hot) with O(log n) lookup (cold)
/// while maintaining good cache locality for cold queries.
#[derive(Debug, Clone)]
pub struct TieredTombstoneManager<T: Clone + Copy + Eq + std::hash::Hash> {
    /// Hot layer: HashMap for recent/frequent deletions (O(1) access)
    hot_tombstones: HashMap<T, Timestamp>,

    /// Cold layer: sorted Vec for older deletions (O(log n) binary search)
    cold_tombstones: Vec<TombstoneEntry<T>>,

    /// Maximum size of hot layer before promotion to cold
    hot_max_size: usize,

    /// Threshold for triggering hot→cold demotion (hot_max_size * 1.5)
    hot_gc_threshold: usize,
}

impl<T: Clone + Copy + Eq + std::hash::Hash + Ord> TieredTombstoneManager<T> {
    /// Create a new tiered tombstone manager
    ///
    /// # Arguments
    /// * `hot_max_size` - maximum capacity before promoting entries to cold layer
    pub fn new(hot_max_size: usize) -> Self {
        let hot_gc_threshold = hot_max_size.saturating_mul(3).saturating_div(2);
        Self {
            hot_tombstones: HashMap::new(),
            cold_tombstones: Vec::new(),
            hot_max_size,
            hot_gc_threshold,
        }
    }

    /// Check if a key is tombstoned (deleted) at the given timestamp
    ///
    /// Returns true if the deletion timestamp <= query timestamp.
    /// Checks hot layer first (O(1)), then cold layer (O(log n)).
    #[inline]
    pub fn is_tombstoned(&self, key: T, ts: Timestamp) -> bool {
        // Hot layer lookup (O(1))
        if let Some(&delete_ts) = self.hot_tombstones.get(&key) {
            return delete_ts <= ts;
        }

        // Cold layer lookup (O(log n) binary search)
        self.is_tombstoned_cold(key, ts)
    }

    /// Binary search in cold layer for tombstone entry
    fn is_tombstoned_cold(&self, key: T, ts: Timestamp) -> bool {
        match self.cold_tombstones.binary_search_by_key(&key, |e| e.key) {
            Ok(idx) => self.cold_tombstones[idx].delete_ts <= ts,
            Err(_) => false,
        }
    }

    /// Add a tombstone entry (mark a key as deleted)
    pub fn add_tombstone(&mut self, key: T, delete_ts: Timestamp) {
        self.hot_tombstones.insert(key, delete_ts);

        // If hot layer exceeds threshold, promote some entries to cold
        if self.hot_tombstones.len() >= self.hot_gc_threshold {
            self.promote_to_cold();
        }
    }

    /// Promote oldest entries from hot layer to cold layer (maintaining sort order)
    fn promote_to_cold(&mut self) {
        let mut entries: Vec<_> = self
            .hot_tombstones
            .drain()
            .map(|(k, ts)| TombstoneEntry {
                key: k,
                delete_ts: ts,
            })
            .collect();

        // Sort by key to maintain order in cold layer
        entries.sort_by_key(|e| e.key);

        // Move approximately 30% of entries to cold
        let move_count = entries.len().saturating_mul(3).saturating_div(10);
        for _ in 0..move_count {
            if let Some(entry) = entries.pop() {
                self.cold_tombstones.push(entry);
            }
        }

        // Keep remaining entries in hot
        for entry in entries {
            self.hot_tombstones.insert(entry.key, entry.delete_ts);
        }

        // Ensure cold layer remains sorted
        self.cold_tombstones.sort_by_key(|e| e.key);
    }

    /// Perform garbage collection: remove tombstones older than min_ts
    ///
    /// Returns the count of entries removed.
    pub fn gc(&mut self, min_ts: Timestamp) -> usize {
        let before_hot = self.hot_tombstones.len();
        let before_cold = self.cold_tombstones.len();

        // Clean hot layer
        self.hot_tombstones.retain(|_, ts| *ts >= min_ts);

        // Clean cold layer (preserves sort order since we only retain newer entries)
        self.cold_tombstones.retain(|e| e.delete_ts >= min_ts);

        let after_hot = self.hot_tombstones.len();
        let after_cold = self.cold_tombstones.len();

        (before_hot - after_hot) + (before_cold - after_cold)
    }

    /// Get the total number of tombstones (hot + cold)
    #[inline]
    pub fn len(&self) -> usize {
        self.hot_tombstones.len() + self.cold_tombstones.len()
    }

    /// Check if the manager is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.hot_tombstones.is_empty() && self.cold_tombstones.is_empty()
    }

    /// Get hot layer size
    #[inline]
    pub fn hot_len(&self) -> usize {
        self.hot_tombstones.len()
    }

    /// Get cold layer size
    #[inline]
    pub fn cold_len(&self) -> usize {
        self.cold_tombstones.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_handle_creation() {
        let handle = SnapshotHandle::new(100, 1);
        assert_eq!(handle.ts, 100);
        assert_eq!(handle.id, 1);
    }

    #[test]
    fn test_tiered_tombstone_manager_basic() {
        let mut mgr = TieredTombstoneManager::new(100);

        // Add a tombstone
        mgr.add_tombstone(1u32, 50);
        assert!(mgr.is_tombstoned(1u32, 50));
        assert!(mgr.is_tombstoned(1u32, 100));
        assert!(!mgr.is_tombstoned(1u32, 40));
    }

    #[test]
    fn test_tiered_tombstone_hot_layer() {
        let mut mgr = TieredTombstoneManager::new(10);

        // Fill hot layer
        for i in 0..5 {
            mgr.add_tombstone(i, 100);
        }

        assert_eq!(mgr.hot_len(), 5);
        assert_eq!(mgr.cold_len(), 0);

        // Verify all are in hot layer
        for i in 0..5 {
            assert!(mgr.is_tombstoned(i, 100));
        }
    }

    #[test]
    fn test_tiered_tombstone_promotion() {
        let mut mgr = TieredTombstoneManager::new(10);

        // Add enough entries to trigger promotion (threshold = 15)
        for i in 0..20 {
            mgr.add_tombstone(i, 100 + i as Timestamp);
        }

        // After promotion, should have both hot and cold
        assert!(mgr.hot_len() > 0);
        assert!(mgr.cold_len() > 0);
        assert_eq!(mgr.hot_len() + mgr.cold_len(), 20);

        // All entries should still be queryable
        for i in 0..20 {
            assert!(mgr.is_tombstoned(i, 100 + i as Timestamp + 1));
        }
    }

    #[test]
    fn test_tiered_tombstone_gc() {
        let mut mgr = TieredTombstoneManager::new(10);

        // Add tombstones at different times
        for i in 0..10 {
            mgr.add_tombstone(i, 50 + i as Timestamp);
        }

        // GC everything with delete_ts < 55 (entries 0-4)
        let removed = mgr.gc(55);
        assert_eq!(removed, 5);

        // Old entries should not be found
        assert!(!mgr.is_tombstoned(0u32, 100));
        assert!(!mgr.is_tombstoned(4u32, 100));

        // New entries should still be found
        assert!(mgr.is_tombstoned(5u32, 100));
        assert!(mgr.is_tombstoned(9u32, 100));
    }

    #[test]
    fn test_tiered_tombstone_binary_search() {
        let mut mgr = TieredTombstoneManager::new(5);

        // Add entries that will be promoted to cold
        for i in 0..10 {
            mgr.add_tombstone(i * 100, 100);
        }

        // All should be queryable via binary search
        for i in 0..10 {
            assert!(mgr.is_tombstoned(i * 100, 100));
        }

        // Non-existent keys should return false
        assert!(!mgr.is_tombstoned(50u32, 100));
        assert!(!mgr.is_tombstoned(150u32, 100));
    }

    #[test]
    fn test_tiered_tombstone_empty() {
        let mgr: TieredTombstoneManager<u32> = TieredTombstoneManager::new(10);
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn test_tiered_tombstone_len() {
        let mut mgr = TieredTombstoneManager::new(10);

        for i in 0..15 {
            mgr.add_tombstone(i, 100);
            assert_eq!(mgr.len(), (i + 1) as usize);
        }
    }
}

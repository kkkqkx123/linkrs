//! Unified MVCC (Multi-Version Concurrency Control) infrastructure
//!
//! Provides a consistent interface for snapshot isolation across all storage tables
//! (VertexTable, EdgeTable, PropertyTable). Implements a tiered tombstone management
//! system for efficient garbage collection.

use graphdb_core::types::storage_ids::Timestamp;
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

    /// Threshold for triggering hot→cold demotion (1.5x the configured
    /// hot-layer capacity)
    hot_gc_threshold: usize,
    cold_gc_cursor: usize,
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
            hot_gc_threshold,
            cold_gc_cursor: 0,
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

    /// Remove a tombstone entry (undo of [`Self::add_tombstone`]).
    ///
    /// Searches both the hot and cold layers; returns true if the key was
    /// present in either.
    pub fn remove(&mut self, key: T) -> bool {
        let hot_removed = self.hot_tombstones.remove(&key).is_some();
        let cold_removed = match self.cold_tombstones.binary_search_by_key(&key, |e| e.key) {
            Ok(idx) => {
                self.cold_tombstones.remove(idx);
                true
            }
            Err(_) => false,
        };
        hot_removed || cold_removed
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
    ///
    /// After a full GC pass the cold cursor is reset so that subsequent
    /// incremental `gc_batch` calls start from the beginning of the
    /// (now-compacted) cold layer.
    pub fn gc(&mut self, min_ts: Timestamp) -> usize {
        let before_hot = self.hot_tombstones.len();
        let before_cold = self.cold_tombstones.len();

        // Clean hot layer
        self.hot_tombstones.retain(|_, ts| *ts >= min_ts);

        // Clean cold layer (preserves sort order since we only retain newer entries)
        self.cold_tombstones.retain(|e| e.delete_ts >= min_ts);

        // Reset incremental GC cursor — full GC is complete, so the next
        // gc_batch should scan from the beginning.
        self.cold_gc_cursor = 0;

        let after_hot = self.hot_tombstones.len();
        let after_cold = self.cold_tombstones.len();

        (before_hot - after_hot) + (before_cold - after_cold)
    }

    pub fn gc_batch(&mut self, min_ts: Timestamp, batch_size: usize) -> usize {
        if batch_size == 0 || self.is_empty() {
            return 0;
        }
        let mut remaining = batch_size;
        let hot_keys: Vec<T> = self
            .hot_tombstones
            .iter()
            .filter_map(|(key, ts)| (*ts < min_ts).then_some(*key))
            .take(remaining)
            .collect();
        remaining -= hot_keys.len();
        let mut removed = 0;
        for key in hot_keys {
            removed += usize::from(self.hot_tombstones.remove(&key).is_some());
        }
        let cold_budget = remaining.min(self.cold_tombstones.len());
        for _ in 0..cold_budget {
            if self.cold_tombstones.is_empty() {
                break;
            }
            if self.cold_gc_cursor >= self.cold_tombstones.len() {
                self.cold_gc_cursor = 0;
            }
            if self.cold_tombstones[self.cold_gc_cursor].delete_ts < min_ts {
                self.cold_tombstones.remove(self.cold_gc_cursor);
                removed += 1;
            } else {
                self.cold_gc_cursor += 1;
            }
        }
        removed
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
    fn test_gc_batch_bounds_work_and_eventually_completes() {
        let mut manager = TieredTombstoneManager::new(4);
        for key in 0..100u32 {
            manager.add_tombstone(key, 10);
        }

        let mut total_removed = 0;
        while !manager.is_empty() {
            let removed = manager.gc_batch(20, 7);
            assert!(removed <= 7);
            assert!(removed > 0);
            total_removed += removed;
        }
        assert_eq!(total_removed, 100);
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

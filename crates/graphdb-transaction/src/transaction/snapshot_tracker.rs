//! Explicit Snapshot Tracking
//!
//! Provides O(1) operations for querying the minimum active snapshot.
//! Enables efficient tombstone garbage collection.
//!
//! ## Performance
//!
//! Uses BTreeMap for ordered snapshot tracking to achieve O(1) min queries
//! instead of O(n) full scans. This is critical for high-concurrency scenarios
//! with hundreds of concurrent reads.

use std::sync::atomic::{AtomicU64, Ordering};
use std::collections::BTreeMap;

use dashmap::DashMap;
use parking_lot::Mutex;

use crate::core::types::Timestamp;
use crate::core::error::StorageError;
use crate::core::error::storage::StorageErrorKind;

/// Tracks all active snapshots and their reference counts
///
/// This structure maintains:
/// - A mapping of active snapshot timestamps to reference counts (for concurrent access)
/// - An ordered BTreeMap for O(1) minimum snapshot queries
/// - Thread-safe concurrent access
///
/// ## Example
///
/// ```ignore
/// let tracker = SnapshotTracker::new();
///
/// // Create a snapshot
/// tracker.add_snapshot(100)?;
///
/// // Query minimum active snapshot
/// let min = tracker.min_active_snapshot(); // O(1)
///
/// // Release snapshot
/// tracker.release_snapshot(100)?;
/// ```
pub struct SnapshotTracker {
    /// Timestamp → reference count mapping (for concurrent updates)
    snapshots: DashMap<u64, AtomicU64>,

    /// Ordered snapshots for O(1) min queries
    /// Only contains snapshots with ref_count > 0
    ordered_snapshots: Mutex<BTreeMap<u64, u32>>,

    /// Minimum active snapshot (cached for O(1) queries)
    min_active: AtomicU64,
}

impl SnapshotTracker {
    /// Create a new snapshot tracker
    pub fn new() -> Self {
        Self {
            snapshots: DashMap::new(),
            ordered_snapshots: Mutex::new(BTreeMap::new()),
            min_active: AtomicU64::new(u64::MAX),
        }
    }

    /// Add a new snapshot, incrementing its reference count
    pub fn add_snapshot(&self, ts: Timestamp) -> Result<(), StorageError> {
        let ts = ts as u64;

        // Increment reference count or create new entry
        match self.snapshots.get(&ts) {
            Some(count) => {
                let new_count = count.fetch_add(1, Ordering::SeqCst) + 1;
                log::trace!("Snapshot {} ref count: {} -> {}", ts, new_count - 1, new_count);
            }
            None => {
                self.snapshots.insert(ts, AtomicU64::new(1));
                log::trace!("Snapshot {} added with ref count 1", ts);

                // Add to ordered map and update min_active
                {
                    let mut ordered = self.ordered_snapshots.lock();
                    ordered.insert(ts, 1);
                }
                self.update_min_active_from_tree();
            }
        }

        Ok(())
    }

    /// Release a snapshot, decrementing its reference count
    ///
    /// When reference count reaches zero, the snapshot is removed
    pub fn release_snapshot(&self, ts: Timestamp) -> Result<(), StorageError> {
        let ts = ts as u64;

        match self.snapshots.get(&ts) {
            Some(count) => {
                let new_count = count.fetch_sub(1, Ordering::SeqCst) - 1;
                log::trace!("Snapshot {} ref count: {} -> {}", ts, new_count + 1, new_count);

                if new_count == 0 {
                    drop(count); // Release the entry guard
                    self.snapshots.remove(&ts);
                    log::trace!("Snapshot {} removed (ref count = 0)", ts);

                    // Remove from ordered map and update min_active
                    {
                        let mut ordered = self.ordered_snapshots.lock();
                        ordered.remove(&ts);
                    }
                    self.update_min_active_from_tree();
                }
                Ok(())
            }
            None => {
                Err(StorageError::new(
                    StorageErrorKind::InvalidInput,
                    format!("Snapshot {} not found", ts),
                ))
            }
        }
    }

    /// Get the minimum active snapshot timestamp (O(1) with caching)
    pub fn min_active_snapshot(&self) -> Timestamp {
        let min = self.min_active.load(Ordering::Acquire);
        if min == u64::MAX {
            u32::MAX  // No active snapshots
        } else {
            min as u32
        }
    }

    /// Get the cleanup threshold based on minimum active snapshot
    ///
    /// Returns the minimum active snapshot timestamp.
    /// All versions with timestamp < this value can be safely cleaned.
    pub fn cleanup_threshold(&self) -> Timestamp {
        self.min_active_snapshot()
    }

    /// Internal: Recalculate and update min_active cache from BTreeMap (O(1))
    fn update_min_active_from_tree(&self) {
        let ordered = self.ordered_snapshots.lock();
        let min = if let Some((&ts, _)) = ordered.iter().next() {
            ts
        } else {
            u64::MAX
        };
        self.min_active.store(min, Ordering::Release);
    }

    /// Get the reference count for a specific snapshot (for testing)
    pub fn ref_count(&self, ts: Timestamp) -> Option<u64> {
        let ts = ts as u64;
        self.snapshots.get(&ts).map(|count| count.load(Ordering::SeqCst))
    }

    /// Get the total number of active snapshots (for testing)
    pub fn active_count(&self) -> usize {
        self.snapshots.len()
    }
}

impl Default for SnapshotTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_query_snapshot() {
        let tracker = SnapshotTracker::new();

        assert!(tracker.add_snapshot(100).is_ok());
        assert_eq!(tracker.min_active_snapshot(), 100);
        assert_eq!(tracker.cleanup_threshold(), 100);
    }

    #[test]
    fn test_multiple_snapshots() {
        let tracker = SnapshotTracker::new();

        assert!(tracker.add_snapshot(50).is_ok());
        assert!(tracker.add_snapshot(100).is_ok());
        assert!(tracker.add_snapshot(150).is_ok());

        assert_eq!(tracker.min_active_snapshot(), 50);
    }

    #[test]
    fn test_release_snapshot() {
        let tracker = SnapshotTracker::new();

        assert!(tracker.add_snapshot(100).is_ok());
        assert_eq!(tracker.min_active_snapshot(), 100);

        assert!(tracker.release_snapshot(100).is_ok());
        assert_eq!(tracker.min_active_snapshot(), u32::MAX);
    }

    #[test]
    fn test_ref_count() {
        let tracker = SnapshotTracker::new();

        assert!(tracker.add_snapshot(100).is_ok());
        assert!(tracker.add_snapshot(100).is_ok());

        assert!(tracker.release_snapshot(100).is_ok());
        assert_eq!(tracker.min_active_snapshot(), 100);

        assert!(tracker.release_snapshot(100).is_ok());
        assert_eq!(tracker.min_active_snapshot(), u32::MAX);
    }

    #[test]
    fn test_cleanup_threshold() {
        let tracker = SnapshotTracker::new();

        // Add snapshots
        tracker.add_snapshot(50).unwrap();
        tracker.add_snapshot(100).unwrap();
        tracker.add_snapshot(150).unwrap();

        // Cleanup threshold should be the minimum active snapshot
        assert_eq!(tracker.cleanup_threshold(), 50);

        // Release the minimum
        tracker.release_snapshot(50).unwrap();

        // Now minimum should be 100
        assert_eq!(tracker.cleanup_threshold(), 100);

        // Release 100
        tracker.release_snapshot(100).unwrap();

        // Now we can cleanup before 150
        assert_eq!(tracker.cleanup_threshold(), 150);

        // Release last one
        tracker.release_snapshot(150).unwrap();
        assert_eq!(tracker.cleanup_threshold(), u32::MAX);
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let tracker = Arc::new(SnapshotTracker::new());

        let mut handles = vec![];

        // Spawn multiple threads adding snapshots
        for i in 0..10 {
            let tracker_clone = Arc::clone(&tracker);
            let handle = thread::spawn(move || {
                for ts in (i * 100)..(i * 100 + 50) {
                    let _ = tracker_clone.add_snapshot(ts as u32);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            let _ = handle.join();
        }

        assert!(tracker.min_active_snapshot() <= 50);
    }

    #[test]
    fn test_release_nonexistent_snapshot() {
        let tracker = SnapshotTracker::new();
        assert!(tracker.release_snapshot(100).is_err());
    }

    #[test]
    fn test_ordered_snapshots_consistency() {
        let tracker = SnapshotTracker::new();

        // Add snapshots in non-sorted order
        tracker.add_snapshot(300).unwrap();
        tracker.add_snapshot(100).unwrap();
        tracker.add_snapshot(200).unwrap();

        // Min should still be correct
        assert_eq!(tracker.min_active_snapshot(), 100);

        // Release out of order
        tracker.release_snapshot(200).unwrap();
        assert_eq!(tracker.min_active_snapshot(), 100);

        tracker.release_snapshot(100).unwrap();
        assert_eq!(tracker.min_active_snapshot(), 300);

        tracker.release_snapshot(300).unwrap();
        assert_eq!(tracker.min_active_snapshot(), u32::MAX);
    }

    #[test]
    fn test_high_concurrency_stress() {
        use std::sync::Arc;
        use std::thread;
        use std::sync::atomic::AtomicUsize;

        let tracker = Arc::new(SnapshotTracker::new());
        let mut handles = vec![];
        let error_count = Arc::new(AtomicUsize::new(0));

        // 100 concurrent threads doing add/release cycles
        for tid in 0..100 {
            let t = Arc::clone(&tracker);
            let errors = Arc::clone(&error_count);
            let handle = thread::spawn(move || {
                for cycle in 0..100 {
                    let ts = (tid * 1000 + cycle) as u32;
                    if t.add_snapshot(ts).is_err() {
                        errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }

                for cycle in 0..100 {
                    let ts = (tid * 1000 + cycle) as u32;
                    if t.release_snapshot(ts).is_err() {
                        errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // No errors should occur
        assert_eq!(error_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(tracker.min_active_snapshot(), u32::MAX);
    }
}

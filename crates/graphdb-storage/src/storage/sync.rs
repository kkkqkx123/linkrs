//! Thread-safe MVCC table wrappers and snapshot management
//!
//! Provides RAII guards for automatic snapshot lifecycle management and
//! thread-safe wrappers for storage tables using RwLock for concurrent access.

use crate::core::error::StorageResult;
use crate::core::types::storage_ids::Timestamp;
use std::sync::{Arc, RwLock};

use super::mvcc::{MVCCTable, SnapshotHandle};

/// Thread-safe wrapper for VertexTable using Arc<RwLock<T>>
pub type VertexTableSync<T> = Arc<RwLock<T>>;

/// Thread-safe wrapper for EdgeTable using Arc<RwLock<T>>
pub type EdgeTableSync<T> = Arc<RwLock<T>>;

/// Thread-safe wrapper for PropertyTable using Arc<RwLock<T>>
pub type PropertyTableSync<T> = Arc<RwLock<T>>;

/// RAII guard for automatic snapshot lifecycle management
///
/// Automatically registers a snapshot on creation and unregisters it on drop.
/// This ensures that snapshots are properly cleaned up even in the presence of panics.
///
/// # Example
/// ```ignore
/// let table = Arc::new(RwLock::new(vertex_table));
/// {
///     let snapshot = SnapshotGuard::new(table.clone(), timestamp)?;
///     let result = snapshot.query(|t| t.get(id));
///     // snapshot automatically unregistered here
/// }
/// ```
pub struct SnapshotGuard<T: MVCCTable> {
    table: Arc<RwLock<T>>,
    handle: SnapshotHandle,
}

impl<T: MVCCTable> SnapshotGuard<T> {
    /// Create a new snapshot guard by registering with the table
    ///
    /// # Arguments
    /// * `table` - the MVCC-enabled table wrapped in Arc<RwLock<>>
    /// * `ts` - the timestamp for the snapshot
    ///
    /// # Returns
    /// StorageResult containing the guard, or error if registration fails
    pub fn new(table: Arc<RwLock<T>>, ts: Timestamp) -> StorageResult<Self> {
        let handle = {
            let mut guard = table.write().expect("RwLock poisoning");
            guard.register_snapshot(ts)?
        };
        Ok(Self { table, handle })
    }

    /// Execute a query against the snapshot
    ///
    /// Acquires a read lock and applies the given function to the table.
    ///
    /// # Arguments
    /// * `f` - a function that takes a reference to the table and returns a result
    ///
    /// # Returns
    /// The result of the function
    #[inline]
    pub fn query<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.table.read().expect("RwLock poisoning");
        f(&guard)
    }

    /// Get the snapshot handle (for manual reference tracking if needed)
    #[inline]
    pub fn handle(&self) -> SnapshotHandle {
        self.handle
    }
}

impl<T: MVCCTable> Drop for SnapshotGuard<T> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.table.write() {
            let _ = guard.unregister_snapshot(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // Mock MVCC table for testing
    struct MockMVCCTable {
        active_snapshots: HashMap<Timestamp, usize>,
        min_active_snapshot_ts: Timestamp,
        handle_counter: u64,
    }

    impl MockMVCCTable {
        fn new() -> Self {
            Self {
                active_snapshots: HashMap::new(),
                min_active_snapshot_ts: u32::MAX,
                handle_counter: 0,
            }
        }
    }

    impl MVCCTable for MockMVCCTable {
        fn register_snapshot(&mut self, ts: Timestamp) -> StorageResult<SnapshotHandle> {
            *self.active_snapshots.entry(ts).or_insert(0) += 1;
            self.min_active_snapshot_ts = self
                .active_snapshots
                .keys()
                .min()
                .copied()
                .unwrap_or(u32::MAX);

            self.handle_counter += 1;
            Ok(SnapshotHandle::new(ts, self.handle_counter))
        }

        fn unregister_snapshot(&mut self, handle: SnapshotHandle) -> StorageResult<()> {
            if let Some(count) = self.active_snapshots.get_mut(&handle.ts) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.active_snapshots.remove(&handle.ts);
                }
            }
            Ok(())
        }

        fn active_snapshot_count(&self) -> usize {
            self.active_snapshots.len()
        }

        fn min_active_snapshot_ts(&self) -> Timestamp {
            self.min_active_snapshot_ts
        }

        fn gc(&mut self, _min_ts: Timestamp) -> StorageResult<usize> {
            Ok(0)
        }
    }

    #[test]
    fn test_snapshot_guard_creation() {
        let table = Arc::new(RwLock::new(MockMVCCTable::new()));
        let guard = SnapshotGuard::new(table.clone(), 100);
        assert!(guard.is_ok());
    }

    #[test]
    fn test_snapshot_guard_registers_snapshot() {
        let table = Arc::new(RwLock::new(MockMVCCTable::new()));
        {
            let guard = SnapshotGuard::new(table.clone(), 100).expect("failed to create guard");
            let count = table.read().unwrap().active_snapshot_count();
            assert_eq!(count, 1);
        }
        // After guard drops, snapshot should be unregistered
        let count = table.read().unwrap().active_snapshot_count();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_snapshot_guard_multiple_snapshots() {
        let table = Arc::new(RwLock::new(MockMVCCTable::new()));
        {
            let _guard1 = SnapshotGuard::new(table.clone(), 100).expect("failed to create guard");
            let _guard2 = SnapshotGuard::new(table.clone(), 200).expect("failed to create guard");

            let count = table.read().unwrap().active_snapshot_count();
            assert_eq!(count, 2);

            let min_ts = table.read().unwrap().min_active_snapshot_ts();
            assert_eq!(min_ts, 100);
        }
        let count = table.read().unwrap().active_snapshot_count();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_snapshot_guard_query() {
        let table = Arc::new(RwLock::new(MockMVCCTable::new()));
        let guard = SnapshotGuard::new(table.clone(), 100).expect("failed to create guard");

        let result = guard.query(|t| t.active_snapshot_count());
        assert_eq!(result, 1);
    }

    #[test]
    fn test_snapshot_guard_handle() {
        let table = Arc::new(RwLock::new(MockMVCCTable::new()));
        let guard = SnapshotGuard::new(table.clone(), 100).expect("failed to create guard");
        let handle = guard.handle();

        assert_eq!(handle.ts, 100);
        assert_eq!(handle.id, 1); // First handle
    }

    #[test]
    fn test_snapshot_guard_cleanup_on_drop() {
        let table = Arc::new(RwLock::new(MockMVCCTable::new()));

        // Create and immediately drop a snapshot
        {
            let _guard = SnapshotGuard::new(table.clone(), 100).expect("failed to create guard");
            assert_eq!(table.read().unwrap().active_snapshot_count(), 1);
        }

        // After drop, snapshot should be cleaned up
        assert_eq!(table.read().unwrap().active_snapshot_count(), 0);
    }

    #[test]
    fn test_multiple_snapshots_same_timestamp() {
        let table = Arc::new(RwLock::new(MockMVCCTable::new()));

        {
            let _guard1 = SnapshotGuard::new(table.clone(), 100).expect("failed to create guard");
            let _guard2 = SnapshotGuard::new(table.clone(), 100).expect("failed to create guard");

            // Should have 1 timestamp with count 2
            let ts = table.read().unwrap().active_snapshot_count();
            assert_eq!(ts, 1); // One timestamp tracked

            // But min_active_snapshot_ts should still be 100
            let min_ts = table.read().unwrap().min_active_snapshot_ts();
            assert_eq!(min_ts, 100);
        }

        let count = table.read().unwrap().active_snapshot_count();
        assert_eq!(count, 0);
    }
}

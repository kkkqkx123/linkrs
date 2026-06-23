//! Integration tests for MVCC and Sync modules

use graphdb_storage::storage::mvcc::{MVCCTable, SnapshotHandle, TieredTombstoneManager};
use graphdb_storage::storage::sync::SnapshotGuard;
use graphdb_storage::core::StorageResult;
use graphdb_core::core::types::storage_ids::Timestamp;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

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
fn test_tiered_tombstone_manager_creation() {
    let mgr: TieredTombstoneManager<u32> = TieredTombstoneManager::new(100);
    assert!(mgr.is_empty());
}

#[test]
fn test_tiered_tombstone_basic_operations() {
    let mut mgr = TieredTombstoneManager::new(100);

    // Add a tombstone
    mgr.add_tombstone(1u32, 50);
    assert!(mgr.is_tombstoned(1u32, 50));
    assert!(mgr.is_tombstoned(1u32, 100));
    assert!(!mgr.is_tombstoned(1u32, 40));
    assert_eq!(mgr.len(), 1);
}

#[test]
fn test_tiered_tombstone_hot_and_cold_layers() {
    let mut mgr = TieredTombstoneManager::new(10);

    // Add entries to trigger promotion
    for i in 0..20 {
        mgr.add_tombstone(i, 100 + i as Timestamp);
    }

    // Should have both hot and cold layers
    assert!(mgr.hot_len() > 0);
    assert!(mgr.cold_len() > 0);
    assert_eq!(mgr.hot_len() + mgr.cold_len(), 20);
}

#[test]
fn test_tiered_tombstone_gc() {
    let mut mgr = TieredTombstoneManager::new(10);

    for i in 0..10 {
        mgr.add_tombstone(i, 50 + i as Timestamp);
    }

    // GC entries with delete_ts < 55
    let removed = mgr.gc(55);
    assert_eq!(removed, 5);

    // Old entries should be gone
    assert!(!mgr.is_tombstoned(0u32, 100));
    assert!(!mgr.is_tombstoned(4u32, 100));

    // New entries should remain
    assert!(mgr.is_tombstoned(5u32, 100));
    assert!(mgr.is_tombstoned(9u32, 100));
}

#[test]
fn test_snapshot_handle_creation() {
    let handle = SnapshotHandle::new(100, 1);
    assert_eq!(handle.ts, 100);
    assert_eq!(handle.id, 1);

    let handle2 = SnapshotHandle::new(100, 2);
    assert_ne!(handle, handle2); // Different IDs
}

#[test]
fn test_snapshot_guard_lifecycle() {
    let table = Arc::new(RwLock::new(MockMVCCTable::new()));

    {
        let _guard = SnapshotGuard::new(table.clone(), 100)
            .expect("failed to create guard");

        // Snapshot should be registered
        let count = table.read().unwrap().active_snapshot_count();
        assert_eq!(count, 1);
    }

    // After guard drops, snapshot should be unregistered
    let count = table.read().unwrap().active_snapshot_count();
    assert_eq!(count, 0);
}

#[test]
fn test_multiple_concurrent_snapshots() {
    let table = Arc::new(RwLock::new(MockMVCCTable::new()));

    {
        let _guard1 = SnapshotGuard::new(table.clone(), 100)
            .expect("failed to create guard 1");
        let _guard2 = SnapshotGuard::new(table.clone(), 200)
            .expect("failed to create guard 2");
        let _guard3 = SnapshotGuard::new(table.clone(), 150)
            .expect("failed to create guard 3");

        let count = table.read().unwrap().active_snapshot_count();
        assert_eq!(count, 3);

        let min_ts = table.read().unwrap().min_active_snapshot_ts();
        assert_eq!(min_ts, 100);
    }

    let count = table.read().unwrap().active_snapshot_count();
    assert_eq!(count, 0);
}

#[test]
fn test_snapshot_guard_query() {
    let table = Arc::new(RwLock::new(MockMVCCTable::new()));
    let guard = SnapshotGuard::new(table.clone(), 100)
        .expect("failed to create guard");

    let result = guard.query(|t| t.active_snapshot_count());
    assert_eq!(result, 1);

    let min_ts = guard.query(|t| t.min_active_snapshot_ts());
    assert_eq!(min_ts, 100);
}

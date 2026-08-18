//! Storage integration tests: slot allocation, compaction, WAL-backed crash
//! recovery.

use std::collections::HashMap;

use vector_search::storage::CollectionStore;
use vector_search::storage::{Wal, WalRecord, WalTxn};
use vector_search::types::{CollectionConfig, DistanceMetric, PointId, VectorPoint};

fn config(dim: usize) -> CollectionConfig {
    CollectionConfig::new(dim, DistanceMetric::Cosine)
}

fn point(id: u64, dim: usize) -> VectorPoint {
    VectorPoint::new(
        id,
        (0..dim)
            .map(|i| ((id as usize * 31 + i) % 100) as f32 / 100.0)
            .collect(),
    )
}

#[test]
fn test_apply_txn_roundtrip_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(4)).unwrap();

    store
        .apply_txn(&WalTxn {
            txn_id: 1,
            ops: vec![
                WalRecord::Upsert {
                    point: vector_search::storage::WalPoint::from_point(&point(1, 4)).unwrap(),
                },
                WalRecord::Upsert {
                    point: vector_search::storage::WalPoint::from_point(&point(2, 4)).unwrap(),
                },
            ],
        })
        .unwrap();
    store
        .apply_txn(&WalTxn {
            txn_id: 2,
            ops: vec![WalRecord::Delete {
                point_id: "1".to_string(),
            }],
        })
        .unwrap();

    assert_eq!(store.count(), 1);
    assert_eq!(store.meta().last_applied_txn, 2);
    assert!(store.get(&PointId::Num(1)).unwrap().is_none());
    assert!(store.get(&PointId::Num(2)).unwrap().is_some());

    drop(store);
    let reopened = CollectionStore::open(&store_dir).unwrap();
    assert_eq!(reopened.count(), 1);
    assert!(reopened.get(&PointId::Num(1)).unwrap().is_none());
    assert_eq!(
        reopened.get(&PointId::Num(2)).unwrap().unwrap().vector,
        point(2, 4).vector
    );
    assert_eq!(reopened.meta().last_applied_txn, 2);
}

#[test]
fn test_wal_written_memory_not_applied_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(4)).unwrap();
    store.upsert(&point(1, 4)).unwrap();
    drop(store);

    // Simulate a crash between "WAL fsync" and "apply to memory": append
    // transactions directly to wal.bin, bypassing the in-memory apply.
    let wal = Wal::open_or_create(&store_dir.join("wal.bin")).unwrap();
    wal.append(&WalTxn {
        txn_id: 1,
        ops: vec![WalRecord::Upsert {
            point: vector_search::storage::WalPoint::from_point(&point(2, 4)).unwrap(),
        }],
    })
    .unwrap();
    wal.append(&WalTxn {
        txn_id: 2,
        ops: vec![WalRecord::Delete {
            point_id: "1".to_string(),
        }],
    })
    .unwrap();
    drop(wal);

    let reopened = CollectionStore::open(&store_dir).unwrap();
    assert_eq!(reopened.count(), 1, "replay must apply the pending WAL");
    assert!(reopened.get(&PointId::Num(1)).unwrap().is_none());
    assert!(reopened.get(&PointId::Num(2)).unwrap().is_some());
}

#[test]
fn test_replay_is_idempotent_for_duplicate_txns() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(4)).unwrap();

    let txn = WalTxn {
        txn_id: 1,
        ops: vec![WalRecord::Upsert {
            point: vector_search::storage::WalPoint::from_point(&point(1, 4)).unwrap(),
        }],
    };
    // Same txn applied twice (crash + coordinator retry semantics).
    store.apply_txn(&txn).unwrap();
    store.apply_txn(&txn).unwrap();
    assert_eq!(store.count(), 1, "re-applying an upsert must not duplicate");

    let del = WalTxn {
        txn_id: 2,
        ops: vec![WalRecord::Delete {
            point_id: "1".to_string(),
        }],
    };
    store.apply_txn(&del).unwrap();
    store.apply_txn(&del).unwrap();
    assert_eq!(store.count(), 0, "re-applying a delete is a no-op");
}

#[test]
fn test_apply_txn_rejects_invalid_before_wal_append() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(4)).unwrap();

    let bad = VectorPoint::new(1u64, vec![1.0, 2.0]); // wrong dim
    let err = store
        .apply_txn(&WalTxn {
            txn_id: 1,
            ops: vec![WalRecord::Upsert {
                point: vector_search::storage::WalPoint::from_point(&bad).unwrap(),
            }],
        })
        .unwrap_err();
    assert!(matches!(
        err,
        vector_search::VectorSearchError::InvalidVectorDimension {
            expected: 4,
            actual: 2
        }
    ));
    // Nothing was logged or applied.
    assert_eq!(store.count(), 0);
    let wal = Wal::open_or_create(&store_dir.join("wal.bin")).unwrap();
    let last = wal.replay(|_| Ok(())).unwrap();
    assert_eq!(last, 0);
}

#[test]
fn test_compact_rebuilds_and_search_stays_correct() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(4)).unwrap();

    for i in 0..20u64 {
        store.upsert(&point(i, 4)).unwrap();
    }
    // Delete some points (below the 20% threshold: 4/20 = 20%, threshold is
    // strictly greater, so no auto-compaction).
    for i in [3u64, 7, 11, 15] {
        assert!(store.delete(&PointId::Num(i)).unwrap());
    }
    assert_eq!(store.count(), 16);
    let meta = store.meta();
    assert_eq!(meta.tombstone_count, 4);

    let live = store.compact().unwrap();
    assert_eq!(live, 16);
    let meta = store.meta();
    assert_eq!(meta.next_slot, 16);
    assert_eq!(meta.tombstone_count, 0);

    // All surviving points are readable with correct vectors.
    for i in 0..20u64 {
        let got = store.get(&PointId::Num(i)).unwrap();
        if [3, 7, 11, 15].contains(&i) {
            assert!(got.is_none(), "deleted point {i} must stay gone");
        } else {
            assert_eq!(got.unwrap().vector, point(i, 4).vector, "point {i}");
        }
    }

    // Reopen: compacted layout must be consistent on disk.
    drop(store);
    let reopened = CollectionStore::open(&store_dir).unwrap();
    assert_eq!(reopened.count(), 16);
    for i in 0..20u64 {
        let got = reopened.get(&PointId::Num(i)).unwrap();
        if [3, 7, 11, 15].contains(&i) {
            assert!(got.is_none());
        } else {
            assert_eq!(got.unwrap().vector, point(i, 4).vector);
        }
    }
}

#[test]
fn test_compact_with_payloads_and_no_auto_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(2)).unwrap();

    let mut payload = HashMap::new();
    payload.insert("color".to_string(), serde_json::json!("red"));
    let mut p = point(1, 2);
    p.payload = Some(payload.clone());
    store.upsert(&p).unwrap();
    let mut p2 = point(2, 2);
    let mut payload2 = HashMap::new();
    payload2.insert("color".to_string(), serde_json::json!("blue"));
    p2.payload = Some(payload2.clone());
    store.upsert(&p2).unwrap();

    assert!(store.delete(&PointId::Num(1)).unwrap());
    // 1/2 > 20% -> auto-compacted
    let meta = store.meta();
    assert_eq!(meta.tombstone_count, 0);

    let got = store.get(&PointId::Num(2)).unwrap().unwrap();
    assert_eq!(got.payload, Some(payload2.clone()));

    drop(store);
    let reopened = CollectionStore::open(&store_dir).unwrap();
    let got = reopened.get(&PointId::Num(2)).unwrap().unwrap();
    assert_eq!(got.payload, Some(payload2));
    assert!(reopened.get(&PointId::Num(1)).unwrap().is_none());
}

#[test]
fn test_compact_checkpoint_truncates_wal() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(4)).unwrap();
    for i in 0..6u64 {
        store.upsert(&point(i, 4)).unwrap();
    }
    for i in 0..2u64 {
        assert!(store.delete(&PointId::Num(i)).unwrap());
    }
    // 2/6 > 20%: auto-compacted on the second delete; WAL must be truncated.
    let wal = Wal::open_or_create(&store_dir.join("wal.bin")).unwrap();
    let last = wal.replay(|_| Ok(())).unwrap();
    assert_eq!(last, 0, "WAL must be empty after compaction");
    assert_eq!(store.count(), 4);
}

#[test]
fn test_grow_after_compact() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");
    let store = CollectionStore::create(&store_dir, "col", &config(4)).unwrap();
    for i in 0..100u64 {
        store.upsert(&point(i, 4)).unwrap();
    }
    for i in 0..50u64 {
        assert!(store.delete(&PointId::Num(i)).unwrap());
    }
    assert_eq!(store.count(), 50);

    // Continue inserting well past the compacted capacity.
    for i in 100..500u64 {
        store.upsert(&point(i, 4)).unwrap();
    }
    assert_eq!(store.count(), 450);
    for i in 100..500u64 {
        assert_eq!(
            store.get(&PointId::Num(i)).unwrap().unwrap().vector,
            point(i, 4).vector,
            "point {i}"
        );
    }
}

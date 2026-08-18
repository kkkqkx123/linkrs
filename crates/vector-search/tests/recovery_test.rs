//! WAL replay idempotency integration tests.

use vector_search::storage::CollectionStore;
use vector_search::storage::{Wal, WalPoint, WalRecord, WalTxn};
use vector_search::types::{CollectionConfig, DistanceMetric, PointId, VectorPoint};

fn config() -> CollectionConfig {
    CollectionConfig::new(4, DistanceMetric::Cosine)
}

fn point(id: u64) -> VectorPoint {
    VectorPoint::new(id, vec![id as f32, 0.0, 0.0, 0.0])
}

#[test]
fn test_reopen_replays_wal_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");

    {
        let store = CollectionStore::create(&store_dir, "col", &config()).unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 1,
                ops: vec![WalRecord::Upsert {
                    point: WalPoint::from_point(&point(1)).unwrap(),
                }],
            })
            .unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 2,
                ops: vec![WalRecord::Upsert {
                    point: WalPoint::from_point(&point(2)).unwrap(),
                }],
            })
            .unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 3,
                ops: vec![WalRecord::Upsert {
                    point: WalPoint::from_point(&point(3)).unwrap(),
                }],
            })
            .unwrap();
    }

    // Reopen N times: the WAL must replay idempotently (upserts overwrite by
    // id, never duplicate).
    for _ in 0..3 {
        let store = CollectionStore::open(&store_dir).unwrap();
        assert_eq!(store.count(), 3);
        assert_eq!(store.meta().last_applied_txn, 3);
    }
}

#[test]
fn test_replay_delete_batch_and_compact_marker() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");

    {
        let store = CollectionStore::create(&store_dir, "col", &config()).unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 1,
                ops: vec![
                    WalRecord::Upsert {
                        point: WalPoint::from_point(&point(1)).unwrap(),
                    },
                    WalRecord::Upsert {
                        point: WalPoint::from_point(&point(2)).unwrap(),
                    },
                    WalRecord::Upsert {
                        point: WalPoint::from_point(&point(3)).unwrap(),
                    },
                ],
            })
            .unwrap();
    }

    // Append a delete-batch + compact marker directly to the WAL (simulating a
    // crash after WAL fsync before memory apply).
    {
        let wal = Wal::open_or_create(&store_dir.join("wal.bin")).unwrap();
        wal.append(&WalTxn {
            txn_id: 2,
            ops: vec![WalRecord::DeleteBatch {
                point_ids: vec!["1".to_string(), "2".to_string()],
            }],
        })
        .unwrap();
        wal.append(&WalTxn {
            txn_id: 3,
            ops: vec![WalRecord::Compact],
        })
        .unwrap();
    }

    let store = CollectionStore::open(&store_dir).unwrap();
    assert_eq!(store.count(), 1, "batch delete must apply during replay");
    assert!(store.get(&PointId::Num(1)).unwrap().is_none());
    assert!(store.get(&PointId::Num(2)).unwrap().is_none());
    assert_eq!(
        store.get(&PointId::Num(3)).unwrap().unwrap().vector,
        point(3).vector
    );
    assert_eq!(store.meta().last_applied_txn, 3);
}

#[test]
fn test_partial_wal_record_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");

    {
        let store = CollectionStore::create(&store_dir, "col", &config()).unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 1,
                ops: vec![WalRecord::Upsert {
                    point: WalPoint::from_point(&point(1)).unwrap(),
                }],
            })
            .unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 2,
                ops: vec![WalRecord::Upsert {
                    point: WalPoint::from_point(&point(2)).unwrap(),
                }],
            })
            .unwrap();
    }

    // Crash mid-append: a length prefix claiming bytes that were never
    // written.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(store_dir.join("wal.bin"))
            .unwrap();
        f.write_all(&4096u32.to_le_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    let store = CollectionStore::open(&store_dir).unwrap();
    assert_eq!(store.count(), 2, "truncated tail record must be ignored");
    assert!(store.get(&PointId::Num(2)).unwrap().is_some());
}

#[test]
fn test_upsert_overwrite_after_replay_does_not_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");

    {
        let store = CollectionStore::create(&store_dir, "col", &config()).unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 1,
                ops: vec![WalRecord::Upsert {
                    point: WalPoint::from_point(&point(1)).unwrap(),
                }],
            })
            .unwrap();
    }

    // Same point id with a different vector, logged after a crash that lost
    // the memory apply of txn 2.
    {
        let wal = Wal::open_or_create(&store_dir.join("wal.bin")).unwrap();
        let mut updated = point(1);
        updated.vector = vec![9.0, 9.0, 9.0, 9.0];
        wal.append(&WalTxn {
            txn_id: 2,
            ops: vec![WalRecord::Upsert {
                point: WalPoint::from_point(&updated).unwrap(),
            }],
        })
        .unwrap();
    }

    let store = CollectionStore::open(&store_dir).unwrap();
    assert_eq!(store.count(), 1);
    assert_eq!(
        store.get(&PointId::Num(1)).unwrap().unwrap().vector,
        vec![9.0, 9.0, 9.0, 9.0],
        "replay overwrite must win"
    );
}

#[test]
fn test_replay_reconciles_stale_meta_counts() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("col");

    {
        let store = CollectionStore::create(&store_dir, "col", &config()).unwrap();
        store
            .apply_txn(&WalTxn {
                txn_id: 1,
                ops: vec![WalRecord::Upsert {
                    point: WalPoint::from_point(&point(1)).unwrap(),
                }],
            })
            .unwrap();
        // Crash window: WAL written but meta.bin not yet persisted with the
        // live_count bump. Append directly to the WAL.
        let wal = Wal::open_or_create(&store_dir.join("wal.bin")).unwrap();
        wal.append(&WalTxn {
            txn_id: 2,
            ops: vec![WalRecord::Upsert {
                point: WalPoint::from_point(&point(2)).unwrap(),
            }],
        })
        .unwrap();
    }

    let store = CollectionStore::open(&store_dir).unwrap();
    assert_eq!(store.count(), 2, "live_count must be reconciled from slots");
    assert_eq!(store.meta().last_applied_txn, 2);
}

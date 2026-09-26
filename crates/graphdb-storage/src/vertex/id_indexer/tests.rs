use std::collections::HashMap;

use super::{
    config::{DEFAULT_GROWTH_FACTOR, MAX_CAPACITY},
    lookup::PkLookup,
    manager::IdManager,
    IdIndexer, IdIndexerConfig, IdKey, PK_DELTA_ANCHOR_THRESHOLD,
};

#[test]
fn test_basic_operations() {
    let indexer = IdIndexer::new();

    let idx1 = indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
    assert_eq!(idx1, 0);

    let idx2 = indexer.insert(IdKey::Text("vertex2".to_string())).unwrap();
    assert_eq!(idx2, 1);

    assert_eq!(
        indexer.get_index(&IdKey::Text("vertex1".to_string())),
        Some(0)
    );
    assert_eq!(
        indexer.get_index(&IdKey::Text("vertex2".to_string())),
        Some(1)
    );
    assert_eq!(indexer.get_index(&IdKey::Text("vertex3".to_string())), None);

    assert_eq!(indexer.get_key(0), Some(IdKey::Text("vertex1".to_string())));
    assert_eq!(indexer.get_key(1), Some(IdKey::Text("vertex2".to_string())));
}

#[test]
fn test_int_id_operations() {
    let indexer = IdIndexer::new();

    let idx1 = indexer.insert(IdKey::Int(100)).unwrap();
    assert_eq!(idx1, 0);

    let idx2 = indexer.insert(IdKey::Int(200)).unwrap();
    assert_eq!(idx2, 1);

    assert_eq!(indexer.get_index(&IdKey::Int(100)), Some(0));
    assert_eq!(indexer.get_index(&IdKey::Int(200)), Some(1));
    assert_eq!(indexer.get_index(&IdKey::Int(300)), None);

    assert_eq!(indexer.get_key(0), Some(IdKey::Int(100)));
    assert_eq!(indexer.get_key(1), Some(IdKey::Int(200)));
}

#[test]
fn test_mixed_id_operations() {
    let indexer = IdIndexer::new();

    let idx1 = indexer.insert(IdKey::Int(100)).unwrap();
    let idx2 = indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
    let idx3 = indexer.insert(IdKey::Int(200)).unwrap();

    assert_eq!(idx1, 0);
    assert_eq!(idx2, 1);
    assert_eq!(idx3, 2);

    assert_eq!(indexer.len(), 3);
}

#[test]
fn test_live_ids_skip_deleted_gaps_in_stable_order() {
    let indexer = IdIndexer::new();
    for value in 0..10 {
        indexer
            .insert(IdKey::Int(value))
            .expect("insert must succeed");
    }
    indexer.remove(&IdKey::Int(1));
    indexer.remove(&IdKey::Int(5));
    indexer.remove(&IdKey::Int(8));

    assert_eq!(indexer.live_ids(), vec![0, 2, 3, 4, 6, 7, 9]);
}

#[test]
fn test_dynamic_expansion() {
    let indexer = IdIndexer::with_config(IdIndexerConfig {
        initial_capacity: 2,
        growth_factor: 2.0,
        max_capacity: MAX_CAPACITY,
    });

    assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v2".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v3".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v4".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v5".to_string())).is_ok());

    assert_eq!(indexer.len(), 5);
}

#[test]
fn test_duplicate_insert() {
    let indexer = IdIndexer::new();

    assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v1".to_string())).is_err());
}

#[test]
fn test_max_capacity() {
    let indexer = IdIndexer::with_config(IdIndexerConfig {
        initial_capacity: 2,
        growth_factor: DEFAULT_GROWTH_FACTOR,
        max_capacity: 3,
    });

    assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v2".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v3".to_string())).is_ok());
    assert!(indexer.insert(IdKey::Text("v4".to_string())).is_err());
}

#[test]
fn test_concurrent_parallel_inserts() {
    use std::sync::Arc as StdArc;
    use std::thread;

    let indexer = StdArc::new(IdIndexer::new());
    let mut handles = vec![];

    for thread_id in 0..4 {
        let indexer_clone = StdArc::clone(&indexer);
        let handle = thread::spawn(move || {
            for i in 0..25 {
                let key = IdKey::Text(format!("v_{}_{}", thread_id, i));
                let _ = indexer_clone.insert(key);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    assert_eq!(indexer.len(), 100);
}

#[test]
fn test_concurrent_mixed_operations() {
    use std::sync::Arc as StdArc;
    use std::thread;

    let indexer = StdArc::new(IdIndexer::new());

    for i in 0..10 {
        let key = IdKey::Text(format!("v{}", i));
        let _ = indexer.insert(key);
    }

    let mut handles = vec![];

    for _ in 0..2 {
        let indexer_clone = StdArc::clone(&indexer);
        let handle = thread::spawn(move || {
            for i in 0..10 {
                let key = IdKey::Text(format!("v{}", i));
                let _ = indexer_clone.get_index(&key);
            }
        });
        handles.push(handle);
    }

    let indexer_clone = StdArc::clone(&indexer);
    let handle = thread::spawn(move || {
        for i in 10..20 {
            let key = IdKey::Text(format!("v{}", i));
            let _ = indexer_clone.insert(key);
        }
    });
    handles.push(handle);

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    assert_eq!(indexer.len(), 20);
}

#[test]
fn test_remove() {
    let indexer = IdIndexer::new();

    indexer.insert(IdKey::Text("v1".to_string())).unwrap();
    indexer.insert(IdKey::Text("v2".to_string())).unwrap();
    indexer.insert(IdKey::Text("v3".to_string())).unwrap();

    assert_eq!(indexer.len(), 3);

    indexer.remove(&IdKey::Text("v2".to_string()));
    assert_eq!(indexer.len(), 2);

    assert_eq!(indexer.get_index(&IdKey::Text("v2".to_string())), None);
}

#[test]
fn test_serialize_deserialize_string_ids() {
    let indexer = IdIndexer::new();

    indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
    indexer.insert(IdKey::Text("vertex2".to_string())).unwrap();
    indexer.insert(IdKey::Text("vertex3".to_string())).unwrap();

    let data = indexer.serialize();
    let deserialized = IdIndexer::deserialize(&data).unwrap();

    assert_eq!(deserialized.len(), 3);
    assert_eq!(
        deserialized.get_index(&IdKey::Text("vertex1".to_string())),
        Some(0)
    );
    assert_eq!(
        deserialized.get_index(&IdKey::Text("vertex2".to_string())),
        Some(1)
    );
    assert_eq!(
        deserialized.get_index(&IdKey::Text("vertex3".to_string())),
        Some(2)
    );
    assert_eq!(
        deserialized.get_key(0),
        Some(IdKey::Text("vertex1".to_string()))
    );
}

#[test]
fn test_serialize_deserialize_int_ids() {
    let indexer = IdIndexer::new();

    indexer.insert(IdKey::Int(100)).unwrap();
    indexer.insert(IdKey::Int(200)).unwrap();
    indexer.insert(IdKey::Int(300)).unwrap();

    let data = indexer.serialize();
    let deserialized = IdIndexer::deserialize(&data).unwrap();

    assert_eq!(deserialized.len(), 3);
    assert_eq!(deserialized.get_index(&IdKey::Int(100)), Some(0));
    assert_eq!(deserialized.get_index(&IdKey::Int(200)), Some(1));
    assert_eq!(deserialized.get_index(&IdKey::Int(300)), Some(2));
    assert_eq!(deserialized.get_key(0), Some(IdKey::Int(100)));
}

#[test]
fn test_serialize_deserialize_mixed_ids() {
    let indexer = IdIndexer::new();

    indexer.insert(IdKey::Int(100)).unwrap();
    indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
    indexer.insert(IdKey::Int(200)).unwrap();
    indexer.insert(IdKey::Text("vertex2".to_string())).unwrap();

    let data = indexer.serialize();
    let deserialized = IdIndexer::deserialize(&data).unwrap();

    assert_eq!(deserialized.len(), 4);
    assert_eq!(deserialized.get_index(&IdKey::Int(100)), Some(0));
    assert_eq!(
        deserialized.get_index(&IdKey::Text("vertex1".to_string())),
        Some(1)
    );
    assert_eq!(deserialized.get_index(&IdKey::Int(200)), Some(2));
    assert_eq!(
        deserialized.get_index(&IdKey::Text("vertex2".to_string())),
        Some(3)
    );
}

#[test]
fn test_serialize_deserialize_empty() {
    let indexer = IdIndexer::new();

    let data = indexer.serialize();
    let deserialized = IdIndexer::deserialize(&data).unwrap();

    assert_eq!(deserialized.len(), 0);
}

#[test]
fn test_serialize_deserialize_with_deletions() {
    let indexer = IdIndexer::new();

    indexer.insert(IdKey::Int(1)).unwrap();
    indexer.insert(IdKey::Int(2)).unwrap();
    indexer.insert(IdKey::Int(3)).unwrap();
    indexer.insert(IdKey::Int(4)).unwrap();
    indexer.insert(IdKey::Int(5)).unwrap();

    indexer.remove(&IdKey::Int(2));
    indexer.remove(&IdKey::Int(4));

    let data = indexer.serialize();
    let deserialized = IdIndexer::deserialize(&data).unwrap();

    assert_eq!(deserialized.len(), 3);
    assert_eq!(deserialized.get_index(&IdKey::Int(1)), Some(0));
    assert_eq!(deserialized.get_index(&IdKey::Int(2)), None);
    assert_eq!(deserialized.get_index(&IdKey::Int(3)), Some(2));
    assert_eq!(deserialized.get_index(&IdKey::Int(4)), None);
    assert_eq!(deserialized.get_index(&IdKey::Int(5)), Some(4));
}

#[test]
fn test_serialize_deserialize_large_dataset() {
    let indexer = IdIndexer::new();

    for i in 0..1000 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }

    let data = indexer.serialize();
    let deserialized = IdIndexer::deserialize(&data).unwrap();

    assert_eq!(deserialized.len(), 1000);

    for i in 0..1000 {
        assert_eq!(deserialized.get_index(&IdKey::Int(i)), Some(i as u32));
    }
}

#[test]
fn test_compute_mapping_is_pure_and_matches_compact() {
    let indexer = IdIndexer::new();
    for i in 0..5 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    indexer.remove(&IdKey::Int(1));
    indexer.remove(&IdKey::Int(3));

    let preview = indexer.compute_compact_mapping();
    let mut expected = HashMap::new();
    expected.insert(2u32, 1u32);
    expected.insert(4u32, 2u32);
    assert_eq!(preview, expected);
    // Preview mutated nothing: live set and lookups are unchanged.
    assert_eq!(indexer.live_ids(), vec![0, 2, 4]);
    assert_eq!(indexer.get_index(&IdKey::Int(4)), Some(4));

    let applied = indexer.compact().unwrap();
    assert_eq!(applied, expected);
}

#[test]
fn test_free_stack_reuses_hole_without_moving_survivors() {
    let indexer = IdIndexer::new();
    for i in 0..3 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    indexer.remove(&IdKey::Int(1));
    let reused = indexer.insert(IdKey::Int(99)).unwrap();
    assert_eq!(reused, 1);
    assert_eq!(indexer.get_index(&IdKey::Int(0)), Some(0));
    assert_eq!(indexer.get_index(&IdKey::Int(2)), Some(2));
    assert_eq!(indexer.get_index(&IdKey::Int(99)), Some(1));
}
#[test]
fn test_snapshot_restore_rolls_back_partial_remap() {
    let indexer = IdIndexer::new();
    for i in 0..3 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    let snapshot = indexer.snapshot_bytes();
    indexer.remove(&IdKey::Int(0));
    indexer.compact().unwrap();
    assert_eq!(indexer.get_index(&IdKey::Int(1)), Some(0));
    indexer.restore_snapshot(&snapshot).unwrap();
    assert_eq!(indexer.get_index(&IdKey::Int(0)), Some(0));
    assert_eq!(indexer.get_index(&IdKey::Int(1)), Some(1));
    assert_eq!(indexer.get_index(&IdKey::Int(2)), Some(2));
}

#[test]
fn test_lookup_collapses_index_and_visibility_check() {
    let indexer = IdIndexer::new();
    indexer.insert(IdKey::Text("a".to_string())).unwrap();
    assert_eq!(
        indexer.lookup(&IdKey::Text("a".to_string()), |_| true),
        PkLookup::Visible(0)
    );
    assert_eq!(
        indexer.lookup(&IdKey::Text("a".to_string()), |_| false),
        PkLookup::Missing
    );
    assert_eq!(
        indexer.lookup(&IdKey::Text("nope".to_string()), |_| true),
        PkLookup::Missing
    );
}

#[test]
fn test_delta_roundtrip_restores_exact_ids() {
    let indexer = IdIndexer::new();
    for i in 0..5 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    indexer.remove(&IdKey::Int(1));
    indexer.remove(&IdKey::Int(3));

    let baseline = indexer.serialize();
    let delta = indexer.serialize_delta();
    let entries = IdIndexer::deserialize_delta(&delta).unwrap();
    assert_eq!(entries.len(), indexer.delta_len());

    // Replay onto an empty loader plus the baseline snapshot.
    let restored_base = IdIndexer::deserialize(&baseline).unwrap();
    assert_eq!(restored_base.len(), 3);
    let mut replay = IdManager::new();
    replay.apply_delta_entries(&entries).unwrap();
    for (key, id) in indexer.iter() {
        assert_eq!(replay.get_id(&key), Some(id));
    }
    assert_eq!(replay.len(), 3);

    // Corrupt bytes fail the whole delta, never partially.
    let mut corrupt = delta.clone();
    corrupt[4] ^= 0xff;
    assert!(IdIndexer::deserialize_delta(&corrupt).is_err());
}

#[test]
fn test_compact_drops_delta_and_invalidates_baseline() {
    let indexer = IdIndexer::new();
    for i in 0..4 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    assert!(indexer.delta_len() > 0);
    indexer.remove(&IdKey::Int(0));
    indexer.remove(&IdKey::Int(1));
    indexer.compact().unwrap();
    assert_eq!(indexer.delta_len(), 0);
    assert!(indexer.should_anchor_baseline_for_live(indexer.len()));
    assert!(!indexer.should_anchor_baseline_for_live(indexer.len()));
}

#[test]
fn test_indexer_rejects_oversized_text_and_negative_int() {
    let indexer = IdIndexer::new();
    let oversized = "k".repeat(graphdb_core::types::VERTEX_ID_MAX_SIZE + 1);
    assert!(indexer.insert(IdKey::Text(oversized)).is_err());
    assert!(indexer.insert(IdKey::Int(-1)).is_err());
    assert_eq!(indexer.len(), 0);
}

#[test]
fn test_deserialize_rejects_impossible_count() {
    let mut corrupt = 0x0100_0000u32.to_le_bytes().to_vec();
    corrupt.extend_from_slice(&[0u8; 8]);
    assert!(IdIndexer::deserialize(&corrupt).is_err());
}

#[test]
fn test_delta_rejects_impossible_count() {
    let mut corrupt = u32::MAX.to_le_bytes().to_vec();
    corrupt.extend_from_slice(&[0u8; 8]);
    assert!(IdIndexer::deserialize_delta(&corrupt).is_err());
}

#[test]
fn test_delta_apply_rejects_divergence() {
    let mut base = IdManager::new();
    base.apply_delta_entries(&[(0, 7, IdKey::Int(3))]).unwrap();
    let divergent = vec![(0u8, 9u32, IdKey::Int(3))];
    assert!(base.apply_delta_entries(&divergent).is_err());
    let idempotent = vec![(0u8, 7u32, IdKey::Int(3))];
    assert!(base.apply_delta_entries(&idempotent).is_ok());
}

#[test]
fn test_over_threshold_delta_forces_anchor() {
    let indexer = IdIndexer::new();
    assert!(!indexer.should_anchor_baseline_for_live(indexer.len()));
    for i in 0..PK_DELTA_ANCHOR_THRESHOLD as i64 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    assert!(indexer.should_anchor_baseline_for_live(indexer.len()));
}

#[test]
fn test_reserve_is_invisible_until_registered() {
    let indexer = IdIndexer::new();
    let id = indexer.reserve_next().unwrap();
    assert_eq!(id, 0);
    assert_eq!(indexer.len(), 0);
    assert!(indexer.live_ids().is_empty());
    assert_eq!(indexer.delta_len(), 0);
    assert_eq!(indexer.get_key(id), None);
    // The high-water mark grows so concurrent inserts skip the slot.
    let other = indexer.insert(IdKey::Int(7)).unwrap();
    assert_eq!(other, 1);
    indexer.register_reserved(IdKey::Int(7), 0).unwrap_err();
    indexer.register_reserved(IdKey::Int(5), id).unwrap();
    assert_eq!(indexer.get_index(&IdKey::Int(5)), Some(0));
    assert_eq!(indexer.live_ids(), vec![0, 1]);
    assert_eq!(indexer.delta_len(), 2);
}

#[test]
fn test_reserve_reuses_free_stack_hole() {
    let indexer = IdIndexer::new();
    indexer.insert(IdKey::Int(0)).unwrap();
    let hole = indexer.insert(IdKey::Int(1)).unwrap();
    indexer.remove(&IdKey::Int(1));
    let reserved = indexer.reserve_next().unwrap();
    assert_eq!(reserved, hole);
    indexer.register_reserved(IdKey::Int(2), reserved).unwrap();
    assert_eq!(indexer.get_index(&IdKey::Int(2)), Some(hole));
}

#[test]
fn test_register_reserved_rejects_bound_or_foreign_slot() {
    let indexer = IdIndexer::new();
    let bound = indexer.insert(IdKey::Int(0)).unwrap();
    assert!(indexer.register_reserved(IdKey::Int(9), bound).is_err());
    assert!(indexer.register_reserved(IdKey::Int(0), 4).is_err());
    // A key already bound cannot claim any slot.
    let free = indexer.reserve_next().unwrap();
    assert!(indexer.register_reserved(IdKey::Int(0), free).is_err());
    assert_eq!(indexer.get_key(free), None);
}

#[test]
fn test_release_reserved_returns_only_unbound_slots() {
    let indexer = IdIndexer::new();
    let reserved = indexer.reserve_next().unwrap();
    let bound = indexer.insert(IdKey::Int(1)).unwrap();
    // Releasing a bound id is a no-op; releasing the reservation frees it.
    indexer.release_reserved(bound);
    indexer.release_reserved(reserved);
    let reused = indexer.reserve_next().unwrap();
    assert_eq!(reused, reserved);
}

#[test]
fn test_memory_breakdown_sums_to_usage() {
    let indexer = IdIndexer::new();
    for i in 0..8 {
        indexer.insert(IdKey::Text(format!("vertex-{i}"))).unwrap();
    }
    indexer.remove(&IdKey::Text("vertex-0".to_string()));
    let breakdown = indexer.memory_breakdown();
    assert_eq!(breakdown.live_count, 7);
    assert_eq!(breakdown.free_depth, 1);
    assert_eq!(breakdown.slot_count, 8);
    assert_eq!(breakdown.delta_entries, indexer.delta_len());
    assert!(breakdown.keys_heap_bytes > 0);
    assert!(breakdown.delta_heap_bytes > 0);
    assert!(breakdown.map_bytes > 0);
    assert!(breakdown.set_bytes > 0);
    assert!(
        breakdown.total_bytes
            >= breakdown.keys_heap_bytes
                + breakdown.delta_heap_bytes
                + breakdown.map_bytes
                + breakdown.set_bytes
                + breakdown.free_bytes
    );
    assert_eq!(breakdown.total_bytes, indexer.manager.memory_usage());
}

#[test]
fn test_reuse_count_tracks_free_stack_pops() {
    let indexer = IdIndexer::new();
    indexer.insert(IdKey::Int(0)).unwrap();
    indexer.insert(IdKey::Int(1)).unwrap();
    assert_eq!(indexer.reuse_count(), 0);
    indexer.remove(&IdKey::Int(0));
    indexer.insert(IdKey::Int(2)).unwrap();
    assert_eq!(indexer.reuse_count(), 1);
    assert_eq!(indexer.free_depth(), 0);
    indexer.insert(IdKey::Int(3)).unwrap();
    assert_eq!(indexer.reuse_count(), 1);
}

#[test]
fn test_reuse_claims_largest_hole_first() {
    let indexer = IdIndexer::new();
    for i in 0..4 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    // Free in ascending order so stack order alone would reclaim the
    // smallest hole first; ordered reuse must refill the tail instead.
    indexer.remove(&IdKey::Int(1));
    indexer.remove(&IdKey::Int(3));
    assert!((indexer.hole_ratio() - 0.5).abs() < f64::EPSILON);
    let first = indexer.insert(IdKey::Int(10)).unwrap();
    assert_eq!(first, 3);
    let second = indexer.insert(IdKey::Int(11)).unwrap();
    assert_eq!(second, 1);
    assert_eq!(indexer.free_depth(), 0);
    assert_eq!(indexer.hole_ratio(), 0.0);
}

#[test]
fn test_anchor_threshold_scales_with_live_size() {
    assert_eq!(
        IdManager::anchor_threshold_for_live(0),
        PK_DELTA_ANCHOR_THRESHOLD
    );
    assert_eq!(
        IdManager::anchor_threshold_for_live(PK_DELTA_ANCHOR_THRESHOLD * 8),
        PK_DELTA_ANCHOR_THRESHOLD * 2
    );
    let indexer = IdIndexer::new();
    for i in 0..8 {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    indexer.clear_index_delta();
    assert!(!indexer.should_anchor_baseline_for_live(8));
    for i in 8..(8 + PK_DELTA_ANCHOR_THRESHOLD as i64) {
        indexer.insert(IdKey::Int(i)).unwrap();
    }
    assert!(indexer.should_anchor_baseline_for_live(8));
}

#[test]
fn test_try_reclaim_cancels_a_release() {
    let indexer = IdIndexer::new();
    let reserved = indexer.reserve_next().unwrap();
    indexer.release_reserved(reserved);
    assert!(indexer.try_reclaim(reserved));
    // Reclaimed: the slot is held out of the free stack again, so the
    // next reservation grows elsewhere.
    let fresh = indexer.reserve_next().unwrap();
    assert_ne!(fresh, reserved);
    // A bound slot cannot be reclaimed.
    let bound = indexer.insert(IdKey::Int(1)).unwrap();
    assert!(!indexer.try_reclaim(bound));
    // An unbound slot that was never released reclaims as a no-op.
    let hole = indexer.reserve_next().unwrap();
    indexer.release_reserved(hole);
    indexer.reserve_next().unwrap();
    assert!(!indexer.try_reclaim(hole));
}

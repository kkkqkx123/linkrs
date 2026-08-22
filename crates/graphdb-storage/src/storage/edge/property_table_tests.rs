//! Property Table Tests
//!
//! Comprehensive test suite for PropertyTable functionality including:
//! - Basic insert/get/delete operations
//! - Property updates (single and multiple)
//! - Overflow handling (boundary values)
//! - Property schema operations (rename, remove)
//! - Persistence (dump/load roundtrips)
//! - Offset reuse after deletion
//! - MVCC snapshot isolation

use super::*;

#[test]
fn test_insert_and_get() {
    let mut table = PropertyTable::new();

    table
        .add_property("weight".to_string(), DataType::Double, false)
        .unwrap();
    table
        .add_property("since".to_string(), DataType::Int, true)
        .unwrap();

    let offset = table
        .insert(
            &[
                ("weight".to_string(), Value::Double(1.5)),
                ("since".to_string(), Value::Int(2020)),
            ],
            100,
        )
        .unwrap();

    let props = table.get(offset, None).unwrap();
    assert_eq!(props.len(), 2);

    let weight = table
        .get(offset, None)
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight, Some(Value::Double(1.5)));
    let since = table
        .get(offset, None)
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "since")
        .and_then(|(_, v)| v);
    assert_eq!(since, Some(Value::Int(2020)));
}

#[test]
fn test_update() {
    let mut table = PropertyTable::new();
    table
        .add_property("weight".to_string(), DataType::Double, false)
        .unwrap();

    let offset = table
        .insert(&[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();

    // In-place versioned update: the offset is stable and the new value
    // becomes the current version.
    table
        .update(offset, &[("weight".to_string(), Value::Double(2.0))], 200)
        .unwrap();

    let weight_current = table
        .get(offset, None)
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight_current, Some(Value::Double(2.0)));

    // The previous version stays readable as a before-image.
    let weight_before = table
        .get(offset, Some(150))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight_before, Some(Value::Double(1.0)));
}

/// Multi-column in-place update must preserve untouched columns (row
/// serialization writes NULL for absent names, so `update` has to carry the
/// full merged row) and keep the row at its original offset.
#[test]
fn test_update_preserves_other_columns() {
    let mut table = PropertyTable::new();
    // A String column keeps the schema variable-size, exercising the shared
    // slow path instead of the fixed-size fast path.
    table
        .add_property("name".to_string(), DataType::String, false)
        .unwrap();
    table
        .add_property("age".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(
            &[
                ("name".to_string(), Value::string("alice")),
                ("age".to_string(), Value::Int(30)),
            ],
            100,
        )
        .unwrap();

    table
        .update(offset, &[("age".to_string(), Value::Int(31))], 200)
        .unwrap();

    let props = table.get(offset, None).unwrap();
    assert_eq!(
        props
            .iter()
            .find(|(n, _)| n == "name")
            .and_then(|(_, v)| v.clone()),
        Some(Value::string("alice")),
        "untouched column must survive the update"
    );
    assert_eq!(
        props
            .iter()
            .find(|(n, _)| n == "age")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(31))
    );

    // Snapshot read before the update still sees the old age.
    assert_eq!(
        table
            .get(offset, Some(150))
            .unwrap()
            .iter()
            .find(|(n, _)| n == "age")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(30))
    );
}

#[test]
fn test_delete() {
    let mut table = PropertyTable::new();
    table
        .add_property("weight".to_string(), DataType::Double, false)
        .unwrap();

    let offset1 = table
        .insert(&[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let _offset2 = table
        .insert(&[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();

    assert!(table.delete(offset1));

    let offset3 = table
        .insert(&[("weight".to_string(), Value::Double(3.0))], 100)
        .unwrap();
    assert_eq!(offset3, offset1);
}

#[test]
fn test_parameterized_schema_survives_dump_load() {
    use crate::core::{ArrayTypeInfo, StructTypeInfo};
    use std::sync::Arc;

    let mut table = PropertyTable::new();
    let struct_type = DataType::Struct(Arc::new(StructTypeInfo::new(vec![
        ("city".to_string(), DataType::String),
        ("street".to_string(), DataType::String),
        (
            "geo".to_string(),
            DataType::Struct(Arc::new(StructTypeInfo::new(vec![
                ("lat".to_string(), DataType::Double),
                ("lon".to_string(), DataType::Double),
            ]))),
        ),
    ])));
    let array_type = DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, Some(3))));
    table
        .add_property("addr".to_string(), struct_type.clone(), true)
        .unwrap();
    table
        .add_property("coords".to_string(), array_type.clone(), true)
        .unwrap();
    // Plain (code <= 31) column alongside parameterized ones.
    table
        .add_property("weight".to_string(), DataType::Double, true)
        .unwrap();

    let data = table.dump();

    let mut loaded = PropertyTable::new();
    loaded.load(&data).expect("load must succeed");
    let types: Vec<_> = loaded
        .schema
        .iter()
        .map(|p| (p.name.clone(), p.data_type.clone()))
        .collect();
    assert_eq!(types.len(), 3);
    assert_eq!(types[0].1, struct_type);
    assert_eq!(types[1].1, array_type);
    assert_eq!(types[2].1, DataType::Double);
}

#[test]
fn test_dump_load_roundtrip() {
    let mut table = PropertyTable::new();
    table
        .add_property("weight".to_string(), DataType::Double, false)
        .unwrap();
    table
        .add_property("since".to_string(), DataType::Int, true)
        .unwrap();

    let offset1 = table
        .insert(
            &[
                ("weight".to_string(), Value::Double(1.5)),
                ("since".to_string(), Value::Int(2020)),
            ],
            100,
        )
        .unwrap();

    let offset2 = table
        .insert(
            &[
                ("weight".to_string(), Value::Double(2.5)),
                ("since".to_string(), Value::Int(2021)),
            ],
            100,
        )
        .unwrap();

    let data = table.dump();

    let mut loaded_table = PropertyTable::new();
    let _ = loaded_table.load(&data);

    let weight1 = loaded_table
        .get(offset1, None)
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight1, Some(Value::Double(1.5)));
    let weight2 = loaded_table
        .get(offset2, None)
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight2, Some(Value::Double(2.5)));
}

#[test]
fn test_rename_and_remove_property() {
    let mut table = PropertyTable::new();
    table
        .add_property("weight".to_string(), DataType::Double, false)
        .unwrap();
    table
        .add_property("since".to_string(), DataType::Int, true)
        .unwrap();

    let offset = table
        .insert(
            &[
                ("weight".to_string(), Value::Double(1.5)),
                ("since".to_string(), Value::Int(2020)),
            ],
            100,
        )
        .unwrap();

    table
        .rename_property("weight", "mass")
        .expect("rename should succeed");
    table
        .remove_property("since")
        .expect("remove should succeed");

    assert!(table.has_property("mass"));
    assert!(!table.has_property("weight"));
    assert!(!table.has_property("since"));

    let props = table.get(offset, None).expect("row should remain visible");
    assert_eq!(
        props
            .iter()
            .find(|(name, _)| name == "mass")
            .and_then(|(_, value)| value.clone()),
        Some(Value::Double(1.5))
    );
    assert!(props.iter().all(|(name, _)| name != "weight"));
    assert!(props.iter().all(|(name, _)| name != "since"));
}

// ==================== Priority Tests ====================

/// Test: Verify property update for single property
#[test]
fn test_property_table_update_single_property() {
    let mut table = PropertyTable::new();
    table
        .add_property("name".to_string(), DataType::String, false)
        .unwrap();
    table
        .add_property("age".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(
            &[
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::Int(30)),
            ],
            100,
        )
        .unwrap();

    // Update only age property
    table
        .set_property(offset, "age", Some(Value::Int(31)), 200)
        .expect("property update should succeed");

    let props = table.get(offset, None).expect("row should be visible");
    assert_eq!(
        props
            .iter()
            .find(|(n, _)| n == "age")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(31))
    );
    assert_eq!(
        props
            .iter()
            .find(|(n, _)| n == "name")
            .and_then(|(_, v)| v.clone()),
        Some(Value::string("Alice"))
    );
}

/// Test: Verify handling of large values
/// All values use columnar storage, so this test verifies large values work correctly.
#[test]
fn test_property_table_overflow_boundary_values() {
    let mut table = PropertyTable::new();
    table
        .add_property("data".to_string(), DataType::String, false)
        .unwrap();

    // Test values at overflow boundary
    let sizes = vec![255, 256, 257];
    let mut offsets = vec![];
    for size in &sizes {
        let value = format!("x-{}", "a".repeat(*size));
        let offset = table
            .insert(&[("data".to_string(), Value::string(value.clone()))], 100)
            .unwrap_or_else(|_| panic!("insert at size {} should succeed", size));
        offsets.push((offset, value));
    }

    // Verify all values are correctly stored and retrieved
    for (offset, expected_value) in offsets {
        let props = table.get(offset, None).expect("row should be visible");
        assert_eq!(
            props
                .iter()
                .find(|(n, _)| n == "data")
                .and_then(|(_, v)| v.clone()),
            Some(Value::string(expected_value))
        );
    }
}

/// Test: Verify property update with null values
#[test]
fn test_property_table_update_to_null() {
    let mut table = PropertyTable::new();
    table
        .add_property("optional".to_string(), DataType::String, true)
        .unwrap();

    let offset = table
        .insert(&[("optional".to_string(), Value::string("value"))], 100)
        .unwrap();

    // Update to null
    table
        .set_property(offset, "optional", None, 200)
        .expect("setting to null should succeed");

    let props = table.get(offset, None).expect("row should be visible");
    assert!(props
        .iter()
        .find(|(n, _)| n == "optional")
        .and_then(|(_, v)| v.clone())
        .is_none());
}

/// Test: Verify multiple property updates
#[test]
fn test_property_table_multiple_sequential_updates() {
    let mut table = PropertyTable::new();
    table
        .add_property("counter".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(&[("counter".to_string(), Value::Int(0))], 100)
        .unwrap();

    // Perform multiple updates
    for i in 1..=5 {
        table
            .set_property(offset, "counter", Some(Value::Int(i)), 100 + i as Timestamp)
            .unwrap_or_else(|_| panic!("update {} should succeed", i));

        let props = table.get(offset, None).expect("row should be visible");
        assert_eq!(
            props
                .iter()
                .find(|(n, _)| n == "counter")
                .and_then(|(_, v)| v.clone()),
            Some(Value::Int(i))
        );
    }
}

/// Test: Verify property offset reuse after slot reclamation
#[test]
fn test_property_table_offset_reuse() {
    let mut table = PropertyTable::new();
    table
        .add_property("value".to_string(), DataType::Int, false)
        .unwrap();

    let offset1 = table
        .insert(&[("value".to_string(), Value::Int(100))], 100)
        .unwrap();

    let offset2 = table
        .insert(&[("value".to_string(), Value::Int(200))], 100)
        .unwrap();

    // Tombstone offset1 and reclaim its slot: live rows keep their offsets,
    // the dead slot returns to the free list.
    table.mark_deleted(offset1, 150).unwrap();
    let reclaimed = table.reclaim_slots(&[offset2].iter().cloned().collect(), 300);
    assert_eq!(reclaimed, 1);

    // New insertion reuses the reclaimed slot.
    let offset3 = table
        .insert(&[("value".to_string(), Value::Int(300))], 100)
        .unwrap();
    assert_eq!(offset3, offset1);

    // Verify the new value is stored
    let props = table.get(offset3, None).expect("row should be visible");
    assert_eq!(
        props
            .iter()
            .find(|(n, _)| n == "value")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(300))
    );
}

// ==================== MVCC Tests ====================

/// Storage-layer write-write conflict detection: a write that would overlap a
/// newer existing version ("back-in-time" write) or a tombstoned row is
/// rejected at the write path, while forward time-travel version writes and
/// same-timestamp re-writes (rollback / WAL redo) pass through.
#[test]
fn test_concurrent_update_conflict() {
    let mut table = PropertyTable::new();
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();

    // Forward write at ts=200 creates a new version (legal history).
    table
        .set_property(offset, "v", Some(Value::Int(2)), 200)
        .unwrap();

    // Same-timestamp re-write (rollback / WAL redo) is allowed.
    table
        .set_property(offset, "v", Some(Value::Int(2)), 200)
        .unwrap();

    // A write at ts=150 would overlap the version created at ts=200: conflict.
    let err = table
        .set_property(offset, "v", Some(Value::Int(3)), 150)
        .unwrap_err();
    assert_eq!(
        err.kind(),
        crate::core::error::storage::StorageErrorKind::Conflict
    );

    // The newer version is unchanged.
    assert_eq!(
        table
            .get(offset, Some(200))
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v),
        Some(Value::Int(2))
    );
    // And the historical version is still queryable.
    assert_eq!(
        table
            .get(offset, Some(150))
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v),
        Some(Value::Int(1))
    );

    // Writing to a row tombstoned at ts=300, at a later ts, is a conflict.
    table.mark_deleted(offset, 300).unwrap();
    let err2 = table
        .set_property(offset, "v", Some(Value::Int(4)), 400)
        .unwrap_err();
    assert_eq!(
        err2.kind(),
        crate::core::error::storage::StorageErrorKind::Conflict
    );
}

/// The before-image chain length must stay bounded under heavy updates once a
/// cap is configured: memory grows with the cap, not with the update count.
#[test]
fn test_version_chain_bounded() {
    let mut table = PropertyTable::new();
    table.set_version_chain_cap(4);
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();
    let offset = table
        .insert(&[("v".to_string(), Value::Int(0))], 100)
        .unwrap();

    for i in 1..=200u32 {
        table
            .set_property(
                offset,
                "v",
                Some(Value::Int(i as i32)),
                100 + i as Timestamp,
            )
            .unwrap();
    }

    let row_idx = prop_offset_to_index(offset).unwrap();
    let chain_len = table.chain_records[row_idx].len();
    assert!(chain_len <= 4, "chain length {} exceeds cap 4", chain_len);
    // The current (newest) value is always exact.
    assert_eq!(
        table
            .get(offset, None)
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v),
        Some(Value::Int(200))
    );
}

/// After folding, the oldest interval returns the folded (oldest) value while
/// recent history stays exact (interval-merge semantics).
#[test]
fn test_merged_oldest_version() {
    let mut table = PropertyTable::new();
    table.set_version_chain_cap(3);
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();
    let offset = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();

    // Updates at 110/120/130/140 create versions 2..5, overflowing the cap.
    for (i, ts) in [110u64, 120, 130, 140].iter().enumerate() {
        table
            .set_property(offset, "v", Some(Value::Int(i as i32 + 2)), *ts)
            .unwrap();
    }

    let row_idx = prop_offset_to_index(offset).unwrap();
    let chain = &table.chain_records[row_idx];
    // The two oldest before-images (v1 and v2) were folded: the oldest entry
    // now covers [100, 120) and represents the oldest value.
    assert_eq!(chain[0].create_ts, 100);
    assert_eq!(chain[0].delete_ts, Some(120));
    assert_eq!(chain.len(), 3);

    // Querying the folded (oldest) range returns the oldest value.
    assert_eq!(
        table
            .get(offset, Some(115))
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v),
        Some(Value::Int(1))
    );
    // Recent history remains exact.
    assert_eq!(
        table
            .get(offset, Some(135))
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v),
        Some(Value::Int(4))
    );
    assert_eq!(
        table
            .get(offset, Some(145))
            .unwrap()
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v),
        Some(Value::Int(5))
    );
}

/// the O(1) `used_data_bytes` counter must match the sum of record
/// payloads after a sequence of insert / update / delete / compact.
#[test]
fn test_used_memory_counter_tracks_records() {
    let mut table = PropertyTable::new();
    table
        .add_property("name".to_string(), DataType::String, false)
        .unwrap();
    table
        .add_property("age".to_string(), DataType::Int, true)
        .unwrap();

    let direct_sum = |t: &PropertyTable| -> usize {
        t.records
            .iter()
            .flatten()
            .map(|record| record.data.len())
            .sum::<usize>()
            + t.chain_records
                .iter()
                .flatten()
                .map(|entry| entry.data.len())
                .sum::<usize>()
    };

    let o1 = table
        .insert(
            &[
                ("name".into(), Value::string("alice")),
                ("age".into(), Value::Int(30)),
            ],
            1,
        )
        .unwrap();
    assert_eq!(table.used_data_bytes, direct_sum(&table));

    let o2 = table
        .insert(
            &[
                ("name".into(), Value::string("bob")),
                ("age".into(), Value::Int(25)),
            ],
            2,
        )
        .unwrap();
    assert_eq!(table.used_data_bytes, direct_sum(&table));

    // In-place single property update.
    table
        .set_property_by_id(o1, PropertyId::new(1), Some(Value::Int(31)), 3)
        .unwrap();
    assert_eq!(table.used_data_bytes, direct_sum(&table));

    // Physical delete.
    assert!(table.delete(o2));
    assert_eq!(table.used_data_bytes, direct_sum(&table));

    // Slot reclamation with an unbounded retention bound reclaims nothing
    // and leaves byte accounting untouched.
    let reclaimed = table.reclaim_slots(&[o1].iter().cloned().collect(), Timestamp::MAX);
    assert_eq!(reclaimed, 0);
    assert_eq!(table.used_data_bytes, direct_sum(&table));
}

// ==================== Version Chain Tests ====================

/// RepeatableRead within a transaction: after an in-place property update,
/// a snapshot read at `query_ts` between the update and the write returns the
/// historical value, while reads at/after the write see the new value.
#[test]
fn test_version_chain_get_at_ts() {
    let mut table = PropertyTable::new();
    table
        .add_property("weight".to_string(), DataType::Double, false)
        .unwrap();

    let offset = table
        .insert(&[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();

    // Snapshot before the insert sees nothing.
    assert!(table.get(offset, Some(99)).is_none());

    // In-place update at ts=200 supersedes version 1.0.
    table
        .set_property(offset, "weight", Some(Value::Double(2.0)), 200)
        .unwrap();

    // Historical snapshot: still 1.0.
    let weight_before = table
        .get(offset, Some(150))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight_before, Some(Value::Double(1.0)));

    // Current / at-update snapshot: 2.0.
    let weight_at = table
        .get(offset, Some(200))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight_at, Some(Value::Double(2.0)));
    let weight_after = table
        .get(offset, None)
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight_after, Some(Value::Double(2.0)));

    // A second update at 300 keeps two before-images: reads at 150 and 250
    // return 1.0 and 2.0 respectively.
    table
        .set_property(offset, "weight", Some(Value::Double(3.0)), 300)
        .unwrap();
    let v150 = table
        .get(offset, Some(150))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    let v250 = table
        .get(offset, Some(250))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    let v300 = table
        .get(offset, Some(300))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(v150, Some(Value::Double(1.0)));
    assert_eq!(v250, Some(Value::Double(2.0)));
    assert_eq!(v300, Some(Value::Double(3.0)));
}

/// Fixed-size fast path must version equally: snapshot reads resolve the
/// history written through `set_property_fixed_size`.
#[test]
fn test_version_chain_fixed_size_at_ts() {
    let mut table = PropertyTable::new();
    table
        .add_property("a".to_string(), DataType::Int, false)
        .unwrap();
    table
        .add_property("b".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(
            &[
                ("a".to_string(), Value::Int(1)),
                ("b".to_string(), Value::Int(2)),
            ],
            10,
        )
        .unwrap();

    table
        .set_property_by_id(offset, PropertyId::new(0), Some(Value::Int(11)), 20)
        .unwrap();

    let a_before = table
        .get_fast(offset, Some(15))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "a")
        .and_then(|(_, v)| v);
    let a_current = table
        .get_fast(offset, Some(20))
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "a")
        .and_then(|(_, v)| v);
    assert_eq!(a_before, Some(Value::Int(1)));
    assert_eq!(a_current, Some(Value::Int(11)));
}

/// `gc_versions` must drop only before-images invisible to the oldest active
/// snapshot and upgrade the O(1) used_data_bytes counter accordingly.
#[test]
fn test_gc_versions_reclaims_obsolete_chain_entries() {
    let mut table = PropertyTable::new();
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();
    table
        .set_property(offset, "v", Some(Value::Int(2)), 200)
        .unwrap();
    table
        .set_property(offset, "v", Some(Value::Int(3)), 300)
        .unwrap();

    assert!(table.get(offset, Some(150)).is_some());
    let used_before = table.used_data_bytes;

    // Oldest active snapshot at 250: the 1.0 entry (interval [100, 200)) is
    // obsolete. Reads at/after 250 are consistent; reads below are not
    // guaranteed and may fall through to the current version.
    let removed = table.gc_versions(250);
    assert_eq!(removed, 1);
    assert!(table.used_data_bytes < used_before);
    assert_eq!(
        table.get(offset, Some(250)).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(2))
    );
    assert_eq!(
        table.get(offset, None).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(3))
    );

    // Oldest active snapshot at 350: no before-image survives.
    let removed2 = table.gc_versions(350);
    assert!(removed2 >= 1);
}

/// A `gc_versions` window that predates the oldest kept timestamp still leaves
/// reads at the boundary consistent (the superseding version covers it).
#[test]
fn test_gc_versions_boundary_semantics() {
    let mut table = PropertyTable::new();
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();
    table
        .set_property(offset, "v", Some(Value::Int(2)), 200)
        .unwrap();

    // Oldest active snapshot == the entry's delete_ts (200): the before-image
    // interval is [100, 200), so it is obsolete — reads at 200 are served by
    // the current version and stay consistent.
    let removed = table.gc_versions(200);
    assert_eq!(removed, 1);

    assert_eq!(
        table.get(offset, Some(200)).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(2))
    );
}

/// Before-image chains survive a dump/load roundtrip so snapshot reads stay
/// consistent after a cold restart.
#[test]
fn test_version_chain_survives_dump_load() {
    let mut table = PropertyTable::new();
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();

    let offset = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();
    table
        .set_property(offset, "v", Some(Value::Int(2)), 200)
        .unwrap();
    let chain_len_before = table.chain_records[prop_offset_to_index(offset).unwrap()].len();
    assert_eq!(chain_len_before, 1);

    let dumped = table.dump();
    let mut restored = PropertyTable::new();
    restored.load(&dumped).unwrap();

    assert!(restored.get(offset, Some(150)).is_some());
    assert_eq!(
        restored.get(offset, Some(150)).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(1))
    );
    assert_eq!(
        restored.get(offset, None).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(2))
    );
    assert_eq!(
        restored.chain_records[prop_offset_to_index(offset).unwrap()].len(),
        1
    );
}

/// Before-images survive slot reclamation, and live rows keep their offsets,
/// so snapshot reads stay correct without any relocation mapping.
#[test]
fn test_version_chain_survives_slot_reclaim() {
    let mut table = PropertyTable::new();
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();

    let keep = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();
    let drop = table
        .insert(&[("v".to_string(), Value::Int(9))], 100)
        .unwrap();
    table
        .set_property(keep, "v", Some(Value::Int(2)), 200)
        .unwrap();

    // Tombstone the dead row and reclaim it; the live row is untouched.
    table.mark_deleted(drop, 150).unwrap();
    let reclaimed = table.reclaim_slots(&[keep].iter().cloned().collect(), 300);
    assert_eq!(reclaimed, 1);

    // The dead row's slot is cleared wholesale.
    assert!(table.get(drop, Some(150)).is_none());
    assert!(!table.is_deleted(drop));

    // The live row kept its offset: history and current version both read
    // through the original offset.
    assert_eq!(
        table.get(keep, Some(150)).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(1))
    );
    assert_eq!(
        table.get(keep, None).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(2))
    );
}

/// Slot reclamation must not destroy history an active snapshot may still
/// observe: rows deleted after the retention bound survive the pass, and an
/// unbounded bound (MAX) reclaims nothing.
#[test]
fn test_slot_reclaim_respects_retention_bound() {
    let mut table = PropertyTable::new();
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();

    let pinned = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();
    let dead = table
        .insert(&[("v".to_string(), Value::Int(9))], 100)
        .unwrap();

    // `dead` was deleted at ts=400, which is after the bound at ts=300:
    // a snapshot registered between 300 and 400 may still observe it.
    table.mark_deleted(pinned, 500).unwrap();
    table.mark_deleted(dead, 400).unwrap();

    let valid = [pinned].iter().cloned().collect::<HashSet<u32>>();
    assert_eq!(table.reclaim_slots(&valid, 300), 0);
    assert!(table.get(dead, None).is_none()); // tombstoned but slot intact

    // Once time moves past every deletion point, the row is reclaimable.
    assert_eq!(table.reclaim_slots(&valid, 450), 1);

    // An unbounded bound is never treated as a real timestamp.
    assert_eq!(table.reclaim_slots(&HashSet::new(), Timestamp::MAX), 0);
}

/// `update()` shares the write-write conflict semantics of `set_property`:
/// a back-in-time write or a write past a tombstone is rejected without any
/// side effect on the stored row.
#[test]
fn test_update_rejects_conflicting_write() {
    let mut table = PropertyTable::new();
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();
    let offset = table
        .insert(&[("v".to_string(), Value::Int(1))], 100)
        .unwrap();

    // A newer version exists at ts=300; updating at ts=150 would clobber it.
    table
        .set_property(offset, "v", Some(Value::Int(2)), 300)
        .unwrap();
    let err = table
        .update(offset, &[("v".to_string(), Value::Int(3))], 150)
        .unwrap_err();
    assert_eq!(
        err.kind(),
        crate::core::error::storage::StorageErrorKind::Conflict
    );

    // Writing to a row tombstoned at ts=400, at a later ts, is a conflict.
    table.mark_deleted(offset, 400).unwrap();
    let err2 = table
        .update(offset, &[("v".to_string(), Value::Int(4))], 500)
        .unwrap_err();
    assert_eq!(
        err2.kind(),
        crate::core::error::storage::StorageErrorKind::Conflict
    );

    // The rejected writes left the row untouched.
    assert_eq!(
        table.get(offset, None).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        None,
        "tombstoned row stays invisible"
    );
    assert!(table.is_deleted(offset));
}

/// Version-chain folding must not destroy entries an active snapshot may
/// still observe. While the retention horizon pins old history the chain may
/// temporarily exceed the cap; once snapshots release, folding resumes.
#[test]
fn test_fold_respects_active_snapshot_horizon() {
    let mut table = PropertyTable::new();
    table.set_version_chain_cap(2);
    table
        .add_property("v".to_string(), DataType::Int, false)
        .unwrap();
    let offset = table
        .insert(&[("v".to_string(), Value::Int(0))], 100)
        .unwrap();

    // Pin history at ts=115: every version created afterwards (delete_ts >
    // 115) must survive folding, so the chain grows past the cap.
    table.set_retention_horizon(115);
    for i in 1..=20u32 {
        table
            .set_property(
                offset,
                "v",
                Some(Value::Int(i as i32)),
                100 + i as Timestamp * 10,
            )
            .unwrap();
    }

    let row_idx = prop_offset_to_index(offset).unwrap();
    let chain_len = table.chain_records[row_idx].len();
    assert!(
        chain_len > 2,
        "pinned versions must not be folded: chain_len={}",
        chain_len
    );

    // The pinned snapshot's view of ts=115 is exact (version [110, 120)).
    assert_eq!(
        table.get(offset, Some(115)).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(1))
    );
    // And the current value too.
    assert_eq!(
        table.get(offset, None).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(20))
    );

    // Release the snapshot: folding resumes and the chain is bounded again.
    table.set_retention_horizon(Timestamp::MAX);
    table
        .set_property(offset, "v", Some(Value::Int(21)), 400)
        .unwrap();
    let chain_len = table.chain_records[row_idx].len();
    assert!(chain_len <= 2, "chain length {} exceeds cap 2", chain_len);

    // The current value stays exact; recent history is preserved by the
    // newest surviving before-image ([300, 400) → value 20).
    assert_eq!(
        table.get(offset, None).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(21))
    );
    assert_eq!(
        table.get(offset, Some(395)).and_then(|props| props
            .into_iter()
            .find(|(n, _)| n == "v")
            .and_then(|(_, v)| v)),
        Some(Value::Int(20))
    );
}

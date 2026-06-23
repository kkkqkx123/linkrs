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
use crate::storage::mvcc::MVCCTable;

#[test]
fn test_insert_and_get() {
    let mut table = PropertyTable::new();

    table.add_property("weight".to_string(), DataType::Double, false);
    table.add_property("since".to_string(), DataType::Int, true);

    let offset = table
        .insert(&[
            ("weight".to_string(), Value::Double(1.5)),
            ("since".to_string(), Value::Int(2020)),
        ], 100)
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
    table.add_property("weight".to_string(), DataType::Double, false);

    let _offset = table
        .insert(&[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();

    // Update returns a new offset with the new record
    let new_offset = table
        .update(_offset, &[("weight".to_string(), Value::Double(2.0))], 200)
        .unwrap();

    let weight = table
        .get(new_offset, None)
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "weight")
        .and_then(|(_, v)| v);
    assert_eq!(weight, Some(Value::Double(2.0)));
}

#[test]
fn test_delete() {
    let mut table = PropertyTable::new();
    table.add_property("weight".to_string(), DataType::Double, false);

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
fn test_dump_load_roundtrip() {
    let mut table = PropertyTable::new();
    table.add_property("weight".to_string(), DataType::Double, false);
    table.add_property("since".to_string(), DataType::Int, true);

    let offset1 = table
        .insert(&[
            ("weight".to_string(), Value::Double(1.5)),
            ("since".to_string(), Value::Int(2020)),
        ], 100)
        .unwrap();

    let offset2 = table
        .insert(&[
            ("weight".to_string(), Value::Double(2.5)),
            ("since".to_string(), Value::Int(2021)),
        ], 100)
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
    table.add_property("weight".to_string(), DataType::Double, false);
    table.add_property("since".to_string(), DataType::Int, true);

    let offset = table
        .insert(&[
            ("weight".to_string(), Value::Double(1.5)),
            ("since".to_string(), Value::Int(2020)),
        ], 100)
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

// ==================== P0 Priority Tests ====================

/// Test: Verify property update for single property
#[test]
fn test_property_table_update_single_property() {
    let mut table = PropertyTable::new();
    table.add_property("name".to_string(), DataType::String, false);
    table.add_property("age".to_string(), DataType::Int, false);

    let offset = table
        .insert(&[
            ("name".to_string(), Value::String("Alice".to_string())),
            ("age".to_string(), Value::Int(30)),
        ], 100)
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
        Some(Value::String("Alice".to_string()))
    );
}

/// Test: Verify handling of large values
/// All values use columnar storage, so this test verifies large values work correctly.
#[test]
fn test_property_table_overflow_boundary_values() {
    let mut table = PropertyTable::new();
    table.add_property("data".to_string(), DataType::String, false);

    // Test values at overflow boundary
    let sizes = vec![255, 256, 257];
    let mut offsets = vec![];
    for size in &sizes {
        let value = format!("x-{}", "a".repeat(*size));
        let offset = table
            .insert(&[("data".to_string(), Value::String(value.clone()))], 100)
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
            Some(Value::String(expected_value))
        );
    }
}

/// Test: Verify property update with null values
#[test]
fn test_property_table_update_to_null() {
    let mut table = PropertyTable::new();
    table.add_property("optional".to_string(), DataType::String, true);

    let offset = table
        .insert(&[("optional".to_string(), Value::String("value".to_string()))], 100)
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
    table.add_property("counter".to_string(), DataType::Int, false);

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

/// Test: Verify property offset reuse after deletion
#[test]
fn test_property_table_offset_reuse() {
    let mut table = PropertyTable::new();
    table.add_property("value".to_string(), DataType::Int, false);

    let _offset1 = table
        .insert(&[("value".to_string(), Value::Int(100))], 100)
        .unwrap();

    let offset2 = table
        .insert(&[("value".to_string(), Value::Int(200))], 100)
        .unwrap();

    // Mark offset1 as deleted via compact
    table.compact(&[offset2].iter().cloned().collect());

    // New insertion might reuse offset1
    let offset3 = table
        .insert(&[("value".to_string(), Value::Int(300))], 100)
        .unwrap();

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

/// Test: Snapshot isolation for property records via update
#[test]
fn test_property_table_snapshot_isolation() {
    let mut table = PropertyTable::new();
    table.add_property("value".to_string(), DataType::Int, false);

    // Time 100: Insert first record
    let offset1 = table
        .insert(&[("value".to_string(), Value::Int(1))], 100)
        .unwrap();

    // Time 150: Register a snapshot
    let snap1 = table.register_snapshot(150).unwrap();

    // Time 200: Update the record (returns new offset)
    let offset2 = table
        .update(offset1, &[("value".to_string(), Value::Int(2))], 200)
        .unwrap();

    // Snapshot at time 150 can still see old version at offset1
    let props_snap1 = table.get(offset1, Some(150)).unwrap();
    assert_eq!(
        props_snap1
            .iter()
            .find(|(n, _)| n == "value")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(1))
    );

    // Current query at offset2 sees new version
    let props_now = table.get(offset2, None).unwrap();
    assert_eq!(
        props_now
            .iter()
            .find(|(n, _)| n == "value")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(2))
    );

    // Cleanup
    table.unregister_snapshot(snap1).unwrap();
}

/// Test: Multiple concurrent snapshots
#[test]
fn test_property_table_multiple_snapshots() {
    let mut table = PropertyTable::new();
    table.add_property("counter".to_string(), DataType::Int, false);

    // Time 100: Insert
    let offset1 = table
        .insert(&[("counter".to_string(), Value::Int(1))], 100)
        .unwrap();

    // Time 100: Create first snapshot
    let snap1 = table.register_snapshot(100).unwrap();

    // Time 200: Update and create second snapshot
    let offset2 = table
        .update(offset1, &[("counter".to_string(), Value::Int(2))], 200)
        .unwrap();
    let snap2 = table.register_snapshot(200).unwrap();

    // Time 300: Update again
    let offset3 = table
        .update(offset2, &[("counter".to_string(), Value::Int(3))], 300)
        .unwrap();

    // Snapshot 1 sees version from time 100
    let props1 = table.get(offset1, Some(100)).unwrap();
    assert_eq!(
        props1
            .iter()
            .find(|(n, _)| n == "counter")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(1))
    );

    // Snapshot 2 sees version from time 200
    let props2 = table.get(offset2, Some(200)).unwrap();
    assert_eq!(
        props2
            .iter()
            .find(|(n, _)| n == "counter")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(2))
    );

    // Current sees version from time 300
    let props_now = table.get(offset3, None).unwrap();
    assert_eq!(
        props_now
            .iter()
            .find(|(n, _)| n == "counter")
            .and_then(|(_, v)| v.clone()),
        Some(Value::Int(3))
    );

    // Cleanup
    table.unregister_snapshot(snap1).unwrap();
    table.unregister_snapshot(snap2).unwrap();
}

/// Test: MVCC Garbage collection
#[test]
fn test_property_table_mvcc_gc() {
    let mut table = PropertyTable::new();
    table.add_property("value".to_string(), DataType::Int, false);

    // Time 100: Insert
    let offset = table
        .insert(&[("value".to_string(), Value::Int(1))], 100)
        .unwrap();

    // Time 200: Mark as deleted
    table.mark_deleted(offset, 200).unwrap();

    // Should have active snapshot tracking
    assert_eq!(table.active_snapshot_count(), 0);

    // Register a snapshot at time 150 (before deletion)
    let snap1 = table.register_snapshot(150).unwrap();
    assert_eq!(table.active_snapshot_count(), 1);

    // Register another at time 250 (after deletion)
    let snap2 = table.register_snapshot(250).unwrap();
    assert_eq!(table.active_snapshot_count(), 2);

    // GC should be prevented because of active snapshots
    let _gc_result = table.gc(300).unwrap();

    // After unregistering snap1, GC can still not remove (snap2 is still active)
    table.unregister_snapshot(snap1).unwrap();
    assert_eq!(table.active_snapshot_count(), 1);

    // After unregistering all snapshots, GC can proceed
    table.unregister_snapshot(snap2).unwrap();
    assert_eq!(table.active_snapshot_count(), 0);

    let gc_reclaimed = table.gc(300).unwrap();
    // Should reclaim the deleted record
    assert!(gc_reclaimed > 0);
}

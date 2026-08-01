use super::*;
use crate::core::types::Timestamp;
use crate::core::DataType;
use crate::storage::types::StoragePropertyDef;

fn new_table(label: LabelId, label_name: &str, schema: VertexSchema) -> VertexTable {
    VertexTable::with_config(
        label,
        label_name.to_string(),
        schema,
        VertexTableConfig::default(),
    )
}

fn create_test_schema() -> VertexSchema {
    VertexSchema {
        label_id: 0,
        label_name: "person".to_string(),
        properties: vec![
            StoragePropertyDef::new("name".to_string(), DataType::String),
            StoragePropertyDef {
                name: "age".to_string(),
                data_type: DataType::Int,
                nullable: true,
                default_value: None,
            },
        ],
        primary_key_index: 0,
        schema_version: 1,
    }
}

#[test]
fn test_insert_and_get() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    let internal_id = table
        .insert(
            "v1",
            &[
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::Int(30)),
            ],
            100,
        )
        .unwrap();

    assert_eq!(internal_id, 0);

    let lookup_id = table.get_internal_id("v1", 100).unwrap();
    let record = table.get_by_internal_id(lookup_id, 100).unwrap();
    assert_eq!(record.properties.len(), 2);
}

#[test]
fn test_batch_projected_read() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert(
            "v1",
            &[
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::Int(30)),
            ],
            100,
        )
        .unwrap();
    table
        .insert(
            "v2",
            &[
                ("name".to_string(), Value::string("Bob")),
                ("age".to_string(), Value::Int(25)),
            ],
            100,
        )
        .unwrap();
    table
        .insert("v3", &[("name".to_string(), Value::string("Carol"))], 100)
        .unwrap();

    let ids = table.live_ids();
    assert_eq!(ids, vec![0, 1, 2]);

    // Full read, aligned with input order.
    let all = table.get_projected_batch(&[1, 0, 2], 100, None);
    let names: Vec<Option<Value>> = all
        .iter()
        .map(|r| {
            r.as_ref()
                .and_then(|rec| rec.properties.iter().find(|(n, _)| n == "name"))
                .map(|(_, v)| v.clone())
        })
        .collect();
    assert_eq!(
        names,
        vec![
            Some(Value::string("Bob")),
            Some(Value::string("Alice")),
            Some(Value::string("Carol"))
        ]
    );

    // Projection only decodes the requested column.
    let projected = table.get_projected_batch(&[0, 1], 100, Some(&["age".to_string()]));
    let projected: Vec<Vec<String>> = projected
        .into_iter()
        .flatten()
        .map(|rec| {
            rec.properties
                .iter()
                .map(|(name, _)| name.clone())
                .collect()
        })
        .collect();
    assert_eq!(
        projected,
        vec![vec!["age".to_string()], vec!["age".to_string()]]
    );

    // Invalid (deleted) id yields None in its input position.
    table.delete("v2", 100).unwrap();
    let with_gap = table.get_projected_batch(&[0, 1, 2], 100, None);
    assert!(with_gap[0].is_some());
    assert!(with_gap[1].is_none());
    assert!(with_gap[2].is_some());
}

#[test]
fn test_delete() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    table.delete("v1", 200).unwrap();

    let internal_id = table.get_internal_id("v1", 150).unwrap();
    assert!(table.get_by_internal_id(internal_id, 150).is_some());
    assert!(table.get_internal_id("v1", 250).is_none());
}

#[test]
fn test_iterator() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();
    table
        .insert("v2", &[("name".to_string(), Value::string("Bob"))], 100)
        .unwrap();
    table
        .insert("v3", &[("name".to_string(), Value::string("Charlie"))], 100)
        .unwrap();

    let count = table.scan(100).count();
    assert_eq!(count, 3);
}

#[test]
fn test_rename_and_remove_property() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .add_property(StoragePropertyDef::new(
            "city".to_string(),
            DataType::String,
        ))
        .expect("add property should succeed");

    let internal_id = table
        .insert(
            "v1",
            &[
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::Int(30)),
                ("city".to_string(), Value::string("Shanghai")),
            ],
            100,
        )
        .unwrap();

    table
        .rename_property("age", "years")
        .expect("rename should succeed");
    table
        .remove_property("city")
        .expect("remove should succeed");

    let record = table
        .get_by_internal_id(internal_id, 100)
        .expect("record should remain visible");

    assert_eq!(
        record
            .properties
            .iter()
            .find(|(name, _)| name == "years")
            .map(|(_, value)| value),
        Some(&Value::Int(30))
    );
    assert!(record.properties.iter().all(|(name, _)| name != "age"));
    assert!(record.properties.iter().all(|(name, _)| name != "city"));
    assert_eq!(
        table
            .schema()
            .properties
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        vec!["name", "years"]
    );
}

#[test]
fn test_batch_insert() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    let vertices = vec![
        (
            "v1".to_string(),
            vec![
                ("name".to_string(), Value::string("Alice")),
                ("age".to_string(), Value::Int(30)),
            ],
        ),
        (
            "v2".to_string(),
            vec![
                ("name".to_string(), Value::string("Bob")),
                ("age".to_string(), Value::Int(25)),
            ],
        ),
        (
            "v3".to_string(),
            vec![
                ("name".to_string(), Value::string("Charlie")),
                ("age".to_string(), Value::Int(35)),
            ],
        ),
    ];

    let ids: Vec<u32> = vertices
        .into_iter()
        .map(|(ext_id, props)| table.insert(&ext_id, &props, 100).unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    assert_eq!(ids[0], 0);
    assert_eq!(ids[1], 1);
    assert_eq!(ids[2], 2);

    let count = table.scan(100).count();
    assert_eq!(count, 3);

    let record1 = table.get_by_internal_id(ids[0], 100).unwrap();
    assert_eq!(
        record1
            .properties
            .iter()
            .find(|(n, _)| n == "name")
            .map(|(_, v)| v),
        Some(&Value::string("Alice"))
    );
}

#[test]
fn test_batch_delete() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();
    table
        .insert("v2", &[("name".to_string(), Value::string("Bob"))], 100)
        .unwrap();
    table
        .insert("v3", &[("name".to_string(), Value::string("Charlie"))], 100)
        .unwrap();

    let deleted = table.batch_delete(&["v1", "v3"], 200).unwrap();
    assert_eq!(deleted, 2);

    let count_before_delete = table.scan(100).count();
    assert_eq!(count_before_delete, 3);

    let count_after_delete = table.scan(200).count();
    assert_eq!(count_after_delete, 1);

    assert!(table.get_internal_id("v2", 200).is_some());
    assert!(table.get_internal_id("v1", 200).is_none());
}

#[test]
fn test_add_property_increments_version() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    let v1 = table.schema().schema_version;
    assert_eq!(v1, 1, "Initial version should be 1");

    table
        .add_property(StoragePropertyDef::new(
            "email".to_string(),
            DataType::String,
        ))
        .expect("add_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, 2, "Version should increment after add_property");
}

#[test]
fn test_remove_property_increments_version() {
    let mut schema = create_test_schema();
    schema.properties.push(StoragePropertyDef::new(
        "email".to_string(),
        DataType::String,
    ));
    let mut table = new_table(0, "person", schema);

    let v1 = table.schema().schema_version;

    table
        .remove_property("email")
        .expect("remove_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, v1 + 1, "Version should increment after remove_property");
}

#[test]
fn test_rename_property_increments_version() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    let v1 = table.schema().schema_version;

    table
        .rename_property("name", "full_name")
        .expect("rename_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, v1 + 1, "Version should increment after rename_property");
}

#[test]
fn test_sequential_property_modifications() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    assert_eq!(table.schema().schema_version, 1);

    table
        .add_property(StoragePropertyDef::new(
            "email".to_string(),
            DataType::String,
        ))
        .expect("add_property 1 should succeed");
    assert_eq!(table.schema().schema_version, 2);

    table
        .add_property(StoragePropertyDef::new(
            "phone".to_string(),
            DataType::String,
        ))
        .expect("add_property 2 should succeed");
    assert_eq!(table.schema().schema_version, 3);

    table
        .rename_property("email", "email_address")
        .expect("rename_property should succeed");
    assert_eq!(table.schema().schema_version, 4);

    table
        .remove_property("phone")
        .expect("remove_property should succeed");
    assert_eq!(table.schema().schema_version, 5);
}

#[test]
fn test_version_history_add_property() {
    use crate::storage::schema::ChangeDetails;

    let schema = create_test_schema();
    let mut table = new_table(1, "User", schema);

    table
        .add_property(StoragePropertyDef::new(
            "email".to_string(),
            DataType::String,
        ))
        .expect("add_property should succeed");

    let history = table.version_history.lock().unwrap();
    let changes = history.change_log.get_version_changes(2);
    assert!(changes.is_some(), "Should have changes for version 2");

    let changes = changes.unwrap();
    assert_eq!(changes.len(), 1, "Should have exactly one change");

    let change = &changes[0];
    match &change.details {
        ChangeDetails::PropertyAdded { name, .. } => {
            assert_eq!(name, "email");
        }
        _ => panic!("Expected PropertyAdded change"),
    }
}

#[test]
fn test_version_history_remove_property() {
    use crate::storage::schema::ChangeDetails;

    let mut schema = create_test_schema();
    schema.properties.push(StoragePropertyDef::new(
        "email".to_string(),
        DataType::String,
    ));

    let mut table = new_table(1, "User", schema);

    table
        .remove_property("email")
        .expect("remove_property should succeed");

    let history = table.version_history.lock().unwrap();
    let changes = history.change_log.get_version_changes(2);
    assert!(changes.is_some(), "Should have changes for version 2");

    let changes = changes.unwrap();
    assert_eq!(changes.len(), 1, "Should have exactly one change");

    let change = &changes[0];
    match &change.details {
        ChangeDetails::PropertyRemoved { name, .. } => {
            assert_eq!(name, "email");
        }
        _ => panic!("Expected PropertyRemoved change"),
    }
}

#[test]
fn test_version_history_rename_property() {
    use crate::storage::schema::ChangeDetails;

    let schema = create_test_schema();
    let mut table = new_table(1, "User", schema);

    table
        .rename_property("name", "full_name")
        .expect("rename_property should succeed");

    let history = table.version_history.lock().unwrap();
    let changes = history.change_log.get_version_changes(2);
    assert!(changes.is_some(), "Should have changes for version 2");

    let changes = changes.unwrap();
    assert_eq!(changes.len(), 1, "Should have exactly one change");

    let change = &changes[0];
    match &change.details {
        ChangeDetails::PropertyRenamed { old_name, new_name } => {
            assert_eq!(old_name, "name");
            assert_eq!(new_name, "full_name");
        }
        _ => panic!("Expected PropertyRenamed change"),
    }
}

#[test]
fn test_compact_delete_all() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    for i in 0..5 {
        table
            .insert(
                &format!("v{}", i),
                &[("name".to_string(), Value::string(format!("Person{}", i)))],
                100,
            )
            .unwrap();
    }

    assert_eq!(table.scan(100).count(), 5);

    for i in 0..5 {
        table.delete(&format!("v{}", i), 200).unwrap();
    }

    assert_eq!(table.scan(200).count(), 0);

    let removed = table
        .compact_with_ts_collect_mapping(300)
        .expect("compact_with_ts_collect_mapping should succeed")
        .0;
    assert_eq!(removed.len(), 5, "Should have removed 5 deleted entries");

    assert_eq!(table.scan(200).count(), 0);
    assert_eq!(
        table.id_indexer.len(),
        0,
        "id_indexer should be empty after removing all deleted entries"
    );
    assert_eq!(
        table.timestamps.size(),
        0,
        "timestamps should be empty after removing all deleted entries"
    );
}

#[test]
fn test_compact_multiple_cycles() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    for cycle in 0..3 {
        let ts_insert = cycle * 100;
        let ts_delete = ts_insert + 50;
        let ts_compact = ts_delete + 50;

        for i in 0..10 {
            table
                .insert(
                    &format!("v{}_{}", cycle, i),
                    &[("name".to_string(), Value::string(format!("P{}", i)))],
                    ts_insert,
                )
                .unwrap_or_else(|_| panic!("insert cycle {} should succeed", cycle));
        }

        let scan_count = table.scan(ts_insert).count();

        let mut expected_after_insert = 0;
        for i in 0..=cycle {
            let ts_cycle_delete = i * 100 + 50;
            if (ts_insert as u32) > (ts_cycle_delete as u32) {
                expected_after_insert += 5;
            } else {
                expected_after_insert += 10;
            }
        }

        assert_eq!(
            scan_count, expected_after_insert,
            "Should have {} vertices after insert at cycle {}",
            expected_after_insert, cycle
        );

        for i in 0..10 {
            if i % 2 == 0 {
                table
                    .delete(&format!("v{}_{}", cycle, i), ts_delete)
                    .unwrap_or_else(|_| panic!("delete cycle {} should succeed", cycle));
            }
        }

        table
            .compact_coordinated()
            .unwrap_or_else(|_| panic!("compact cycle {} should succeed", cycle));

        let mut expected_count = 0;
        for i in 0..=cycle {
            let ts_cycle_delete = i * 100 + 50;
            if ts_compact > ts_cycle_delete {
                expected_count += 5;
            } else {
                expected_count += 10;
            }
        }

        let final_scan = table.scan(ts_compact).count();

        assert_eq!(
            final_scan, expected_count,
            "Should have {} vertices after compact/delete in cycle {}",
            expected_count, cycle
        );
    }
}

#[test]
fn test_compact_id_consistency() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    let _ids = [
        table
            .insert("v0", &[("name".to_string(), Value::string("Alice"))], 100)
            .unwrap(),
        table
            .insert("v2", &[("name".to_string(), Value::string("Bob"))], 100)
            .unwrap(),
        table
            .insert("v4", &[("name".to_string(), Value::string("Charlie"))], 100)
            .unwrap(),
        table
            .insert("v5", &[("name".to_string(), Value::string("David"))], 100)
            .unwrap(),
        table
            .insert("v8", &[("name".to_string(), Value::string("Eve"))], 100)
            .unwrap(),
    ];

    table.delete("v2", 200).unwrap();
    table.delete("v5", 200).unwrap();

    let before_count = table.scan(150).count();
    assert_eq!(before_count, 5);

    table.compact_coordinated().expect("compact should succeed");

    if cfg!(debug_assertions) {
        table.verify_invariants().unwrap();
    }

    let after_count = table.scan(200).count();
    assert_eq!(after_count, 3);

    for (key, expected_name) in &[("v0", "Alice"), ("v4", "Charlie"), ("v8", "Eve")] {
        let internal_id = table
            .get_internal_id(key, 200)
            .unwrap_or_else(|| panic!("should find {}", key));
        let record = table
            .get_by_internal_id(internal_id, 200)
            .unwrap_or_else(|| panic!("should retrieve {}", key));

        let name_val = record
            .properties
            .iter()
            .find(|(n, _)| n == "name")
            .map(|(_, v)| v);

        assert_eq!(
            name_val,
            Some(&Value::string(expected_name)),
            "Name should be preserved for {}",
            key
        );
    }

    assert_eq!(
        table.id_indexer.len(),
        table.timestamps.size(),
        "id_indexer and timestamps must have same size"
    );
    assert_eq!(
        table.columns.row_count(),
        table.id_indexer.len(),
        "columns row_count must match id_indexer size"
    );
}

#[test]
fn test_vertex_snapshot_isolation() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    let snap1 = table.register_snapshot(100).unwrap();
    assert_eq!(table.active_snapshot_count(), 1);
    assert_eq!(table.min_active_snapshot_ts(), 100);

    table
        .update_property(0, "name", &Value::string("Alice Updated"), 200)
        .unwrap();

    table.delete("v1", 300).unwrap();

    assert!(table.get_by_internal_id(0, 100).is_some());
    assert!(table.get_internal_id("v1", 300).is_none());

    table.unregister_snapshot(snap1).unwrap();
    assert_eq!(table.active_snapshot_count(), 0);
    assert_eq!(table.min_active_snapshot_ts(), Timestamp::MAX);
}

#[test]
fn test_vertex_multiple_snapshots() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    let snap1 = table.register_snapshot(100).unwrap();
    assert_eq!(table.min_active_snapshot_ts(), 100);

    table
        .insert("v2", &[("name".to_string(), Value::string("Bob"))], 150)
        .unwrap();

    let snap2 = table.register_snapshot(200).unwrap();
    assert_eq!(table.active_snapshot_count(), 2);
    assert_eq!(table.min_active_snapshot_ts(), 100);

    table.delete("v1", 250).unwrap();

    let v1_at_snap1 = table.get_by_internal_id(0, 100);
    assert!(v1_at_snap1.is_some());

    let v1_at_snap2 = table.get_by_internal_id(0, 200);
    assert!(v1_at_snap2.is_some());

    assert!(table.get_by_internal_id(0, 300).is_none());

    table.unregister_snapshot(snap1).unwrap();
    assert_eq!(table.active_snapshot_count(), 1);
    assert_eq!(table.min_active_snapshot_ts(), 200);

    table.unregister_snapshot(snap2).unwrap();
    assert_eq!(table.active_snapshot_count(), 0);
    assert_eq!(table.min_active_snapshot_ts(), Timestamp::MAX);
}

#[test]
fn test_vertex_concurrent_snapshots_same_timestamp() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    let snap1 = table.register_snapshot(100).unwrap();
    let snap2 = table.register_snapshot(100).unwrap();

    assert_eq!(table.active_snapshot_count(), 1);
    assert_eq!(table.min_active_snapshot_ts(), 100);

    assert_ne!(snap1.id, snap2.id);
    assert_eq!(snap1.ts, snap2.ts);

    table.unregister_snapshot(snap1).unwrap();
    assert_eq!(table.active_snapshot_count(), 1);
    assert_eq!(table.min_active_snapshot_ts(), 100);

    table.unregister_snapshot(snap2).unwrap();
    assert_eq!(table.active_snapshot_count(), 0);
    assert_eq!(table.min_active_snapshot_ts(), Timestamp::MAX);
}

#[test]
fn test_vertex_gc_placeholder() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    let cleaned = table.gc(200).unwrap();
    assert_eq!(cleaned, 0);

    assert!(table.get_by_internal_id(0, 100).is_some());
}

#[test]
fn test_vertex_mvcc_table_ops() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    let snap = table.register_snapshot(100).unwrap();
    assert_eq!(table.active_snapshot_count(), 1);

    table.unregister_snapshot(snap).unwrap();
    assert_eq!(table.active_snapshot_count(), 0);

    let gc_count = table.gc(200).unwrap();
    assert_eq!(gc_count, 0);
}

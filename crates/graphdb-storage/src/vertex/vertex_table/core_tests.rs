use super::*;
use crate::types::StoragePropertyDef;
use graphdb_core::error::storage::StorageErrorKind;
use graphdb_core::types::Timestamp;
use graphdb_core::DataType;

/// Test-only extension trait for invariant verification.
trait VerifyInvariants {
    fn verify_invariants(&self) -> StorageResult<()>;
}

impl VerifyInvariants for VertexTable {
    /// Verify internal consistency after compaction.
    ///
    /// Invariants checked:
    /// 1. Every key in id_indexer has a valid timestamp entry
    /// 2. Every valid timestamp entry has a corresponding key in id_indexer
    /// 3. Column count matches id_indexer.len()
    fn verify_invariants(&self) -> StorageResult<()> {
        let id_count = self.id_indexer.len();

        // Check 1: Every key in id_indexer has a valid timestamp entry
        for (key, idx) in self.id_indexer.iter() {
            let start_ts = self.timestamps.get_start_ts(idx);
            if start_ts.is_none() {
                return Err(StorageError::new(
                    StorageErrorKind::StorageError,
                    format!("ID {} for key {:?} missing in timestamps", idx, key),
                ));
            }
        }

        // Check 2: Every valid timestamp entry has a corresponding key in id_indexer
        for idx in 0..self.timestamps.size() {
            if let Some(_start_ts) = self.timestamps.get_start_ts(idx as u32) {
                let key = self.id_indexer.get_key(idx as u32);
                if key.is_none() {
                    return Err(StorageError::new(
                        StorageErrorKind::StorageError,
                        format!("Timestamp entry {} missing in id_indexer", idx),
                    ));
                }
            }
        }

        // Check 3: Column count matches id_indexer.len()
        if self.columns.row_count() != id_count {
            return Err(StorageError::new(
                StorageErrorKind::StorageError,
                format!(
                    "Column count ({}) mismatch with id_indexer.len() ({})",
                    self.columns.row_count(),
                    id_count
                ),
            ));
        }

        Ok(())
    }
}

fn new_table(label: LabelId, label_name: &str, schema: VertexSchema) -> VertexTable {
    VertexTable::with_config(
        label,
        label_name.to_string(),
        schema,
        VertexTableConfig::default(),
    )
}

/// Single-step schema helpers over the staged machine for test setup.
fn staged_add(table: &mut VertexTable, prop: StoragePropertyDef) -> StorageResult<()> {
    table.prepare_add_property_staged(prop)?;
    table.fill_pending_schema_change()?;
    table.publish_pending_schema_change()
}

fn staged_remove(table: &mut VertexTable, name: &str) -> StorageResult<()> {
    table.prepare_remove_property_staged(name)?;
    table.fill_pending_schema_change()?;
    table.publish_pending_schema_change()
}

fn staged_rename(table: &mut VertexTable, old_name: &str, new_name: &str) -> StorageResult<()> {
    table.prepare_rename_property_staged(old_name, new_name)?;
    table.fill_pending_schema_change()?;
    table.publish_pending_schema_change()
}

fn create_test_schema() -> VertexSchema {
    // The first property is the primary key, which mirrors the external
    // id: inserts below omit it and let the table auto-fill the mirror.
    VertexSchema {
        label_id: 0,
        label_name: "person".to_string(),
        properties: vec![
            StoragePropertyDef::new("id".to_string(), DataType::String),
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
    // `id` is auto-filled from the external key on top of the two payloads.
    assert_eq!(record.properties.len(), 3);
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

    let ids = table.live_ids(100);
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
fn test_live_ids_at_excludes_timestamp_deleted_rows() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();
    table
        .insert("v2", &[("name".to_string(), Value::string("Bob"))], 100)
        .unwrap();

    table.delete("v1", 200).unwrap();

    // Snapshot enumeration hides the row deleted at 200.
    assert_eq!(table.live_ids(150), vec![0, 1]);
    assert_eq!(table.live_ids(200), vec![1]);
    assert_eq!(table.scan(200).count(), 1);
}

#[test]
fn test_rename_and_remove_property() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    staged_add(
        &mut table,
        StoragePropertyDef::new("city".to_string(), DataType::String),
    )
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

    staged_rename(&mut table, "age", "years").expect("rename should succeed");
    staged_remove(&mut table, "city").expect("remove should succeed");

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
        vec!["id", "name", "years"]
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

    staged_add(
        &mut table,
        StoragePropertyDef::new("email".to_string(), DataType::String),
    )
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

    staged_remove(&mut table, "email").expect("remove_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, v1 + 1, "Version should increment after remove_property");
}

#[test]
fn test_rename_property_increments_version() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    let v1 = table.schema().schema_version;

    staged_rename(&mut table, "name", "full_name").expect("rename_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, v1 + 1, "Version should increment after rename_property");
}

#[test]
fn test_sequential_property_modifications() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    assert_eq!(table.schema().schema_version, 1);

    staged_add(
        &mut table,
        StoragePropertyDef::new("email".to_string(), DataType::String),
    )
    .expect("add_property 1 should succeed");
    assert_eq!(table.schema().schema_version, 2);

    staged_add(
        &mut table,
        StoragePropertyDef::new("phone".to_string(), DataType::String),
    )
    .expect("add_property 2 should succeed");
    assert_eq!(table.schema().schema_version, 3);

    staged_rename(&mut table, "email", "email_address").expect("rename_property should succeed");
    assert_eq!(table.schema().schema_version, 4);

    staged_remove(&mut table, "phone").expect("remove_property should succeed");
    assert_eq!(table.schema().schema_version, 5);
}

#[test]
fn test_version_history_add_property() {
    use crate::schema::ChangeDetails;

    let schema = create_test_schema();
    let mut table = new_table(1, "User", schema);

    staged_add(
        &mut table,
        StoragePropertyDef::new("email".to_string(), DataType::String),
    )
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
    use crate::schema::ChangeDetails;

    let mut schema = create_test_schema();
    schema.properties.push(StoragePropertyDef::new(
        "email".to_string(),
        DataType::String,
    ));

    let mut table = new_table(1, "User", schema);

    staged_remove(&mut table, "email").expect("remove_property should succeed");

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
    use crate::schema::ChangeDetails;

    let schema = create_test_schema();
    let mut table = new_table(1, "User", schema);

    staged_rename(&mut table, "name", "full_name").expect("rename_property should succeed");

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
        .compact_with_cutoff_collect_mapping(300)
        .expect("compact_with_cutoff_collect_mapping should succeed")
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

    table
        .update_property(0, "name", &Value::string("Alice Updated"), 200)
        .unwrap();

    table.delete("v1", 300).unwrap();

    assert!(table.get_by_internal_id(0, 100).is_some());
    assert!(table.get_internal_id("v1", 300).is_none());
}

#[test]
fn test_vertex_multiple_snapshots() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    table
        .insert("v2", &[("name".to_string(), Value::string("Bob"))], 150)
        .unwrap();

    table.delete("v1", 250).unwrap();

    let v1_at_snap1 = table.get_by_internal_id(0, 100);
    assert!(v1_at_snap1.is_some());

    let v1_at_snap2 = table.get_by_internal_id(0, 200);
    assert!(v1_at_snap2.is_some());

    assert!(table.get_by_internal_id(0, 300).is_none());
}

#[test]
fn test_vertex_gc_placeholder() {
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);

    table
        .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();

    let (gc_vertices, gc_versions) = table.gc_detailed(200).unwrap();

    let cleaned = gc_vertices + gc_versions;
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

    let (gc_vertices, gc_versions) = table.gc_detailed(200).unwrap();

    let gc_count = gc_vertices + gc_versions;
    assert_eq!(gc_count, 0);
}

#[test]
fn test_repeatable_read_property_updates() {
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

    // T1 opens a snapshot at ts=100 (reads pin the timestamp directly).
    // T2 updates the property at a later timestamp.
    table
        .update_property(0, "age", &Value::Int(31), 200)
        .unwrap();
    table
        .update_property(0, "name", &Value::string("Alice-renamed"), 200)
        .unwrap();

    // T1 re-reads at its snapshot timestamp: it must still see the old values
    // (RepeatableRead), not the values written by T2.
    let t1_read = table.get_by_internal_id(0, 100).unwrap();
    let props: std::collections::HashMap<String, Value> = t1_read.properties.into_iter().collect();
    assert_eq!(props.get("age"), Some(&Value::Int(30)));
    assert_eq!(props.get("name"), Some(&Value::string("Alice")));

    // A newer reader at ts=200 sees the new values.
    let t2_read = table.get_by_internal_id(0, 200).unwrap();
    let props: std::collections::HashMap<String, Value> = t2_read.properties.into_iter().collect();
    assert_eq!(props.get("age"), Some(&Value::Int(31)));
    assert_eq!(props.get("name"), Some(&Value::string("Alice-renamed")));
}

#[test]
fn test_property_version_gc_does_not_break_visible_snapshots() {
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
        .update_property(0, "age", &Value::Int(31), 200)
        .unwrap();
    table
        .update_property(0, "age", &Value::Int(32), 300)
        .unwrap();

    // Active table-level pin keeps versions [.., 300) alive.
    let (gc_vertices, gc_versions) = table.gc_detailed(150).unwrap();

    let removed = gc_vertices + gc_versions;
    // Version chain entries with end_ts <= 150 are reclaimed; the entries
    // covering ts=200.. must survive.
    assert_eq!(
        removed, 0,
        "no versions may be reclaimed while a snapshot is active below them"
    );
    let props_at_200: std::collections::HashMap<String, Value> = table
        .get_by_internal_id(0, 200)
        .unwrap()
        .properties
        .into_iter()
        .collect();
    assert_eq!(props_at_200.get("age"), Some(&Value::Int(31)));

    // With no active snapshots, gc at 250 reclaims everything older.
    let (gc_vertices, gc_versions) = table.gc_detailed(250).unwrap();

    let removed = gc_vertices + gc_versions;
    assert!(
        removed >= 1,
        "old versions should be reclaimed after snapshots drop"
    );
    let props_at_300: std::collections::HashMap<String, Value> = table
        .get_by_internal_id(0, 300)
        .unwrap()
        .properties
        .into_iter()
        .collect();
    assert_eq!(props_at_300.get("age"), Some(&Value::Int(32)));
}

#[test]
fn test_flush_chunk_sidecars_and_chunked_reload() {
    use tempfile::TempDir;
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema.clone());
    for i in 0..20 {
        table
            .insert(
                &format!("v{}", i),
                &[
                    ("name".to_string(), Value::string(format!("n{}", i))),
                    ("age".to_string(), Value::Int(i)),
                ],
                100,
            )
            .unwrap();
    }
    let tmp = TempDir::new().unwrap();
    let shard = tmp.path().join("shard");
    table
        .flush(
            &shard,
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .unwrap();
    // Chunk metadata is written alongside columns.bin without format change.
    assert!(shard.join("columns.bin").exists());
    assert!(shard.join("age.chunks").exists());
    assert!(shard.join("name.chunks").exists());

    let mut reloaded = new_table(0, "person", schema);
    reloaded.load(&shard).unwrap();
    // Schema and stats load eagerly; verify a mid-table read first.
    let rec = reloaded.get_by_internal_id(10, 100).unwrap();
    let props: std::collections::HashMap<String, Value> = rec.properties.into_iter().collect();
    assert_eq!(props.get("age"), Some(&Value::Int(10)));

    // Chunked columns serve point reads after reload.
    let age = reloaded.columns.get_column_mut("age").unwrap();
    age.set_chunk_capacity(8);
    age.materialize_chunks();
    assert!(age.chunk_count() >= 2);
    let age = reloaded.columns.get_column("age").unwrap();
    assert_eq!(age.get(0), Some(Value::Int(0)));
    let rec = reloaded.get_by_internal_id(0, 100).unwrap();
    let props: std::collections::HashMap<String, Value> = rec.properties.into_iter().collect();
    assert_eq!(props.get("age"), Some(&Value::Int(0)));
}

#[test]
fn test_partial_compact_preserves_unmoved_rows() {
    // Regression test: `IdIndexer::compact` only reports moved rows, but the
    // coordinator must carry over every live row. Unmoved rows (old == new)
    // used to vanish from timestamps and columns after a partial compact.
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);
    for (key, name) in [
        ("v0", "Alice"),
        ("v1", "Bob"),
        ("v2", "Carol"),
        ("v3", "Dave"),
        ("v4", "Eve"),
    ] {
        table
            .insert(key, &[("name".to_string(), Value::string(name))], 100)
            .unwrap();
    }
    table.delete("v1", 200).unwrap();
    table.delete("v3", 200).unwrap();

    let (removed, _mapping) = table
        .compact_with_cutoff_collect_mapping(300)
        .expect("partial compact should succeed");
    assert_eq!(removed.len(), 2);

    for (key, name) in [("v0", "Alice"), ("v2", "Carol"), ("v4", "Eve")] {
        let id = table
            .get_internal_id(key, 300)
            .unwrap_or_else(|| panic!("{} lost after partial compact", key));
        let record = table
            .get_by_internal_id(id, 300)
            .unwrap_or_else(|| panic!("no record for {} after partial compact", key));
        let got = record
            .properties
            .iter()
            .find(|(n, _)| n == "name")
            .map(|(_, v)| v.clone());
        assert_eq!(got, Some(Value::string(name)), "property lost for {}", key);
    }

    if cfg!(debug_assertions) {
        table.verify_invariants().unwrap();
    }
    assert_eq!(table.columns.row_count(), table.id_indexer.len());
    assert_eq!(table.id_indexer.len(), table.timestamps.size());
}

#[test]
fn test_compact_preserves_moved_row_history() {
    // A row that moves during compaction must keep its before-image chain:
    // historical snapshot reads have to see pre-update values afterwards.
    let schema = create_test_schema();
    let mut table = new_table(0, "person", schema);
    table
        .insert("tmp", &[("name".to_string(), Value::string("Tmp"))], 100)
        .unwrap();
    table
        .insert("v0", &[("name".to_string(), Value::string("Alice"))], 100)
        .unwrap();
    let v0 = table.get_internal_id("v0", 100).expect("v0 exists");
    table
        .update_property(v0, "name", &Value::string("Alice2"), 200)
        .unwrap();
    table.delete("tmp", 250).unwrap();

    let (_, mapping) = table
        .compact_with_cutoff_collect_mapping(300)
        .expect("compact should succeed");
    assert!(!mapping.is_empty(), "expected rows to move");

    let id = table.get_internal_id("v0", 300).expect("v0 survives");
    let name_at = |ts: Timestamp| {
        table
            .get_by_internal_id(id, ts)
            .unwrap_or_else(|| panic!("no record for v0 at {}", ts))
            .properties
            .iter()
            .find(|(n, _)| n == "name")
            .map(|(_, v)| v.clone())
    };
    assert_eq!(name_at(150), Some(Value::string("Alice")));
    assert_eq!(name_at(300), Some(Value::string("Alice2")));
}

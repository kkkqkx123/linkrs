//! Tests for sharded write scopes: staged insert/update/delete, commit
//! and undo semantics, and bulk import invariants.

use super::*;
use crate::types::StoragePropertyDef;
use crate::vertex::VertexSchema;
use linkrs_core::DataType;

fn test_schema() -> VertexSchema {
    VertexSchema {
        label_id: 7,
        label_name: "scoped".into(),
        properties: vec![StoragePropertyDef::new("name".into(), DataType::String)],
        primary_key_index: 0,
        schema_version: 1,
    }
}

fn props(name: &str) -> Vec<(std::sync::Arc<str>, Value)> {
    vec![("name".into(), Value::from(name))]
}

#[test]
fn scoped_insert_is_self_consistent_and_older_snapshots_miss() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let write_ts: Timestamp = 100;
    let mut scope = WriteScope::new(write_ts);
    table
        .insert_with_scope("k1", &props("k1"), write_ts, &mut scope)
        .unwrap();
    // Staged but unapplied: the global area still misses the key.
    assert_eq!(table.lookup_pk("k1", write_ts), PkLookup::Missing);
    let applied = table
        .commit_write_scope_tracked(&mut scope, write_ts)
        .unwrap()
        .mapping;
    assert_eq!(applied.len(), 1);
    let global = applied[0].1;
    assert!(table.get_by_internal_id_offline(global, write_ts).is_some());
    assert_eq!(table.lookup_pk("absent", write_ts), PkLookup::Missing);
    assert_eq!(
        table.lookup_pk("k1", write_ts - 1),
        PkLookup::Missing,
        "older snapshots miss committed keys by timestamp ordering"
    );
    assert!(scope.is_empty());
    assert!(table.get_by_internal_id_offline(global, write_ts).is_some());
}

#[test]
fn same_scope_duplicate_fails_without_extra_allocation() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let mut scope = WriteScope::new(ts);
    table
        .insert_with_scope("dup", &props("dup"), ts, &mut scope)
        .unwrap();
    assert!(table
        .insert_with_scope("dup", &props("dup"), ts, &mut scope)
        .is_err());
    table.commit_write_scope_tracked(&mut scope, ts).unwrap();
    assert_eq!(table.approximate_total_count(), 1);
}

#[test]
fn cross_scope_conflict_fails_at_commit_and_allocates_once() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let mut first = WriteScope::new(ts);
    let mut second = WriteScope::new(ts);
    table
        .insert_with_scope("hot", &props("hot"), ts, &mut first)
        .unwrap();
    table
        .insert_with_scope("hot", &props("hot"), ts, &mut second)
        .unwrap();
    assert!(table.commit_write_scope_tracked(&mut first, ts).is_ok());
    assert!(table.commit_write_scope_tracked(&mut second, ts).is_err());
    assert_eq!(table.approximate_total_count(), 1);
    assert!(second.is_empty());
}

#[test]
fn rollback_discards_staged_rows_without_global_writes() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let mut scope = WriteScope::new(ts);
    table
        .insert_with_scope("tmp", &props("tmp"), ts, &mut scope)
        .unwrap();
    table.rollback_write_scope(&mut scope, ts);
    assert!(scope.is_empty());
    assert_eq!(table.approximate_total_count(), 0);
    assert_eq!(table.lookup_pk("tmp", ts), PkLookup::Missing);
}

#[test]
fn reserved_ids_recycle_through_release_and_bind_at_commit() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let key = IdKey::Text("x".into());
    let first = table.reserve_vertex_id(&key).unwrap();
    table.release_reserved_vertex(first);
    let second = table.reserve_vertex_id(&key).unwrap();
    assert_eq!(second, first, "released reservation is reused");
    let mut scope = WriteScope::new(ts);
    scope
        .stage_insert(7, key.clone(), second, props("x"))
        .unwrap();
    let applied = table
        .commit_write_scope_tracked(&mut scope, ts)
        .unwrap()
        .mapping;
    assert_eq!(applied, vec![(key.clone(), second)]);
    assert_eq!(table.get_internal_id("x", ts), Some(second));
    // Releasing a bound id is a no-op: the live row keeps its slot and
    // re-reserving a bound key reports the existing id.
    table.release_reserved_vertex(second);
    assert_eq!(table.reserve_vertex_id(&key).unwrap(), second);
}

#[test]
fn over_limit_scoped_write_is_rejected_before_global_mutation() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let mut scope = WriteScope::new(ts);
    for index in 0..crate::vertex::MAX_WRITE_SCOPE_KEYS {
        scope
            .stage_insert(7, IdKey::Int(index as i64), index as u32, props("x"))
            .expect("prefill within capacity");
    }
    assert!(table
        .insert_with_scope("overflow", &props("overflow"), ts, &mut scope)
        .is_err());
    assert_eq!(table.approximate_total_count(), 0);
}

#[test]
fn concurrent_scoped_same_key_inserts_allocate_once() {
    use std::sync::Arc;
    let table = Arc::new(ShardedVertexTable::with_config(
        7,
        "scoped".into(),
        test_schema(),
        8,
    ));
    let ts: Timestamp = 100;
    let mut handles = Vec::new();
    for _ in 0..8 {
        let table = Arc::clone(&table);
        handles.push(std::thread::spawn(move || {
            let mut scope = WriteScope::new(ts);
            table
                .insert_with_scope("race", &props("race"), ts, &mut scope)
                .expect("staging never touches global state");
            table.commit_write_scope_tracked(&mut scope, ts).map(|_| ())
        }));
    }
    let mut oks = 0usize;
    for handle in handles {
        if handle.join().unwrap().is_ok() {
            oks += 1;
        }
    }
    assert_eq!(oks, 1);
    assert_eq!(table.approximate_total_count(), 1);
}

#[test]
fn scoped_batch_same_key_duplicate_fails_without_extra_allocation() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let mut scope = WriteScope::new(ts);
    let holder = props("dup");
    let rows: Vec<(&str, &[(Arc<str>, Value)])> =
        vec![("dup", holder.as_slice()), ("dup", holder.as_slice())];
    let results = table.insert_batch_str_with_scope(&rows, ts, &mut scope);
    assert_eq!(results.len(), 2);
    let oks = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(oks, 1);
    let applied = table
        .commit_write_scope_tracked(&mut scope, ts)
        .unwrap()
        .mapping;
    assert_eq!(applied.len(), 1);
    assert_eq!(table.approximate_total_count(), 1);
    assert!(scope.is_empty());
}

#[test]
fn scoped_batch_over_limit_rejected_before_global_mutation() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let mut scope = WriteScope::new(ts);
    for index in 0..crate::vertex::MAX_WRITE_SCOPE_KEYS {
        scope
            .stage_insert(7, IdKey::Int(index as i64), index as u32, props("x"))
            .expect("prefill within capacity");
    }
    let holder = props("overflow");
    let rows: Vec<(&str, &[(Arc<str>, Value)])> = vec![("overflow", holder.as_slice())];
    let results = table.insert_batch_str_with_scope(&rows, ts, &mut scope);
    assert!(results[0].is_err());
    assert_eq!(table.approximate_total_count(), 0);
}

#[test]
fn scoped_batch_i64_same_key_duplicate_applies_once() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let mut scope = WriteScope::new(ts);
    let holder = props("42");
    let rows: Vec<(i64, &[(Arc<str>, Value)])> =
        vec![(42, holder.as_slice()), (42, holder.as_slice())];
    let results = table.insert_batch_i64_with_scope(&rows, ts, &mut scope);
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let applied = table
        .commit_write_scope_tracked(&mut scope, ts)
        .unwrap()
        .mapping;
    assert_eq!(applied.len(), 1);
    assert_eq!(table.approximate_total_count(), 1);
    assert!(scope.is_empty());
}

#[test]
fn scoped_update_and_delete_apply_at_commit() {
    use crate::types::StoragePropertyDef;
    use linkrs_core::DataType;
    let schema = VertexSchema {
        label_id: 7,
        label_name: "scoped".into(),
        properties: vec![
            StoragePropertyDef::new("name".into(), DataType::String),
            StoragePropertyDef {
                name: "age".into(),
                data_type: DataType::Int,
                nullable: true,
                default_value: None,
            },
        ],
        primary_key_index: 0,
        schema_version: 1,
    };
    let table = ShardedVertexTable::with_config(7, "scoped".into(), schema, 4);
    let ts: Timestamp = 100;
    let mut scope = WriteScope::new(ts);
    table
        .insert_with_scope("row", &[("age".into(), Value::from(1))], ts, &mut scope)
        .unwrap();
    table.commit_write_scope_tracked(&mut scope, ts).unwrap();
    let global = table.get_internal_id("row", ts).expect("applied");

    let mut scope = WriteScope::new(ts + 1);
    table
        .update_property_with_scope(global, "age", &Value::from(2), ts + 1, &mut scope)
        .unwrap();
    // Staged update is invisible until the commit apply.
    assert_eq!(
        table
            .get_by_internal_id_offline(global, ts + 1)
            .expect("row")
            .properties,
        vec![
            ("name".into(), Value::from("row")),
            ("age".into(), Value::from(1)),
        ]
    );
    table
        .commit_write_scope_tracked(&mut scope, ts + 1)
        .unwrap();
    assert_eq!(
        table
            .get_by_internal_id_offline(global, ts + 1)
            .expect("row")
            .properties,
        vec![
            ("name".into(), Value::from("row")),
            ("age".into(), Value::from(2)),
        ]
    );

    let mut scope = WriteScope::new(ts + 2);
    table.delete_with_scope("row", ts + 2, &mut scope).unwrap();
    assert!(table.get_by_internal_id_offline(global, ts + 2).is_some());
    table
        .commit_write_scope_tracked(&mut scope, ts + 2)
        .unwrap();
    assert!(table.get_by_internal_id_offline(global, ts + 2).is_none());
}

#[test]
fn failed_commit_leaves_no_partial_application() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    // Seed a conflicting row directly.
    table.insert("taken", &props("taken"), ts).unwrap();
    let mut scope = WriteScope::new(ts);
    table
        .insert_with_scope("fresh", &props("fresh"), ts, &mut scope)
        .unwrap();
    table
        .insert_with_scope("taken", &props("taken"), ts, &mut scope)
        .unwrap();
    // Staging accepts both; the commit recheck rejects the taken key and
    // undoes the fresh row applied ahead of it in shard order.
    assert!(table.commit_write_scope_tracked(&mut scope, ts).is_err());
    assert_eq!(table.lookup_pk("fresh", ts), PkLookup::Missing);
    assert_eq!(table.approximate_total_count(), 1);
}

#[test]
fn scoped_insert_rejects_cross_timestamp_reuse() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let mut scope = WriteScope::new(100);
    assert!(table
        .insert_with_scope("k1", &props("k1"), 101, &mut scope)
        .is_err());
    assert_eq!(table.approximate_total_count(), 0);
    assert!(scope.is_empty());
}

#[test]
fn bulk_import_empty_sorted_matches_batched_rows() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let names: Vec<String> = (0..10).map(|i| format!("s_{:02}", i)).collect();
    let holders: Vec<Vec<(Arc<str>, Value)>> = names.iter().map(|n| props(n)).collect();
    let rows: Vec<(&str, &[(Arc<str>, Value)])> = names
        .iter()
        .zip(holders.iter())
        .map(|(n, p)| (n.as_str(), p.as_slice()))
        .collect();
    let count = table
        .bulk_import_str(&rows, ts, true)
        .expect("sorted import");
    assert_eq!(count, 10);
    assert_eq!(table.approximate_total_count(), 10);
    for name in &names {
        assert!(table.get_internal_id(name, ts).is_some());
    }
}

#[test]
fn bulk_import_rejects_nonempty_and_false_sorted_declaration() {
    let table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ts: Timestamp = 100;
    let holder = props("b");
    let rows: Vec<(&str, &[(Arc<str>, Value)])> = vec![("b", holder.as_slice())];
    assert!(table.bulk_import_str(&rows, ts, true).is_ok());
    let holder_b = props("c");
    let rows_b: Vec<(&str, &[(Arc<str>, Value)])> = vec![("c", holder_b.as_slice())];
    assert!(table.bulk_import_str(&rows_b, ts, false).is_err());
    let unsorted_table = ShardedVertexTable::with_config(7, "scoped".into(), test_schema(), 4);
    let ha = props("z");
    let hb = props("a");
    let unsorted: Vec<(&str, &[(Arc<str>, Value)])> =
        vec![("z", ha.as_slice()), ("a", hb.as_slice())];
    assert!(unsorted_table.bulk_import_str(&unsorted, ts, true).is_err());
    assert_eq!(unsorted_table.approximate_total_count(), 0);
}

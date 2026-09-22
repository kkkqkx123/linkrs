use super::common::{create_test_schema, EdgeTable};
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::EdgeStrategy;

#[test]
fn test_staging_batch_commit_is_atomic_on_duplicate() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let committed = table.edge_count();

    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 2, 0, &[], 110);
    batch.stage_insert(0, 3, 0, &[], 110);
    batch.stage_insert(0, 1, 0, &[], 110);
    assert!(table.commit_staging_batch(batch).is_err());

    // The whole batch is rolled back: earlier entries leave no residue.
    assert_eq!(table.edge_count(), committed);
    assert!(!table.has_edge(0, 2, 0, 120));
    assert!(!table.has_edge(0, 3, 0, 120));
    assert!(table.has_edge(0, 1, 0, 120));
}

#[test]
fn test_staging_batch_multi_entry_commits_together() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    let mut batch = EdgeTable::staging_batch();
    assert!(batch.is_empty());
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_insert(0, 2, 0, &[], 100);
    assert!(batch.contains_insert(0, 1, 0));
    batch.stage_delete(0, 1, 0, 150);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 1);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(table.has_edge(0, 2, 0, 200));
    assert_eq!(table.mvcc.total_tombstone_count(), 0);
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_batch_insert_insert_same_key_rejected() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_insert(0, 1, 0, &[], 110);
    assert!(table.commit_staging_batch(batch).is_err());
    assert!(!table.has_edge(0, 1, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
}

#[test]
fn test_batch_delete_insert_same_key_rebuilds() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_delete(0, 1, 0, 150);
    batch.stage_insert(0, 1, 0, &[], 160);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 2);
    assert!(table.has_edge(0, 1, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_batch_insert_delete_same_key_cancels_without_tombstone() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_delete(0, 1, 0, 150);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 0);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert_eq!(table.mvcc.total_tombstone_count(), 0);
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_batch_delete_delete_same_key_idempotent() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_delete(0, 1, 0, 150);
    batch.stage_delete(0, 1, 0, 150);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 1);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
}

#[test]
fn test_batch_single_slot_second_insert_rejected() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Single;
    schema.ie_strategy = EdgeStrategy::Single;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_insert(0, 2, 0, &[], 110);
    assert!(table.commit_staging_batch(batch).is_err());
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(!table.has_edge(0, 2, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
}

#[test]
fn test_staging_prevalidate_rejects_batch_without_side_effects() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let mut batch = EdgeStore::staging_batch();
    batch.stage_insert(0, 2, 0, &[], 110);
    batch.stage_insert(0, 2, 0, &[], 111);
    assert!(table.commit_staging_batch(batch).is_err());
    assert!(!table.has_edge(0, 2, 0, 120));
    assert!(table.has_edge(0, 1, 0, 120));
}

#[test]
fn test_batch_overlay_reads_observe_net_effect_without_committing() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();

    let mut batch = EdgeStore::staging_batch();
    batch.stage_insert(0, 2, 0, &[], 110);
    batch.stage_delete(0, 1, 0, 150);

    // Default reads stay on the committed snapshot.
    assert!(table.has_edge(0, 1, 0, 200));
    assert!(!table.has_edge(0, 2, 0, 200));
    assert!(table.get_edge(0, 2, 0, 200).is_none());

    // Owner-view helpers overlay the batch net effect.
    assert!(!table.has_edge_with_batch(&batch, 0, 1, 0, 200));
    assert!(table.has_edge_with_batch(&batch, 0, 2, 0, 200));
    assert!(table.get_edge_with_batch(&batch, 0, 1, 0, 200).is_none());
    assert!(table.get_edge_with_batch(&batch, 0, 2, 0, 200).is_some());

    let out = table.out_edges_with_batch(0, 200, &batch);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].dst_vid, graphdb_core::types::VertexId::from_int64(2));
    assert!(table.in_edges_with_batch(1, 200, &batch).is_empty());
    let incoming = table.in_edges_with_batch(2, 200, &batch);
    assert_eq!(incoming.len(), 1);

    // Empty batches delegate to the committed fast path.
    let empty = EdgeStore::staging_batch();
    assert!(table.has_edge_with_batch(&empty, 0, 1, 0, 200));
    assert_eq!(
        table.out_edges_with_batch(0, 200, &empty).len(),
        table.out_edges(0, 200).len()
    );
}

#[test]
fn test_delete_edges_batch_applies_many_keys_in_one_commit() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 110).unwrap();
    table.insert_edge(0, 3, 0, &[], 120).unwrap();

    // Two live keys plus one missing key: the missing key is a silent no-op
    // and the reported count covers the applied deletes only.
    let applied = table
        .delete_edges_batch(&[(0, 1, 0), (0, 2, 0), (0, 9, 0)], 150)
        .unwrap();
    assert_eq!(applied, 2);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(!table.has_edge(0, 2, 0, 200));
    assert!(table.has_edge(0, 3, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));

    // Re-deleting tombstoned keys fails the batch like the single path,
    // with the surviving edge untouched.
    let redelete = table.delete_edges_batch(&[(0, 1, 0), (0, 2, 0), (0, 9, 0)], 160);
    assert!(redelete.is_err());
    assert!(table.has_edge(0, 3, 0, 200));
}

#[test]
fn test_insert_edges_batch_on_empty_columnar_table_matches_staging_effects() {
    use crate::edge::{EdgeSchema, RecordForm};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;
    use graphdb_core::Value;
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "rated".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![
            StoragePropertyDef {
                name: "weight".to_string(),
                data_type: DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            },
            StoragePropertyDef {
                name: "note".to_string(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            },
        ],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert_eq!(table.schema.record_form, RecordForm::Columnar);

    let payloads: Vec<Vec<(String, Value)>> = (1..=4u32)
        .map(|dst| vec![("weight".to_string(), Value::Double(f64::from(dst)))])
        .collect();
    let entries: Vec<crate::edge::BatchInsertEntry> = payloads
        .iter()
        .enumerate()
        .map(|(i, props)| (0, (i + 1) as u32, 0, props.as_slice(), 100))
        .collect();
    table.insert_edges_batch(&entries).unwrap();

    // Fast-path effects match the staging path: contiguous ids, per-edge
    // timestamps preserved, properties projected.
    let mut ids: Vec<u64> = Vec::new();
    for dst in 1..=4u32 {
        assert!(table.has_edge(0, dst, 0, 150));
        let record = table.get_edge(0, dst, 0, 150).unwrap();
        assert!(record
            .properties
            .iter()
            .any(|(k, v)| k == "weight" && *v == Value::Double(f64::from(dst))));
        ids.push(table.edge_id_of(0, dst, 0, 150).unwrap().0);
    }
    ids.sort_unstable();
    assert_eq!(ids, vec![0, 1, 2, 3]);

    // A follow-up batch on the now non-empty table commits through staging.
    let extra: Vec<(String, Value)> = vec![("weight".to_string(), Value::Double(9.0))];
    let more: Vec<crate::edge::BatchInsertEntry> = vec![(0, 5, 0, extra.as_slice(), 160)];
    table.insert_edges_batch(&more).unwrap();
    assert!(table.has_edge(0, 5, 0, 200));
    assert_eq!(table.edge_count(), 5);
}

#[test]
fn test_insert_edges_batch_on_empty_table_rejects_without_residue() {
    use crate::edge::{EdgeSchema, RecordForm};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;
    use graphdb_core::Value;
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "rated".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![
            StoragePropertyDef {
                name: "weight".to_string(),
                data_type: DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            },
            StoragePropertyDef {
                name: "note".to_string(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            },
        ],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert_eq!(table.schema.record_form, RecordForm::Columnar);

    let props: Vec<(String, Value)> = vec![("weight".to_string(), Value::Double(1.0))];
    let entries: Vec<crate::edge::BatchInsertEntry> = vec![
        (0, 1, 0, props.as_slice(), 100),
        (0, 1, 0, props.as_slice(), 110),
    ];
    assert!(table.insert_edges_batch(&entries).is_err());
    assert_eq!(table.edge_count(), 0);

    // The rejected fast path leaves the id counter untouched: a clean batch
    // afterwards still assigns ids from zero.
    let clean: Vec<crate::edge::BatchInsertEntry> = vec![(0, 1, 0, props.as_slice(), 120)];
    table.insert_edges_batch(&clean).unwrap();
    assert_eq!(table.edge_id_of(0, 1, 0, 150).unwrap().0, 0);
}

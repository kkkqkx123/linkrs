use super::common::{create_test_schema, EdgeTable};
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm};
use crate::types::StoragePropertyDef;
use graphdb_core::types::{DataType, EdgeId};
use graphdb_core::Value;

#[test]
fn test_csr_timestamps_agree_with_mvcc() {
    // CSR row timestamps are physical replicas of the MVCC authority: every
    // stored entry must carry the same create/delete timestamps in both
    // CSRs and in `edge_timestamps`, plus a matching tombstone when deleted.
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 110).unwrap();
    table.delete_edge(0, 1, 0, 150).unwrap();

    for csr in [&table.out_csr, &table.in_csr] {
        let mut seen = 0;
        for (_src, nbr) in csr.iter_all() {
            let ts = table
                .mvcc
                .edge_timestamps
                .get(&nbr.edge_id)
                .unwrap_or_else(|| panic!("mvcc record missing for {:?}", nbr.edge_id));
            assert_eq!(nbr.delete_ts, ts.delete_ts);
            seen += 1;
        }
        assert_eq!(seen, 2);
    }
    assert_eq!(table.mvcc.deletion_ts_of(EdgeId(0)), Some(150));
    assert!(!table.mvcc.is_edge_deleted(EdgeId(1)));
}

#[test]
fn test_with_gate_methods_hide_foreign_pending_edge() {
    use crate::mvcc_visibility::PendingGate;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let vm = graphdb_transaction::VersionManager::new();
    let foreign = vm.try_next_write_timestamp().expect("pending ts");
    table.insert_edge(0, 1, 0, &[], foreign).unwrap();

    let gate = PendingGate::new(&vm, None);
    assert!(table.get_edge(0, 1, 0, foreign).is_some());
    assert!(table.get_edge_with_gate(0, 1, 0, foreign, &gate).is_none());
    assert!(table.out_edges_with_gate(0, foreign, &gate).is_empty());
    assert!(table.in_edges_with_gate(1, foreign, &gate).is_empty());
    assert!(table.scan_with_gate(foreign, &gate).is_empty());

    vm.commit_ordered(foreign).expect("ordered commit");
    let gate = PendingGate::new(&vm, None);
    assert!(table.get_edge_with_gate(0, 1, 0, foreign, &gate).is_some());
    assert_eq!(table.out_edges_with_gate(0, foreign, &gate).len(), 1);
    assert_eq!(table.in_edges_with_gate(1, foreign, &gate).len(), 1);
    assert_eq!(table.scan_with_gate(foreign, &gate).len(), 1);
}

#[test]
fn test_authority_is_single_truth_for_deletion() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    // Deletion truth lives in the authority record alone.
    table.mvcc.record_deletion(EdgeId(0), 150);
    assert_eq!(table.mvcc.deletion_ts_of(EdgeId(0)), Some(150));
    assert!(!table.mvcc.is_edge_visible(EdgeId(0), 200));
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(table.has_edge(0, 1, 0, 149));
}

#[test]
fn test_single_time_travel_survives_flush_load() {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "spouse".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef::new(
            "weight".to_string(),
            DataType::Double,
        )],
        oe_strategy: EdgeStrategy::Single,
        ie_strategy: EdgeStrategy::Single,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert!(!table.has_edge(1, 2, 0, 99));
    assert!(table.has_edge(1, 2, 0, 100));

    let temp_dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            temp_dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let schema = EdgeSchema {
        label_id: 0,
        label_name: "spouse".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef::new(
            "weight".to_string(),
            DataType::Double,
        )],
        oe_strategy: EdgeStrategy::Single,
        ie_strategy: EdgeStrategy::Single,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut loaded = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    loaded.load(temp_dir.path()).expect("load should succeed");
    assert!(!loaded.has_edge(1, 2, 0, 99));
    assert!(loaded.has_edge(1, 2, 0, 100));
    let edge = loaded.get_edge(1, 2, 0, 100).unwrap();
    assert_eq!(
        edge.properties
            .iter()
            .find(|(k, _)| k == "weight")
            .map(|(_, v)| v),
        Some(&Value::Double(1.5))
    );
    assert_eq!(loaded.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_scan_iterator_applies_limit_while_advancing() {
    use crate::edge::edge_table::iterator::EdgeTableScanIterator;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..=5u32 {
        table.insert_edge(0, dst, 0, &[], 100).unwrap();
    }

    let limited: Vec<_> = EdgeTableScanIterator::with_limit(&table, 200, Some(2)).collect();
    assert_eq!(limited.len(), 2);

    let streamed: Vec<_> = table.iter(200).collect();
    assert_eq!(streamed.len(), 5);
    assert_eq!(table.scan(200).len(), streamed.len());

    let mut partial = table.iter(200);
    assert!(partial.next().is_some());
    assert!(partial.next().is_some());
}

#[test]
fn test_visibility_consistent_across_point_adjacency_and_scan() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());

    assert!(table.get_edge(0, 1, 0, 199).is_some());
    assert_eq!(table.out_edges(0, 199).len(), 1);
    assert_eq!(table.scan(199).len(), 1);

    assert!(table.get_edge(0, 1, 0, 200).is_none());
    assert!(table.out_edges(0, 200).is_empty());
    assert!(table.scan(200).is_empty());
}

#[test]
fn test_single_time_travel_survives_reload() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Single;
    schema.ie_strategy = EdgeStrategy::Single;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(!table.has_edge(0, 1, 0, 99));
    assert!(table.has_edge(0, 1, 0, 100));
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let schema2 = create_test_schema();
    let mut schema2 = schema2;
    schema2.oe_strategy = EdgeStrategy::Single;
    schema2.ie_strategy = EdgeStrategy::Single;
    let mut loaded = EdgeTable::with_config(schema2, EdgeTableConfig::default()).unwrap();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(!loaded.has_edge(0, 1, 0, 99));
    assert!(loaded.has_edge(0, 1, 0, 100));
}

#[test]
fn test_projected_scan_empty_projection_decodes_no_properties() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    let full = table.scan_projected(100, None);
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].properties.len(), 1);
    let empty: Vec<String> = Vec::new();
    let pruned = table.scan_projected(100, Some(empty));
    assert_eq!(pruned.len(), 1);
    assert!(pruned[0].properties.is_empty());
    assert_eq!(table.out_edges_projected(0, 100, Some(&[])).len(), 1);
    assert!(table.out_edges_projected(0, 100, Some(&[]))[0]
        .properties
        .is_empty());
}

#[test]
fn test_limit_nbrs_returns_prefix_without_full_row() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..10u32 {
        table.insert_edge(0, dst, 0, &[], 100).unwrap();
    }
    let limited = table.merged_out_nbrs_with_limit(0, 100, 3);
    assert_eq!(limited.len(), 3);
    let full = table.merged_out_nbrs(0, 100);
    assert_eq!(full.len(), 9);
    assert_eq!(limited, full[..3].to_vec());
}

#[test]
fn test_batch_accessor_reuses_caller_buffer() {
    use crate::edge::Nbr;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..=8u32 {
        table
            .insert_edge(
                0,
                dst,
                0,
                &[("weight".to_string(), Value::Double(dst as f64))],
                100,
            )
            .unwrap();
    }
    let accessor = table.batch_accessor(true, 200);
    let mut buffer: Vec<Nbr> = Vec::new();
    accessor.fill_into(0, &mut buffer);
    assert_eq!(buffer.len(), 8);
    let owned = table.merged_out_nbrs(0, 200);
    assert_eq!(buffer, owned);

    let buffer_ptr = buffer.as_ptr();
    accessor.fill_into(0, &mut buffer);
    assert_eq!(buffer.as_ptr(), buffer_ptr);
    assert_eq!(buffer.len(), 8);

    let mut limited: Vec<Nbr> = Vec::new();
    accessor.fill_limited(0, &mut limited, 3);
    assert_eq!(limited.len(), 3);

    let mut visited = 0usize;
    let mut scratch: Vec<Nbr> = Vec::new();
    accessor.visit_batched(0, &mut scratch, 3, |batch| {
        assert!(batch.len() <= 3);
        visited += batch.len();
        true
    });
    assert_eq!(visited, 8);
    assert!(scratch.is_empty());

    let hit = accessor.lookup(0, 5, 0).expect("point lookup must hit");
    assert_eq!(hit.endpoint, 5);
    assert!(accessor.lookup(0, 99, 0).is_none());
}

#[test]
fn test_visibility_contract_four_rules() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(!table.has_edge(0, 1, 0, 99));
    assert!(table.has_edge(0, 1, 0, 100));
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    assert!(table.has_edge(0, 1, 0, 149));
    assert!(!table.has_edge(0, 1, 0, 150));
    // Same-stamp re-delete is idempotent.
    assert!(!table.delete_edge(0, 1, 0, 150).unwrap());
    // Cross-stamp re-delete is a conflict.
    assert!(table.delete_edge(0, 1, 0, 160).is_err());
}

#[test]
fn test_gated_point_lookup_resolves_visible_generation() {
    use crate::mvcc_visibility::PendingGate;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    // Same key rebuilt: tombstone generation first, live generation after.
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(2.0))], 200)
        .unwrap();

    let vm = graphdb_transaction::VersionManager::new();
    let gate = PendingGate::new(&vm, None);
    // The first physical slot is the old tombstone carrying value 1.0; a
    // first-match lookup would report absence or the stale generation here.
    let record = table
        .get_edge_with_gate(0, 1, 0, 250, &gate)
        .expect("merged gate lookup must find the visible generation");
    assert!(record
        .properties
        .iter()
        .any(|(k, v)| k == "weight" && *v == Value::Double(2.0)));
    let projected = table
        .get_edge_with_gate_projected(0, 1, 0, 250, &gate, Some(&[]))
        .expect("projected variant must resolve the same generation");
    assert!(projected.properties.is_empty());
    assert!(!table.has_edge(0, 1, 0, 175));
    assert!(table.has_edge(0, 1, 0, 250));
}

#[test]
fn test_fused_point_lookup_matches_split_paths() {
    use crate::mvcc_visibility::PendingGate;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 110).unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());

    let vm = graphdb_transaction::VersionManager::new();
    let gate = PendingGate::new(&vm, None);
    // Live, deleted, never-created and pre-creation timestamps: the fused
    // entries must agree with the split record-plus-id lookups exactly.
    for ts in [99, 100, 120, 150, 200] {
        for (src, dst) in [(0, 1), (0, 2), (0, 3)] {
            let split = table.get_edge(src, dst, 0, ts);
            let fused = table.get_edge_with_id(src, dst, 0, ts);
            assert_eq!(fused.is_some(), split.is_some());
            if let (Some((fused_record, fused_id)), Some(split_record)) = (&fused, &split) {
                assert_eq!(fused_record.rank, split_record.rank);
                assert_eq!(fused_record.properties, split_record.properties);
                assert_eq!(*fused_id, table.edge_id_of(src, dst, 0, ts).unwrap());
            } else {
                assert!(table.edge_id_of(src, dst, 0, ts).is_none());
            }
            let split_gated = table.get_edge_with_gate_projected(src, dst, 0, ts, &gate, None);
            let fused_gated = table.get_edge_projected_with_id(src, dst, 0, ts, &gate, None);
            assert_eq!(fused_gated.is_some(), split_gated.is_some());
            if let (Some((fused_record, _)), Some(split_record)) = (&fused_gated, &split_gated) {
                assert_eq!(fused_record.rank, split_record.rank);
                assert_eq!(fused_record.properties, split_record.properties);
            }
        }
    }
}

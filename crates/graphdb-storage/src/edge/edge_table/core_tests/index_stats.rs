use super::common::{create_test_schema, EdgeTable};
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::{EdgeStrategy, RecordForm};
use crate::types::StoragePropertyDef;
use graphdb_core::types::DataType;
use graphdb_core::Value;

#[test]
fn test_revert_delete_fast_path_restores_property_index() {
    use graphdb_core::value::ordered_codec::OrderedCodec;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.enable_property_index(1024).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert!(table.revert_delete_edge(0, 1, 0, 250).unwrap());

    // The fast path must restore index entries like the slow path: at least
    // one live (non-deleted) record for the edge must be present.
    let codec = OrderedCodec::new();
    let lower = codec.encode(&Value::Double(0.0)).unwrap();
    let hits = table.lookup_edges_by_property_range("weight", &lower, &Vec::new());
    assert!(!hits.is_empty());
    let index = table.property_index.as_ref().expect("index enabled");
    let records = index.lookup("weight", &lower, &Vec::new());
    assert!(records
        .iter()
        .any(|(key, record)| { *key == (0, 1, 0) && record.deleted_ts.is_none() }));
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_delete_maintains_property_index() {
    use graphdb_core::value::ordered_codec::OrderedCodec;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.enable_property_index(1024).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    let codec = OrderedCodec::new();
    let lower = codec.encode(&Value::Double(0.0)).unwrap();
    let index = table.property_index.as_ref().expect("index enabled");
    let records = index.lookup("weight", &lower, &Vec::new());
    assert!(!records.is_empty());
    assert!(records
        .iter()
        .all(|(key, record)| *key != (0, 1, 0) || record.deleted_ts.is_some()));
    assert!(table.revert_delete_edge(0, 1, 0, 250).unwrap());
    let hits = table.lookup_edges_by_property_range("weight", &lower, &Vec::new());
    assert!(!hits.is_empty());
}

#[test]
fn test_index_success_records_metrics_without_failures() {
    use graphdb_metrics::{MetricType, StatsManager};
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let stats = std::sync::Arc::new(StatsManager::new());
    table.set_stats_manager(stats.clone());
    table.enable_property_index(1024).unwrap();
    assert_eq!(table.index_failure_count(), 0);
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert_eq!(table.index_failure_count(), 0);
    assert!(stats.get_value(MetricType::NumIndexOperations).unwrap_or(0) > 0);
    assert_eq!(stats.get_value(MetricType::NumIndexErrors).unwrap_or(0), 0);
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert_eq!(table.index_failure_count(), 0);
}

#[test]
fn test_index_failure_counter_drives_threshold_rebuild() {
    use graphdb_metrics::{MetricType, StatsManager};
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let stats = std::sync::Arc::new(StatsManager::new());
    table.set_stats_manager(stats.clone());
    table.enable_property_index(1024).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    table.note_index_result(
        "weight",
        Err(graphdb_core::StorageError::db_error(
            "injected index failure",
        )),
        1,
    );
    table.note_index_result(
        "weight",
        Err(graphdb_core::StorageError::db_error(
            "injected index failure",
        )),
        1,
    );
    assert_eq!(table.index_failure_count(), 2);
    assert_eq!(stats.get_value(MetricType::NumIndexErrors).unwrap_or(0), 2);
    assert!(!table.rebuild_property_index_on_failures(3, 1024).unwrap());
    assert_eq!(table.index_failure_count(), 2);
    assert!(table.rebuild_property_index_on_failures(2, 1024).unwrap());
    assert_eq!(table.index_failure_count(), 0);
    table.reset_index_failures();
    assert_eq!(table.index_failure_count(), 0);
}

#[test]
fn test_bytes_per_edge_reports_measured_zero_without_fallback() {
    let schema = create_test_schema();
    let table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert_eq!(table.out_csr.bytes_per_edge(), 0);
    assert_eq!(table.in_csr.bytes_per_edge(), 0);
    assert_eq!(table.estimate_memory_usage(), 0);
}

#[test]
fn test_pushdown_filters_at_column_scan_layer() {
    use crate::cursor::ScanPredicate;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..=4u32 {
        table
            .insert_edge(
                0,
                dst,
                0,
                &[("weight".to_string(), Value::Double(dst as f64 * 10.0))],
                100,
            )
            .unwrap();
    }
    let equal = vec![ScanPredicate::ColumnEqual {
        column: "weight".to_string(),
        value: Value::Double(20.0),
    }];
    let hits = table.filter_edge_ids(&equal, 200, None);
    assert_eq!(hits.len(), 1);
    let edge_id = hits[0];
    assert!(table.matches_pushdown(edge_id, 200, &equal));

    let range = vec![ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::Double(15.0)),
        upper: Some(Value::Double(35.0)),
        include_lower: true,
        include_upper: true,
    }];
    assert_eq!(table.filter_edge_ids(&range, 200, None).len(), 2);

    let missing = vec![ScanPredicate::ColumnEqual {
        column: "no_such_column".to_string(),
        value: Value::Double(1.0),
    }];
    assert!(table.filter_edge_ids(&missing, 200, None).is_empty());
    assert!(!table.matches_pushdown(edge_id, 200, &missing));

    let all = table.filter_edge_ids(&[], 200, None);
    assert_eq!(all.len(), 4);
}

#[test]
fn test_pushdown_null_cells_never_match() {
    use crate::cursor::ScanPredicate;
    use crate::edge::EdgeSchema;
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "link".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef {
            name: "note".to_string(),
            data_type: DataType::String,
            nullable: true,
            default_value: None,
        }],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table
        .insert_edge(
            0,
            2,
            0,
            &[("note".to_string(), Value::string("hello"))],
            100,
        )
        .unwrap();
    let equal = vec![ScanPredicate::ColumnEqual {
        column: "note".to_string(),
        value: Value::string("hello"),
    }];
    let hits = table.filter_edge_ids(&equal, 200, None);
    assert_eq!(hits.len(), 1);
}

#[test]
fn test_segment_stats_checkpoint_roundtrip_and_pruning() {
    use crate::cursor::ScanPredicate;
    use crate::edge::edge_table::iterator::EdgeTableScanIterator;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..=4u32 {
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
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert!(dir.path().join("segment_stats.bin").exists());
    let snapshot = table.segment_stats_snapshot();
    assert!(!snapshot.is_empty());
    let stats = snapshot.get(&0).expect("group zero stats must exist");
    assert_eq!(stats.live_count, 4);

    let outside = vec![ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::Double(1000.0)),
        upper: None,
        include_lower: true,
        include_upper: true,
    }];
    assert!(!table.segment_may_contain(0, &outside));
    let inside = vec![ScanPredicate::ColumnEqual {
        column: "weight".to_string(),
        value: Value::Double(2.0),
    }];
    assert!(table.segment_may_contain(0, &inside));

    let mut pruned_iter = EdgeTableScanIterator::with_predicates(&table, 200, None, outside);
    assert_eq!(pruned_iter.by_ref().count(), 0);
    let report = pruned_iter.prune_report();
    assert_eq!(report.segments_total, report.segments_pruned);
    assert!(report.prune_rate() > 0.0);

    let selective = vec![ScanPredicate::ColumnEqual {
        column: "weight".to_string(),
        value: Value::Double(3.0),
    }];
    let mut selective_iter = EdgeTableScanIterator::with_predicates(&table, 200, None, selective);
    let rows: Vec<_> = selective_iter.by_ref().collect();
    assert_eq!(rows.len(), 1);
    let report = selective_iter.prune_report();
    assert_eq!(report.rows_scanned, 4);
    assert_eq!(report.rows_filtered, 3);
    assert!((report.filter_rate() - 0.75).abs() < 1e-9);

    let schema = create_test_schema();
    let mut loaded = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 2, 0, 200));
    assert_eq!(loaded.scan(200).len(), 4);
    let restored = loaded.segment_stats_snapshot();
    assert_eq!(restored.len(), snapshot.len());
    assert_eq!(restored[&0].live_count, 4);
    let rows: Vec<_> = loaded.scan(200);
    let reloaded_rows: Vec<_> = table.scan(200);
    assert_eq!(rows.len(), reloaded_rows.len());
}

#[test]
fn test_topology_encoding_report_uses_integer_path() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..=6u32 {
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
    let report = table.topology_encoding_report();
    assert!(!report.is_empty());
    assert!(report
        .iter()
        .any(|(name, _, _, _)| name.contains("neighbor")));
    assert!(report
        .iter()
        .any(|(name, _, _, _)| name.contains("edge_id")));
    for (_, _, plain, encoded) in &report {
        assert!(encoded <= plain);
    }
}

#[test]
fn test_property_fallback_counter_stays_flat_under_normal_load() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .update_edge_property(0, 1, 0, "weight", &Value::Double(2.0), 120)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .unwrap();
    assert_eq!(table.property_fallback_rewrites(), 0);

    // Insurance path: dirt with no group trace rewrites all owners and is
    // counted exactly once per such checkpoint.
    table.properties_dirty = true;
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .unwrap();
    assert_eq!(table.property_fallback_rewrites(), 1);
}

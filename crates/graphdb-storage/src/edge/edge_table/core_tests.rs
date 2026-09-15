use super::*;
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::{EdgeSchema, EdgeStrategy};
use crate::types::StoragePropertyDef;
use graphdb_core::types::{DataType, VertexId};
use graphdb_core::Value;

type EdgeTable = EdgeStore;

fn create_test_schema() -> EdgeSchema {
    EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef::new(
            "weight".to_string(),
            DataType::Double,
        )],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
    }
}

#[test]
fn test_insert_and_get() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    assert!(table.has_edge(0, 1, 0, 100));

    let edge = table.get_edge(0, 1, 0, 100).unwrap();
    assert_eq!(edge.src_vid, VertexId::from_int64(0));
    assert_eq!(edge.dst_vid, VertexId::from_int64(1));
    assert_eq!(edge.properties.len(), 1);
}

#[test]
fn test_rank_distinguishes_parallel_edges() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table
        .insert_edge(0, 1, 10, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 1, 20, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();

    let rank_10 = table.get_edge(0, 1, 10, 100).unwrap();
    let rank_20 = table.get_edge(0, 1, 20, 100).unwrap();
    assert_ne!(rank_10.properties, rank_20.properties);
    assert_eq!(table.out_edges(0, 100).len(), 2);
}

#[test]
fn test_duplicate_insert_is_rejected() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(table.insert_edge(0, 1, 0, &[], 100).is_err());
    assert_eq!(table.out_edges(0, 100).len(), 1);
}

#[test]
fn test_delete_hides_edge_at_and_after_delete_ts() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert!(table.has_edge(0, 1, 0, 199));
    assert!(!table.has_edge(0, 1, 0, 200));
    assert_eq!(table.scan(250).len(), 0);
}

#[test]
fn test_single_segment_has_unique_edge_ids() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for i in 0..50u32 {
        table.insert_edge(0, i + 1, 0, &[], 100).unwrap();
    }
    let nbrs = table.merged_out_nbrs(0, 200);
    assert_eq!(nbrs.len(), 50);
    let mut ids: Vec<u64> = nbrs.iter().map(|nbr| nbr.edge_id.0).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 50);
    assert_eq!(table.scan(200).len(), 50);
}

#[test]
fn test_delete_marks_properties_deleted() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    let dst_key = EdgeTable::edge_endpoint_key(1, 0);
    let nbr = table.out_csr.get_edge(0, dst_key, 100).unwrap();
    let row_idx = table.properties.get_row_for_edge(nbr.edge_id).unwrap();
    assert!(!table.properties.is_deleted_at_row(row_idx));

    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert!(table.properties.is_deleted_at_row(row_idx));
}

#[test]
fn test_revert_delete_restores_properties() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    let dst_key = EdgeTable::edge_endpoint_key(1, 0);
    let nbr = table.out_csr.get_edge(0, dst_key, 100).unwrap();
    let row_idx = table.properties.get_row_for_edge(nbr.edge_id).unwrap();

    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert!(table.properties.is_deleted_at_row(row_idx));

    let reverted = table
        .revert_delete_edge_by_offset(0, 1, 0, 0, 0, 250)
        .unwrap();
    assert!(reverted);
    assert!(!table.properties.is_deleted_at_row(row_idx));

    let edge = table.get_edge(0, 1, 0, 250).unwrap();
    assert_eq!(
        edge.properties
            .iter()
            .find(|(k, _)| k == "weight")
            .map(|(_, v)| v),
        Some(&Value::Double(1.5))
    );
}

#[test]
fn test_edge_property_update_keeps_current_value_only() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    assert!(table
        .update_edge_property(0, 1, 0, "weight", &Value::Double(2.0), 200)
        .unwrap());
    let current = table.get_edge(0, 1, 0, 250).unwrap();
    assert_eq!(
        current
            .properties
            .iter()
            .find(|(k, _)| k == "weight")
            .map(|(_, v)| v),
        Some(&Value::Double(2.0))
    );
}

#[test]
fn test_compact_reclaims_deleted_edge_properties() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert_eq!(table.properties.row_count(), 1);

    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert_eq!(table.properties.row_count(), 1);

    // A bound before the deletion point keeps the row.
    table.compact_properties(150);
    assert_eq!(table.properties.row_count(), 1);

    // A bound at/after the deletion point reclaims the row.
    table.compact_properties(200);
    assert_eq!(table.properties.row_count(), 0);
}

#[test]
fn test_compact_physically_removes_edges_below_gc_bound() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for i in 0..4u32 {
        table.insert_edge(0, i + 1, 0, &[], 100).unwrap();
    }
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert!(table.delete_edge(0, 2, 0, 250).unwrap());
    assert_eq!(table.edge_count(), 2);

    table.mvcc.register_active_snapshot(210);
    let removed = table.compact_csr_only(210, 0.0);
    assert_eq!(removed, 1);
    assert_eq!(table.edge_count(), 2);
    assert!(!table.has_edge(0, 1, 0, 300));
    assert!(!table.has_edge(0, 2, 0, 300));
    table.mvcc.unregister_active_snapshot(210);
}

#[test]
fn test_auto_gc_tombstones() {
    let schema = create_test_schema();
    let mut config = EdgeTableConfig::default();
    config.auto_maintenance.tombstone_gc_threshold = 10;
    config.auto_maintenance.gc_min_serial = 0;
    let mut table = EdgeTable::with_config(schema, config).unwrap();

    for i in 0..20u64 {
        table.insert_edge(0, 1, i as i64, &[], 100).unwrap();
    }
    for i in 0..20u64 {
        assert!(table.delete_edge(0, 1, i as i64, 200).unwrap());
    }
    assert_eq!(table.mvcc.total_tombstone_count(), 20);
    // Pin a GC bound newer than the deletions; the next write-path pass
    // must drop every covered tombstone.
    table.mvcc.register_active_snapshot(300);
    table.insert_edge(0, 999, 0, &[], 300).unwrap();
    assert_eq!(table.mvcc.total_tombstone_count(), 0);
    assert_eq!(table.scan(400).len(), 1);
}

#[test]
fn test_sparse_high_ids_keep_csr_rows_proportional() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table.insert_edge(0, 100_000, 0, &[], 100).unwrap();
    table.insert_edge(50_000, 100_001, 0, &[], 100).unwrap();
    table.insert_edge(100_002, 0, 0, &[], 100).unwrap();

    let max_id = 100_002usize;
    let out_rows = table.out_csr.vertex_capacity();
    let in_rows = table.in_csr.vertex_capacity();

    assert!(out_rows <= ((max_id + 1) as f64 * 1.25).ceil() as usize);
    assert!(in_rows <= ((max_id + 1) as f64 * 1.25).ceil() as usize);
    assert!(out_rows > max_id);
    assert!(in_rows > max_id);
    assert!(table.out_csr.wasted_bytes_estimate() < 64 * 64);
    assert!(table.in_csr.wasted_bytes_estimate() < 64 * 64);
}

#[test]
fn test_row_capacity_assertion_on_compaction() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table.insert_edge(0, 500_000, 0, &[], 100).unwrap();
    table.insert_edge(50_000, 100_001, 0, &[], 100).unwrap();
    table.insert_edge(100_002, 0, 0, &[], 100).unwrap();

    table.delete_edge(0, 50_000, 0, 150).unwrap();
    table.insert_edge(0, 50_001, 1, &[], 160).unwrap();

    let _removed = table.compact_csr_only(200, 0.25);

    let max_src = 100_002usize;
    let max_dst = 500_001usize;
    let out_rows = table.out_csr.vertex_capacity();
    let in_rows = table.in_csr.vertex_capacity();
    let tail = |rows: usize| ((rows as f64) * 1.25).ceil() as usize;

    assert!(
        out_rows <= tail(max_src + 1),
        "out rows {} exceeds 1.25x tail of {}",
        out_rows,
        max_src + 1
    );
    assert!(
        in_rows <= tail(max_dst + 1),
        "in rows {} exceeds 1.25x tail of {}",
        in_rows,
        max_dst + 1
    );
    assert!(
        table.out_csr.wasted_bytes_estimate() < 64 * 64,
        "out CSR wasted memory {} exceeds lazy allocation tolerance",
        table.out_csr.wasted_bytes_estimate()
    );
    assert!(
        table.in_csr.wasted_bytes_estimate() < 64 * 64,
        "in CSR wasted memory {} exceeds lazy allocation tolerance",
        table.in_csr.wasted_bytes_estimate()
    );
}

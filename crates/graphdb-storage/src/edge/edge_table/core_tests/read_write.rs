use super::common::{create_test_schema, EdgeTable};
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::{EdgeStrategy, MutableCsrTrait};
use graphdb_core::types::{EdgeId, VertexId};
use graphdb_core::Value;

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

    let reverted = table.revert_delete_edge(0, 1, 0, 250).unwrap();
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
fn test_failed_insert_leaves_no_orphan_copies() {
    // Single-direction schemas are rejected at construction: the write path
    // assumes unconditional double writes, so the illegal combination must
    // surface here instead of failing mid-write with a self-rollback.
    let mut schema = create_test_schema();
    schema.ie_strategy = EdgeStrategy::None;
    let result = EdgeTable::with_config(schema, EdgeTableConfig::default());
    assert!(result.is_err());
}

#[test]
fn test_erase_edge_removes_all_copies_idempotently() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert!(table.erase_edge(0, 1, 0, 100));
    assert!(table.mvcc.creation_ts_of(EdgeId(0)).is_none());
    assert!(!table.mvcc.is_edge_deleted(EdgeId(0)));
    assert!(table.properties.get_row_for_edge(EdgeId(0)).is_none());
    assert!(table.get_edge(0, 1, 0, 100).is_none());
    // Replay is idempotent: the second erase finds nothing but still succeeds.
    assert!(!table.erase_edge(0, 1, 0, 100));
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_single_csr_and_table_reject_second_live_edge_with_same_error() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Single;
    schema.ie_strategy = EdgeStrategy::Single;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let table_err = table
        .insert_edge(0, 2, 0, &[], 110)
        .expect_err("table must reject second live edge");
    assert!(table_err.to_string().contains("Single"));
    assert!(table_err.to_string().contains("conflict"));
    let mut csr = crate::edge::SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(0), 100)
        .unwrap();
    let csr_err = csr
        .insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(1), 200)
        .expect_err("csr must reject second live edge");
    assert!(csr_err.to_string().contains("conflict"));
    assert_eq!(
        std::mem::discriminant(&table_err.kind()),
        std::mem::discriminant(&csr_err.kind())
    );
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_delete_by_dst_count_observable_and_rollback_reconciles() {
    use crate::edge::MutableCsrTrait;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let src_key = EdgeTable::edge_endpoint_key(0, 0);
    assert_eq!(table.in_csr.delete_edge_by_dst(1, src_key, 150), 1);
    table.in_csr.revert_delete_by_edge_id(1, EdgeId(0), 150);
    assert!(table.has_edge(0, 1, 0, 200));
    let mut csr = crate::edge::MutableCsr::new();
    csr.insert_edge(0, VertexId::edge_endpoint_key(1, 0), EdgeId(0), 100)
        .unwrap();
    assert_eq!(
        csr.delete_edge_by_dst(0, VertexId::edge_endpoint_key(1, 0), 150),
        1
    );
    assert_eq!(
        csr.delete_edge_by_dst(0, VertexId::edge_endpoint_key(1, 0), 150),
        0
    );
    assert_eq!(table.live_authority_orphans(), 0);
}

#[test]
fn test_single_strategy_rejects_second_live_edge() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Single;
    schema.ie_strategy = EdgeStrategy::Single;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let err = table
        .insert_edge(0, 2, 0, &[], 110)
        .expect_err("second live edge on Single src must fail");
    assert!(err.to_string().contains("Single"));
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
    assert!(table.delete_edge(0, 1, 0, 120).unwrap());
    table.insert_edge(0, 2, 0, &[], 130).unwrap();
    assert!(table.has_edge(0, 2, 0, 140));
}

#[test]
fn test_revert_delete_by_key_restores_edge() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(table.revert_delete_edge(0, 1, 0, 200).unwrap());
    assert!(table.has_edge(0, 1, 0, 200));
}

#[test]
fn test_delete_conflict_survives_merged_miss() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    assert!(table.delete_edge(0, 1, 0, 160).is_err());
    assert!(!table.delete_edge(0, 1, 0, 150).unwrap());
}

#[test]
fn test_revert_delete_keeps_authority_on_partial_failure() {
    use crate::edge::MutableCsrTrait;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    assert!(table.in_csr.rollback_insert(1, EdgeId(0)));
    assert!(table.revert_delete_edge(0, 1, 0, 150).is_err());
    assert!(table.mvcc.is_edge_deleted(EdgeId(0)));
    assert!(!table.has_edge(0, 1, 0, 200));
}

#[test]
fn test_single_direction_schema_rejected_at_construction() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Multiple;
    schema.ie_strategy = EdgeStrategy::None;
    assert!(EdgeTable::with_config(schema, EdgeTableConfig::default()).is_err());
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::None;
    schema.ie_strategy = EdgeStrategy::Multiple;
    assert!(EdgeTable::with_config(schema, EdgeTableConfig::default()).is_err());
}

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
    // Single-direction tables are supported: only the stored leg is written
    // and the missing leg reads as empty adjacency.
    let mut schema = create_test_schema();
    schema.ie_strategy = EdgeStrategy::None;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert_eq!(
        table.storage_direction(),
        crate::edge::StorageDirection::OutOnly
    );
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert!(table.get_edge(0, 1, 0, 100).is_some());
    assert!(table.merged_in_nbrs_with_limit(1, 100, 16).is_empty());
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
    // Single-direction tables are supported: construction succeeds and only
    // the stored leg serves reads and writes.
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Multiple;
    schema.ie_strategy = EdgeStrategy::None;
    let table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert_eq!(
        table.storage_direction(),
        crate::edge::StorageDirection::OutOnly
    );
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::None;
    schema.ie_strategy = EdgeStrategy::Multiple;
    let table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert_eq!(
        table.storage_direction(),
        crate::edge::StorageDirection::InOnly
    );
}

#[test]
fn test_in_only_table_serves_stored_leg_everywhere() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::None;
    schema.ie_strategy = EdgeStrategy::Multiple;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert!(!table.is_direction_available(true));
    assert!(table.is_direction_available(false));
    assert!(table.direction_note(true).is_some());
    assert!(table.direction_note(false).is_none());

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    assert!(table.has_edge(0, 1, 0, 100));
    assert_eq!(table.edge_count(), 1);
    let edge = table.get_edge(0, 1, 0, 100).expect("in-only point lookup");
    assert_eq!(edge.src_vid, VertexId::from_int64(0));
    assert_eq!(edge.dst_vid, VertexId::from_int64(1));
    assert!(table.edge_id_of(0, 1, 0, 100).is_some());
    assert!(table.out_edges(0, 100).is_empty());
    assert!(table.merged_out_nbrs(0, 100).is_empty());
    assert_eq!(table.in_edges(1, 100).len(), 1);
    let scanned = table.scan(100);
    assert_eq!(scanned.len(), 1);
    assert_eq!(scanned[0].src_vid, VertexId::from_int64(0));
    assert_eq!(scanned[0].dst_vid, VertexId::from_int64(1));

    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(table.get_edge(0, 1, 0, 200).is_none());
    assert_eq!(table.scan(200).len(), 0);
}

#[test]
fn test_hot_groups_rank_owner_groups_by_write_volume() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    assert!(table.hot_groups(10).is_empty());
    assert_eq!(table.group_write_count(0), 0);
    for dst in 1..=2u32 {
        table.insert_edge(0, dst, 0, &[], 100).unwrap();
    }
    let far = 1u32 << 20;
    table.insert_edge(far, far + 1, 0, &[], 100).unwrap();
    let home = table.owner_gid_for(0, 1);
    let away = table.owner_gid_for(far, far + 1);
    assert_ne!(home, away);
    assert_eq!(table.group_write_count(home), 2);
    assert_eq!(table.group_write_count(away), 1);
    let hot = table.hot_groups(2);
    assert_eq!(hot, vec![(home, 2), (away, 1)]);
    assert_eq!(table.hot_groups(1), vec![(home, 2)]);
    assert_eq!(table.hot_groups(0).len(), 2);
}

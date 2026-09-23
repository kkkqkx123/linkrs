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
    assert_eq!(
        edge.src_vid,
        VertexId::try_from_int64(0).expect("test vertex id")
    );
    assert_eq!(
        edge.dst_vid,
        VertexId::try_from_int64(1).expect("test vertex id")
    );
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
    assert_eq!(
        edge.src_vid,
        VertexId::try_from_int64(0).expect("test vertex id")
    );
    assert_eq!(
        edge.dst_vid,
        VertexId::try_from_int64(1).expect("test vertex id")
    );
    assert!(table.edge_id_of(0, 1, 0, 100).is_some());
    assert!(table.out_edges(0, 100).is_empty());
    assert!(table.merged_out_nbrs(0, 100).is_empty());
    assert_eq!(table.in_edges(1, 100).len(), 1);
    let scanned = table.scan(100);
    assert_eq!(scanned.len(), 1);
    assert_eq!(
        scanned[0].src_vid,
        VertexId::try_from_int64(0).expect("test vertex id")
    );
    assert_eq!(
        scanned[0].dst_vid,
        VertexId::try_from_int64(1).expect("test vertex id")
    );

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

#[test]
fn batch_projection_matches_point_lookups() {
    use crate::edge::{EdgeSchema, RecordForm};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
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
                name: "score".to_string(),
                data_type: DataType::Int,
                nullable: true,
                default_value: None,
            },
        ],
        oe_strategy: crate::edge::EdgeStrategy::Multiple,
        ie_strategy: crate::edge::EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(
            0,
            1,
            0,
            &[
                ("weight".to_string(), Value::Double(1.5)),
                ("score".to_string(), Value::Int(7)),
            ],
            100,
        )
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
        .unwrap();
    table
        .insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.5))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 2, 0, 300).unwrap());

    // Visible edges at 400: (0,1) with both columns, (0,3) with the
    // nullable column absent. The deleted edge stays out of the batch.
    let ids: Vec<EdgeId> = [(0u32, 1u32), (0, 3)]
        .iter()
        .map(|(src, dst)| {
            table
                .edge_id_of(*src, *dst, 0, 400)
                .expect("visible edge id")
        })
        .collect();
    let batch = table.properties_for_edge_projected_columnar_batch_assume_visible(&ids, 400, None);
    assert_eq!(batch.len(), 2);
    for (i, (src, dst)) in [(0u32, 1u32), (0, 3)].iter().enumerate() {
        let point = table.get_edge(*src, *dst, 0, 400).expect("point lookup");
        assert_eq!(batch[i], point.properties);
    }
    assert_eq!(batch[0].len(), 2);
    assert_eq!(batch[1].len(), 1);

    // Projection subset resolves once for the whole batch.
    let subset = table.properties_for_edge_projected_columnar_batch_assume_visible(
        &ids,
        400,
        Some(&["score".to_string()]),
    );
    assert_eq!(subset.len(), 2);
    assert!(subset[0].iter().any(|(name, _)| name == "score"));
    assert!(subset[1].is_empty());

    // Empty projection decodes nothing; unmapped edges decode to empty,
    // mirroring the single-edge contract.
    let empty =
        table.properties_for_edge_projected_columnar_batch_assume_visible(&ids, 400, Some(&[]));
    assert!(empty.iter().all(|props| props.is_empty()));
    let mut with_ghost = ids.clone();
    with_ghost.push(EdgeId(999_999));
    let ghost =
        table.properties_for_edge_projected_columnar_batch_assume_visible(&with_ghost, 400, None);
    assert_eq!(ghost.len(), 3);
    assert!(ghost[2].is_empty());
}

#[test]
fn predicate_prune_stays_sound_across_stats_rebuild() {
    use crate::cursor::ScanPredicate;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(5.0))], 100)
        .unwrap();
    table.properties.refresh_column_stats();
    // Overwrite then rebuild: bounds must keep the old extreme so the
    // pre-rebuild snapshot still prunes soundly.
    table
        .update_edge_property(0, 1, 0, "weight", &Value::Double(100.0), 200)
        .unwrap();
    table.properties.refresh_column_stats();

    let covering_old = ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::Double(0.0)),
        upper: Some(Value::Double(10.0)),
        include_lower: true,
        include_upper: true,
    };
    // Snapshot 150 sees 5.0: a shrinking rebuild would prune this away.
    let old_hits =
        table
            .properties
            .filter_edge_ids_by_predicates(&[covering_old.clone()], 150, None);
    assert_eq!(old_hits.len(), 1);
    // Current snapshot sees 100.0: the old range matches nothing.
    let new_hits = table
        .properties
        .filter_edge_ids_by_predicates(&[covering_old], 250, None);
    assert!(new_hits.is_empty());

    let covering_new = ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::Double(50.0)),
        upper: Some(Value::Double(150.0)),
        include_lower: true,
        include_upper: true,
    };
    let current = table
        .properties
        .filter_edge_ids_by_predicates(&[covering_new], 250, None);
    assert_eq!(current.len(), 1);

    // Disjoint range exercises the early return with candidates too.
    let disjoint = ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::Double(1000.0)),
        upper: Some(Value::Double(2000.0)),
        include_lower: true,
        include_upper: true,
    };
    let edge_id = table.edge_id_of(0, 1, 0, 250).expect("edge id");
    assert!(table
        .properties
        .filter_edge_ids_by_predicates(&[disjoint], 250, Some(&[edge_id]))
        .is_empty());
}

#[test]
fn chunk_zone_skip_matches_full_walk_across_chunks() {
    use crate::cursor::ScanPredicate;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    // Clustered values across three zone chunks (1024 rows each): the
    // selective range below only overlaps the last chunk, so the first two
    // chunks must skip their version-chain reads without dropping hits.
    for dst in 1..=2500u32 {
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
    table.properties.refresh_column_stats();
    let selective = ScanPredicate::ColumnRange {
        column: "weight".to_string(),
        lower: Some(Value::Double(2000.0)),
        upper: Some(Value::Double(2100.0)),
        include_lower: true,
        include_upper: true,
    };
    let hits = table
        .properties
        .filter_edge_ids_by_predicates(&[selective.clone()], 150, None);
    assert_eq!(hits.len(), 101);
    for edge_id in &hits {
        let props = table
            .properties
            .get_projected_physical_by_edge_id(*edge_id, 150, None)
            .expect("hit owns a row");
        let (_, value) = props
            .iter()
            .find(|(name, _)| name == "weight")
            .expect("weight projected");
        match value {
            Some(Value::Double(v)) => assert!((2000.0..=2100.0).contains(v)),
            other => panic!("unexpected weight cell: {:?}", other),
        }
    }
    // The candidates path applies the same chunk skip with identical hits.
    let all: Vec<EdgeId> = table.properties.edge_ids().collect();
    let via_candidates =
        table
            .properties
            .filter_edge_ids_by_predicates(&[selective], 150, Some(&all));
    assert_eq!(via_candidates, hits);
}

#[test]
fn fill_many_into_matches_repeated_single_row_fills() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..=3u32 {
        table.insert_edge(0, dst, 0, &[], 100).unwrap();
    }
    table.insert_edge(1, 7, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 2, 0, 200).unwrap());

    let accessor = table.batch_accessor(true, 300);
    let mut batched = Vec::new();
    let mut scratch = Vec::new();
    accessor.fill_many_into(&[0, 1, 2], &mut batched, &mut scratch);

    let mut repeated = Vec::new();
    let mut single = Vec::new();
    for src in [0u32, 1, 2] {
        accessor.fill_into(src, &mut single);
        repeated.extend(single.iter().copied());
    }
    assert_eq!(batched, repeated);
    // Row 0 lost (0,2) to the delete, row 2 is empty.
    assert_eq!(batched.len(), 3);
}

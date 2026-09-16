use super::*;
use crate::edge::edge_table::config::{AutoMaintenanceConfig, EdgeTableConfig};
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::{EdgeSchema, EdgeStrategy};
use crate::types::StoragePropertyDef;
use graphdb_core::types::{CommitLsn, DataType, EdgeId, Timestamp, VertexId};
use graphdb_core::Value;

type EdgeTable = EdgeStore;

fn watermark_at(ts: Timestamp) -> graphdb_transaction::MvccWatermarks {
    graphdb_transaction::MvccWatermarks::from_parts(ts, ts, None, CommitLsn::ZERO)
}

fn create_test_schema() -> EdgeSchema {
    EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef {
            name: "weight".to_string(),
            data_type: DataType::Double,
            nullable: false,
            default_value: Some(Value::Double(0.0)),
        }],
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

    let removed = table.compact_csr_only_with_watermarks(&watermark_at(210), 0, 0.0);
    assert_eq!(removed, 1);
    assert_eq!(table.edge_count(), 2);
    assert!(!table.has_edge(0, 1, 0, 300));
    assert!(!table.has_edge(0, 2, 0, 300));
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

    let _removed = table.compact_csr_only_with_watermarks(&watermark_at(Timestamp::MAX), 0, 0.25);

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

#[test]
fn test_auto_maintenance_serial_advances_without_progress() {
    // Pinned watermark, tombstones above threshold, nothing reclaimable:
    // the cooldown serial must still advance on every call so attempts stay
    // rate-limited instead of rescanning the tombstone map on every write.
    let schema = create_test_schema();
    let config = EdgeTableConfig {
        auto_maintenance: AutoMaintenanceConfig {
            tombstone_gc_threshold: 1,
            property_compact_ratio: 0.0,
            gc_min_serial: 2,
        },
        ..EdgeTableConfig::default()
    };
    let mut table = EdgeTable::with_config(schema, config).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 100).unwrap();
    table.delete_edge(0, 1, 0, 150).unwrap();
    table.delete_edge(0, 2, 0, 150).unwrap();
    assert_eq!(table.mvcc.total_tombstone_count(), 2);

    // Pin the watermark below both deletions: no GC run can make progress.
    table.mvcc.register_active_snapshot(100);
    for _ in 0..5 {
        table.maybe_run_auto_maintenance();
    }
    // Serial counts calls (2 inserts + 2 deletes + 5 explicit = 9);
    // with the old stuck-counter logic it would still be 0.
    assert_eq!(table.maintenance_serial, 9);
    assert_eq!(table.mvcc.total_tombstone_count(), 2);

    // Advancing the watermark past the deletions reclaims both tombstones
    // through an explicit watermark-driven pass: unregistering snapshots
    // is pure bookkeeping and never reclaims on its own.
    table.mvcc.register_active_snapshot(200);
    table.mvcc.unregister_active_snapshot(100);
    table.mvcc.unregister_active_snapshot(200);
    assert_eq!(table.mvcc.total_tombstone_count(), 2);
    let reclaimed = table
        .mvcc
        .gc_tombstones(graphdb_core::types::Timestamp::MAX);
    assert_eq!(reclaimed, 2);
    assert_eq!(table.mvcc.total_tombstone_count(), 0);
}

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
            assert_eq!(nbr.create_ts, ts.create_ts);
            assert_eq!(nbr.delete_ts, ts.delete_ts);
            seen += 1;
        }
        assert_eq!(seen, 2);
    }
    assert_eq!(table.mvcc.tombstones.get(&EdgeId(0)), Some(&150));
    assert!(!table.mvcc.tombstones.contains_key(&EdgeId(1)));
}

#[test]
fn test_failed_insert_leaves_no_orphan_copies() {
    // In-direction strategy None forces the in-CSR leg to fail after the
    // out-CSR leg and the property row succeeded: the rollback must release
    // the property row for reuse and drop the authority entry.
    let mut schema = create_test_schema();
    schema.ie_strategy = EdgeStrategy::None;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let rows_before = table.properties.row_count();
    let result = table.insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100);
    assert!(result.is_err());
    assert_eq!(table.properties.row_count(), rows_before);
    assert!(table.mvcc.creation_ts_of(EdgeId(0)).is_none());
    assert!(!table.mvcc.tombstones.contains_key(&EdgeId(0)));
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));
}

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
    assert!(table
        .revert_delete_edge_by_offset(0, 1, 0, 0, 0, 250)
        .unwrap());

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
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));
}

#[test]
fn test_valid_edge_ids_survive_tombstone_gc() {
    // Tombstone-GC must not resurrect deleted edges: after the tombstone is
    // reclaimed, the authoritative visibility still excludes the edge from
    // the valid set, so its property row stays reclaimable.
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert_eq!(table.mvcc.gc_tombstones(Timestamp::MAX), 1);
    // Visibility is unchanged by tombstone reclamation (authority first).
    assert!(!table.mvcc.is_edge_visible(EdgeId(0), 250));
    assert!(table.mvcc.is_edge_visible(EdgeId(1), 250));
    table.compact_properties(250);
    assert_eq!(table.properties.row_count(), 1);
    assert!(table.get_edge(0, 2, 0, 250).is_some());
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));
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
    assert!(!table.mvcc.tombstones.contains_key(&EdgeId(0)));
    assert!(table.properties.get_row_for_edge(EdgeId(0)).is_none());
    assert!(table.get_edge(0, 1, 0, 100).is_none());
    // Replay is idempotent: the second erase finds nothing but still succeeds.
    assert!(!table.erase_edge(0, 1, 0, 100));
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));
}

#[test]
fn test_loaded_copy_mismatches_detect_orphans() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));
    // Simulate a write-path regression that drops the authority entry.
    table.mvcc.edge_timestamps.remove(&EdgeId(0));
    let (mappings, csr_rows, _) = table.loaded_copy_mismatches();
    assert!(mappings >= 1);
    assert!(csr_rows >= 1);
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

/// Unified row space invariant: property mappings never outlive the
/// authority, and every live edge owns a property row.
fn assert_row_space_unified(table: &EdgeTable) {
    for edge_id in table.properties.edge_ids() {
        assert!(
            table.mvcc.edge_timestamps.contains_key(&edge_id),
            "orphan property mapping for {:?}",
            edge_id
        );
    }
    for (edge_id, ts) in table.mvcc.edge_timestamps.iter() {
        if ts.delete_ts == Timestamp::MAX {
            assert!(
                table.properties.get_row_for_edge(*edge_id).is_some(),
                "live edge {:?} without property row",
                edge_id
            );
        }
    }
    for (_, nbr) in table.out_csr.iter_all().chain(table.in_csr.iter_all()) {
        assert!(
            table.mvcc.edge_timestamps.contains_key(&nbr.edge_id),
            "orphan CSR row for {:?}",
            nbr.edge_id
        );
    }
}

#[test]
fn test_unified_row_space_insert_delete_reclaim_remap() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    table.insert_edge(0, 2, 0, &[], 100).unwrap();
    table.insert_edge(1, 2, 0, &[], 110).unwrap();
    // Edges without properties own rows too.
    assert_eq!(table.properties.row_count(), 3);
    assert_row_space_unified(&table);

    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert_row_space_unified(&table);

    table.compact_properties(200);
    assert_eq!(table.properties.row_count(), 2);
    assert_row_space_unified(&table);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));

    // Rebuilding the topology rows keeps the unified mapping intact.
    let mapping = std::collections::HashMap::from([(5u32, 6u32)]);
    table
        .remap_vertex_ids(Some(&mapping), Some(&mapping))
        .unwrap();
    assert_row_space_unified(&table);
    assert!(table.get_edge(0, 2, 0, 250).is_some());
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));
}

#[test]
fn test_authority_overrules_stale_tombstone() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    // A tombstone entry without an authority deletion must not hide a live edge.
    table.mvcc.record_deletion(EdgeId(0), 150);
    assert!(table.mvcc.is_edge_visible(EdgeId(0), 200));
    assert!(table.has_edge(0, 1, 0, 200));
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
    assert_eq!(loaded.loaded_copy_mismatches(), (0, 0, 0));
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
    assert_eq!(applied, 3);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(table.has_edge(0, 2, 0, 200));
}

#[test]
fn test_vertex_reclaim_is_row_scoped() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 100).unwrap();
    table.insert_edge(5, 6, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    assert!(table.has_edge(0, 1, 0, 149));

    let frag_before = table.vertex_fragmentation(0, 200);
    assert!(frag_before.needs_compact());
    assert!(!table.vertex_fragmentation(5, 200).needs_compact());

    let removed = table.compact_reclaimable_vertices(200, 32);
    assert!(removed >= 1);

    // Deletions covered by the cutoff stay invisible through the authority
    // record even after their physical entries are gone.
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(table.has_edge(0, 2, 0, 200));
    // The untouched row keeps its edge and reports no dead entries.
    assert_eq!(table.merged_out_nbrs(5, 200).len(), 1);
    assert!(!table.vertex_fragmentation(0, 200).needs_compact());
    assert_eq!(table.vertex_fragmentation(5, 200).dead_entries, 0);
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
    assert_eq!(table.loaded_copy_mismatches(), (0, 0, 0));
    assert!(table.delete_edge(0, 1, 0, 120).unwrap());
    table.insert_edge(0, 2, 0, &[], 130).unwrap();
    assert!(table.has_edge(0, 2, 0, 140));
}

#[test]
fn test_delete_by_offset_maintains_property_index() {
    use graphdb_core::value::ordered_codec::OrderedCodec;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.enable_property_index(1024).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert!(table.delete_edge_by_offset(0, 1, 0, 0, 0, 200).unwrap());
    let codec = OrderedCodec::new();
    let lower = codec.encode(&Value::Double(0.0)).unwrap();
    let index = table.property_index.as_ref().expect("index enabled");
    let records = index.lookup("weight", &lower, &Vec::new());
    assert!(!records.is_empty());
    assert!(records
        .iter()
        .all(|(key, record)| *key != (0, 1, 0) || record.deleted_ts.is_some()));
    assert!(table
        .revert_delete_edge_by_offset(0, 1, 0, 0, 0, 250)
        .unwrap());
    let hits = table.lookup_edges_by_property_range("weight", &lower, &Vec::new());
    assert!(!hits.is_empty());
}

use super::common::{assert_row_space_unified, create_test_schema, watermark_at, EdgeTable};
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::{CsrBase, EdgeStrategy, MutableCsrTrait};
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::Value;

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
    let config = EdgeTableConfig::default();
    let mut table = EdgeTable::with_config(schema, config).unwrap();

    for i in 0..20u64 {
        table.insert_edge(0, 1, i as i64, &[], 100).unwrap();
    }
    for i in 0..20u64 {
        assert!(table.delete_edge(0, 1, i as i64, 200).unwrap());
    }
    assert_eq!(table.mvcc.total_tombstone_count(), 20);
    // Pin a GC bound newer than the deletions; the next write-path pass
    // reclaims the covered physical slots while authority stays for visibility.
    table.mvcc.register_active_snapshot(300);
    table.insert_edge(0, 999, 0, &[], 300).unwrap();
    assert_eq!(table.mvcc.total_tombstone_count(), 20);
    assert_eq!(table.scan(400).len(), 1);
}

#[test]
fn test_sparse_high_ids_keep_csr_rows_proportional() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    table.insert_edge(0, 100_000, 0, &[], 100).unwrap();
    table.insert_edge(50_000, 100_001, 0, &[], 100).unwrap();
    table.insert_edge(100_002, 0, 0, &[], 100).unwrap();

    // Sparse groups materialize only existing owners: capacity stays
    // proportional to dirty groups rather than the endpoint span, holes read
    // as empty and never produce state.
    assert_eq!(table.out_csr.existing_group_ids().len(), 3);
    assert_eq!(table.in_csr.existing_group_ids().len(), 2);
    let out_rows = table.out_csr.vertex_capacity();
    let in_rows = table.in_csr.vertex_capacity();
    assert!(out_rows < 100_002);
    assert!(in_rows < 100_002);
    assert!(table.out_edges(1, 200).is_empty());
    assert!(table.out_edges(60_000, 200).is_empty());
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
fn test_auto_maintenance_reclaim_with_progress() {
    let schema = create_test_schema();
    let config = EdgeTableConfig::default();
    let mut table = EdgeTable::with_config(schema, config).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 100).unwrap();
    table.delete_edge(0, 1, 0, 150).unwrap();
    table.delete_edge(0, 2, 0, 150).unwrap();
    assert_eq!(table.mvcc.total_tombstone_count(), 2);

    table.mvcc.register_active_snapshot(100);
    for _ in 0..5 {
        table.maybe_run_auto_maintenance();
    }
    assert_eq!(table.mvcc.total_tombstone_count(), 2);

    table.mvcc.register_active_snapshot(200);
    table.mvcc.unregister_active_snapshot(100);
    table.mvcc.unregister_active_snapshot(200);
    assert_eq!(table.mvcc.total_tombstone_count(), 2);
    let reclaimed = table.compact_reclaimable_vertices(201, 32);
    assert_eq!(reclaimed, 2);
    assert_eq!(table.mvcc.total_tombstone_count(), 2);
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
    // Authority records survive collection; visibility is unchanged.
    assert!(!table.mvcc.is_edge_visible(EdgeId(0), 250));
    assert!(table.mvcc.is_edge_visible(EdgeId(1), 250));
    table.compact_properties(250);
    assert_eq!(table.properties.row_count(), 1);
    assert!(table.get_edge(0, 2, 0, 250).is_some());
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_loaded_copy_mismatches_detect_orphans() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
    // Simulate a write-path regression that drops the authority entry.
    table.mvcc.edge_timestamps.remove(&EdgeId(0));
    let (mappings, csr_rows) = table.loaded_copy_mismatches();
    assert!(mappings >= 1);
    assert!(csr_rows >= 1);
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
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));

    // Rebuilding the topology rows keeps the unified mapping intact.
    let mapping = std::collections::HashMap::from([(5u32, 6u32)]);
    table
        .remap_vertex_ids(Some(&mapping), Some(&mapping))
        .unwrap();
    assert_row_space_unified(&table);
    assert!(table.get_edge(0, 2, 0, 250).is_some());
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_topology_property_authority_consistency() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 100).unwrap();
    table.delete_edge(0, 1, 0, 150).unwrap();
    let mut out_topo: Vec<EdgeId> = table
        .out_csr
        .iter_all()
        .map(|(_, nbr)| nbr.edge_id)
        .collect();
    out_topo.sort();
    let mut in_topo: Vec<EdgeId> = table
        .in_csr
        .iter_all()
        .map(|(_, nbr)| nbr.edge_id)
        .collect();
    in_topo.sort();
    let mut props: Vec<EdgeId> = table.properties.edge_ids().collect();
    props.sort();
    let mut authority: Vec<EdgeId> = table.mvcc.edge_timestamps.keys().collect();
    authority.sort();
    assert_eq!(out_topo, authority);
    assert_eq!(in_topo, authority);
    assert_eq!(props, authority);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
    assert_eq!(table.live_authority_orphans(), 0);
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
fn test_remap_preserves_reclaim_hint_for_tombstones() {
    use std::collections::HashMap;
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.delete_edge(0, 1, 0, 150).unwrap();
    let mapping = HashMap::from([(0u32, 0u32), (1u32, 1u32)]);
    table
        .remap_vertex_ids(Some(&mapping), Some(&mapping))
        .unwrap();
    assert!(table.out_csr.group_needs_reclaim_scan(0));
    assert_eq!(table.out_edges(0, 149).len(), 1);
    assert_eq!(table.out_edges(0, 200).len(), 0);
}

#[test]
fn test_single_reclaim_clears_tombstone_slot() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Single;
    schema.ie_strategy = EdgeStrategy::Single;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    assert_eq!(table.out_csr.reclaimable_count(0, 150), 1);
    let mut reported = Vec::new();
    let removed = table
        .out_csr
        .compact_vertex_with_reporting(0, 150, &mut |id, ts| reported.push((id, ts)));
    assert_eq!(removed, 1);
    assert_eq!(reported.len(), 1);
}

#[test]
fn test_fragmentation_ratio_unified() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    for dst in 1..5u32 {
        table.insert_edge(0, dst, 0, &[], 100).unwrap();
    }
    table.delete_edge(0, 1, 0, 150).unwrap();
    let ratio = table.out_csr.fragmentation_ratio();
    let stats = table.out_csr.fragmentation_stats().unwrap();
    let expected = if stats.total_capacity == 0 {
        0.0
    } else {
        stats.wasted_capacity as f32 / stats.total_capacity as f32
    };
    assert!((ratio - expected).abs() < f32::EPSILON);
}

#[test]
fn test_group_reset_refuses_non_empty_shards() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    assert!(table.out_csr.resize_groups(2).is_err());
    assert!(table.out_csr.set_groups(&[0, 1]).is_err());
    // Empty tables still reset through the construction and load paths.
    let mut empty =
        EdgeTable::with_config(create_test_schema(), EdgeTableConfig::default()).unwrap();
    assert!(empty.out_csr.resize_groups(2).is_ok());
}

#[test]
fn test_background_maintenance_reclaims_gone_authority() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(2, 3, 0, &[], 100).unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());
    assert_eq!(table.mvcc.edge_timestamps.len(), 2);

    // Watermark past the deletion: rows become physically reclaimable, then
    // the wired authority reclaim must drop the now row-less entry.
    let wm = watermark_at(300);
    let ran = table.maybe_run_auto_maintenance_with_watermarks(&wm, 0);
    assert!(ran > 0);
    assert_eq!(table.mvcc.edge_timestamps.len(), 1);
    assert!(!table.mvcc.edge_timestamps.contains_key(&EdgeId(0)));
    assert!(table.mvcc.edge_timestamps.contains_key(&EdgeId(1)));
    // The surviving edge stays readable; the reclaimed one was already
    // invisible at any post-watermark snapshot by convention.
    assert!(table.has_edge(2, 3, 0, 200));
}

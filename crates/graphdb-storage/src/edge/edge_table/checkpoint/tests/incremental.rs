use super::common::{make_bounded_table, make_table};
use crate::edge::edge_table::checkpoint::{
    out_append_file, out_group_file, props_group_file, ts_group_file,
};
use graphdb_core::Value;

#[test]
fn persistence_live_markers_are_current() {
    // Guards the documented layout in `edge_table::persistence`: the single
    // mutable dump marker is the only version-like negotiation left. It
    // selects the live write mode, not history.
    assert_eq!(
        crate::edge::mutable_csr::serialization::MUTABLE_CSR_FORMAT_VERSION,
        8
    );
}

#[test]
fn clean_groups_are_skipped_on_flush() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let group_zero = dir.path().join(out_group_file(0));
    assert!(group_zero.exists());
    let stamp = group_zero.metadata().unwrap().modified().unwrap();

    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("second flush should succeed");
    assert_eq!(group_zero.metadata().unwrap().modified().unwrap(), stamp);
}

#[test]
fn flush_records_incremental_checkpoint_metrics() {
    use graphdb_metrics::{MetricType, StatsManager};

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let stats = std::sync::Arc::new(StatsManager::new());
    table.set_stats_manager(stats.clone());
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let bytes = stats
        .get_value(MetricType::CheckpointIncrementalBytesFlushed)
        .unwrap_or(0);
    assert!(bytes > 0, "flushed bytes should be recorded");
    assert_eq!(
        stats.get_value(MetricType::CheckpointStrategyIncremental),
        Some(1)
    );
}

#[test]
fn flush_without_metrics_registry_behaves_the_same() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush without a registry should succeed");
    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
}

#[test]
fn insert_only_flush_reports_append_only() {
    use crate::edge::EdgeCheckpointKind;

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    let kind = table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
}

#[test]
fn delete_flush_reports_rebalance() {
    use crate::edge::EdgeCheckpointKind;

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    table.delete_edge(0, 1, 0, 200).unwrap();
    let kind = table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert_eq!(kind, EdgeCheckpointKind::Rebalance);
}

#[test]
fn property_only_update_skips_topology_rewrite() {
    use crate::edge::EdgeCheckpointKind;

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let group_zero = dir.path().join(out_group_file(0));
    let stamp = group_zero.metadata().unwrap().modified().unwrap();

    table
        .update_edge_property(0, 1, 0, "weight", &Value::Double(9.0), 200)
        .expect("property update should succeed");
    assert!(!table.out_csr.column_dirty_group_ids().is_empty());
    let kind = table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
    assert_eq!(group_zero.metadata().unwrap().modified().unwrap(), stamp);
}

#[test]
fn remap_forces_rebalance_checkpoint() {
    use crate::edge::EdgeCheckpointKind;
    use std::collections::HashMap;

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert!(table.out_csr.dirty_group_ids().is_empty());

    let src_mapping: HashMap<u32, u32> = [(5000u32, 2u32)].into_iter().collect();
    let dst_mapping: HashMap<u32, u32> = [(6000u32, 3u32)].into_iter().collect();
    table
        .remap_vertex_ids(Some(&src_mapping), Some(&dst_mapping))
        .expect("remap should succeed");
    assert!(!table.out_csr.dirty_group_ids().is_empty());
    assert!(table.has_edge(2, 3, 0, 200));
    let kind = table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert_eq!(kind, EdgeCheckpointKind::Rebalance);
}

#[test]
fn flush_reports_tombstone_totals_to_registry() {
    use graphdb_metrics::{MetricType, StatsManager};

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    table.delete_edge(0, 1, 0, 200).unwrap();
    let stats = std::sync::Arc::new(StatsManager::new());
    table.set_stats_manager(stats.clone());
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert_eq!(stats.get_value(MetricType::TombstoneCount), Some(1));
    assert!(
        stats
            .get_value(MetricType::TombstoneMemoryBytes)
            .unwrap_or(0)
            > 0
    );
}

#[test]
fn append_only_flush_skips_base_rewrite() {
    use crate::edge::EdgeCheckpointKind;

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    let kind = table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("first flush should succeed");
    assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
    // First flush of a new group writes the base; no sidecar exists yet.
    let base_path = dir.path().join(out_group_file(0));
    assert!(base_path.exists());
    assert!(!dir.path().join(out_append_file(0)).exists());
    let stamp = base_path.metadata().unwrap().modified().unwrap();

    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
        .unwrap();
    let kind = table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("second flush should succeed");
    assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
    // Insert-only groups persist the sidecar alone: the base is untouched.
    assert_eq!(base_path.metadata().unwrap().modified().unwrap(), stamp);
    assert!(dir.path().join(out_append_file(0)).exists());

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(0, 2, 0, 200));
    assert_eq!(loaded.edge_count(), 2);
}

#[test]
fn cumulative_sidecars_survive_two_append_flushes() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("first flush should succeed");
    // Two insert-only batches with a flush each: the second sidecar must
    // accumulate the first, never overwrite it.
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
        .unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("second flush should succeed");
    table
        .insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 120)
        .unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("third flush should succeed");

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(0, 2, 0, 200));
    assert!(loaded.has_edge(0, 3, 0, 200));
    assert_eq!(loaded.edge_count(), 3);
}

#[test]
fn delete_flush_rewrites_base_and_drops_sidecar() {
    use crate::edge::EdgeCheckpointKind;

    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("first flush should succeed");
    table
        .insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 110)
        .unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("append flush should succeed");
    assert!(dir.path().join(out_append_file(0)).exists());

    table.delete_edge(0, 1, 0, 200).unwrap();
    let kind = table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("delete flush should succeed");
    assert_eq!(kind, EdgeCheckpointKind::Rebalance);
    // The base merge absorbs the sidecar: no append file remains.
    assert!(!dir.path().join(out_append_file(0)).exists());

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(!loaded.has_edge(0, 1, 0, 250));
    assert!(loaded.has_edge(0, 2, 0, 250));
    assert!(loaded.has_edge(0, 3, 0, 250));
    assert_eq!(loaded.edge_count(), 2);
}

#[test]
fn small_write_sidecar_stays_proportional_to_dirty_scale() {
    let mut table = make_table();
    for i in 0..200u32 {
        table.insert_edge(i, i + 1000, 0, &[], 100).unwrap();
    }
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("baseline flush should succeed");
    let base_len = std::fs::metadata(dir.path().join(out_group_file(0)))
        .expect("base readable")
        .len();

    table.insert_edge(0, 2001, 0, &[], 110).unwrap();
    table.insert_edge(1, 2002, 0, &[], 110).unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("small flush should succeed");
    let sidecar_len = std::fs::metadata(dir.path().join(out_append_file(0)))
        .expect("sidecar readable")
        .len();
    // Two fresh edges persist as a small delta, not a group rewrite.
    assert!(
        (sidecar_len as f64) < (base_len as f64) / 4.0,
        "sidecar {} must stay far below base {}",
        sidecar_len,
        base_len
    );
    // Region dirt is cleared by the flush that persists it.
    assert!(table.out_csr.dirty_region_ids(0).is_empty());
}

#[test]
fn small_timestamp_write_stays_proportional_to_dirty_owners() {
    let mut table = make_table();
    for i in 0..100u32 {
        table.insert_edge(i, i + 1000, 0, &[], 100).unwrap();
    }
    for i in 5000..5100u32 {
        table.insert_edge(i, i + 1000, 0, &[], 100).unwrap();
    }
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("baseline flush should succeed");
    let baseline_ts: u64 = dir
        .path()
        .read_dir()
        .expect("read dir")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("ts_g"))
        .map(|entry| entry.metadata().map(|meta| meta.len()).unwrap_or(0))
        .sum();
    assert!(baseline_ts > 0);

    table.insert_edge(0, 2001, 0, &[], 110).unwrap();
    table.insert_edge(1, 2002, 0, &[], 110).unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("small flush should succeed");
    let small_ts = std::fs::metadata(dir.path().join(ts_group_file(0)))
        .expect("dirty ts shard readable")
        .len();
    assert!(
        (small_ts as f64) < (baseline_ts as f64),
        "dirty ts shard {} must stay below baseline total {}",
        small_ts,
        baseline_ts
    );
    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 2001, 0, 200));
    assert_eq!(
        loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(200)),
        Some(110)
    );
}

#[test]
fn small_property_write_stays_proportional_to_dirty_owners() {
    let mut table = make_table();
    for i in 0..100u32 {
        table
            .insert_edge(
                i,
                i + 1000,
                0,
                &[("weight".to_string(), Value::Double(1.0))],
                100,
            )
            .unwrap();
    }
    for i in 5000..5100u32 {
        table
            .insert_edge(
                i,
                i + 1000,
                0,
                &[("weight".to_string(), Value::Double(2.0))],
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
        .expect("baseline flush should succeed");
    let baseline_props: u64 = dir
        .path()
        .read_dir()
        .expect("read dir")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("props_g"))
        .map(|entry| entry.metadata().map(|meta| meta.len()).unwrap_or(0))
        .sum();
    assert!(baseline_props > 0);

    table
        .update_edge_property(0, 1000, 0, "weight", &Value::Double(9.0), 200)
        .expect("property update should succeed");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("small flush should succeed");
    let small_props = std::fs::metadata(dir.path().join(props_group_file(0)))
        .expect("dirty props shard readable")
        .len();
    assert!(
        (small_props as f64) < (baseline_props as f64),
        "dirty props shard {} must stay below baseline total {}",
        small_props,
        baseline_props
    );
    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    let record = loaded.get_edge(0, 1000, 0, 200).expect("edge survives");
    assert!(record
        .properties
        .iter()
        .any(|(k, v)| k == "weight" && *v == Value::Double(9.0)));
}

#[test]
fn append_bound_forces_base_merge() {
    let mut table = make_bounded_table(4);
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("baseline flush should succeed");
    for i in 3..8u32 {
        table.insert_edge(0, i, 0, &[], 110).unwrap();
    }
    let before = table.edge_count();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("bound flush should succeed");
    assert!(dir.path().join(out_group_file(0)).exists());
    assert!(!dir.path().join(out_append_file(0)).exists());
    assert_eq!(table.edge_count(), before);
    let mut loaded = make_bounded_table(4);
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(loaded.edge_count(), before);
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(0, 7, 0, 200));
}

#[test]
fn under_bound_stays_sidecar() {
    let mut table = make_bounded_table(16);
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("baseline flush should succeed");
    let base_path = dir.path().join(out_group_file(0));
    assert!(base_path.exists());
    let stamp = base_path.metadata().unwrap().modified().unwrap();
    table.insert_edge(0, 2, 0, &[], 110).unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("small flush should succeed");
    assert!(dir.path().join(out_append_file(0)).exists());
    assert_eq!(base_path.metadata().unwrap().modified().unwrap(), stamp);
    let mut loaded = make_bounded_table(16);
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(loaded.edge_count(), 2);
}

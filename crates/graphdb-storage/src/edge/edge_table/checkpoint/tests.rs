use super::super::core::EdgeStore;
use super::super::persistence;
use super::*;
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::{
    frozen_serving::serving_path_for, CsrVariant, EdgeSchema, EdgeStrategy, RecordForm,
};
use crate::types::StoragePropertyDef;
use graphdb_core::Value;
use std::io::Write as _;

fn make_table() -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef {
            name: "weight".to_string(),
            data_type: graphdb_core::types::DataType::Double,
            nullable: false,
            default_value: Some(Value::Double(0.0)),
        }],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
}

#[test]
fn frozen_flush_writes_serving_and_load_serves_mapped() {
    use graphdb_core::types::Timestamp;
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
    let before: Vec<_> = table
        .out_edges(0, 200)
        .into_iter()
        .map(|e| e.dst_vid)
        .collect();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let serving = serving_path_for(&dir.path().join(out_group_file(0)));
    assert!(
        serving.exists(),
        "flush writes the serving sidecar for frozen groups"
    );

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(
        matches!(loaded.out_csr.group_variant(0), Some(CsrVariant::Mapped(_))),
        "load serves a clean frozen group from the mapping"
    );
    let after: Vec<_> = loaded
        .out_edges(0, 200)
        .into_iter()
        .map(|e| e.dst_vid)
        .collect();
    assert_eq!(after, before);

    // A mapped group unfreezes back to the same logical content.
    loaded.unfreeze_group(true, 0).unwrap();
    let unfrozen: Vec<_> = loaded
        .out_edges(0, 200)
        .into_iter()
        .map(|e| e.dst_vid)
        .collect();
    assert_eq!(unfrozen, before);
}

#[test]
fn missing_serving_falls_back_to_authoritative() {
    use graphdb_core::types::Timestamp;
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
    let before: Vec<_> = table
        .out_edges(0, 200)
        .into_iter()
        .map(|e| e.dst_vid)
        .collect();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let serving = serving_path_for(&dir.path().join(out_group_file(0)));
    std::fs::remove_file(&serving).unwrap();

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(
        matches!(loaded.out_csr.group_variant(0), Some(CsrVariant::Frozen(_))),
        "missing sidecar falls back to the heap frozen form"
    );
    let after: Vec<_> = loaded
        .out_edges(0, 200)
        .into_iter()
        .map(|e| e.dst_vid)
        .collect();
    assert_eq!(after, before);
}

#[test]
fn corrupt_serving_falls_back_to_authoritative() {
    use graphdb_core::types::Timestamp;
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
    let before: Vec<_> = table
        .out_edges(0, 200)
        .into_iter()
        .map(|e| e.dst_vid)
        .collect();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let serving = serving_path_for(&dir.path().join(out_group_file(0)));
    let mut bytes = std::fs::read(&serving).unwrap();
    bytes[0] ^= 0xff;
    std::fs::write(&serving, &bytes).unwrap();

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(
        matches!(loaded.out_csr.group_variant(0), Some(CsrVariant::Frozen(_))),
        "corrupt sidecar falls back to the heap frozen form"
    );
    let after: Vec<_> = loaded
        .out_edges(0, 200)
        .into_iter()
        .map(|e| e.dst_vid)
        .collect();
    assert_eq!(after, before);
}

#[test]
fn mutable_flush_removes_stale_serving() {
    use graphdb_core::types::Timestamp;
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let serving = serving_path_for(&dir.path().join(out_group_file(0)));
    assert!(serving.exists());

    // Unfreezing makes the group mutable again; the next flush rewrites
    // the base and must drop the stale serving file with it.
    table.unfreeze_group(true, 0).unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert!(
        !serving.exists(),
        "mutable bases must not keep a frozen serving file"
    );
}

#[test]
fn serving_state_machine_full_cycle() {
    use graphdb_core::types::Timestamp;
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    let flush = |table: &mut EdgeStore| {
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed")
    };
    let serving = serving_path_for(&dir.path().join(out_group_file(0)));

    // Generate: frozen flush writes a valid sidecar; loads map it.
    flush(&mut table);
    assert!(serving.exists());
    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(matches!(
        loaded.out_csr.group_variant(0),
        Some(CsrVariant::Mapped(_))
    ));
    assert_eq!(loaded.out_edges(0, 200).len(), 2);

    // Expire: corrupt the sidecar; the next load falls back to the authority
    // base and regenerates the cache on flush.
    let mut bytes = std::fs::read(&serving).unwrap();
    bytes[8] ^= 0xff;
    std::fs::write(&serving, &bytes).unwrap();
    let mut expired = make_table();
    expired.load(dir.path()).expect("load should succeed");
    assert!(matches!(
        expired.out_csr.group_variant(0),
        Some(CsrVariant::Frozen(_))
    ));
    assert_eq!(expired.out_edges(0, 200).len(), 2);
    expired
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush regenerates the cache");
    assert!(serving.exists());
    let mut remapped = make_table();
    remapped.load(dir.path()).expect("load should succeed");
    assert!(matches!(
        remapped.out_csr.group_variant(0),
        Some(CsrVariant::Mapped(_))
    ));

    // Delete: unfreezing plus flush drops the stale sidecar with the base.
    table.unfreeze_group(true, 0).unwrap();
    flush(&mut table);
    assert!(!serving.exists());
    assert!(table.audit_copy_drift().is_empty());
}

#[test]
fn persistence_live_markers_are_current() {
    // Guards the documented layout in `edge_table::persistence`: the two
    // mutable dump markers are the only version-like negotiation left, and
    // they select the two live write modes, not history.
    assert_eq!(
        crate::edge::mutable_csr::serialization::MUTABLE_CSR_FORMAT_VERSION,
        8
    );
    assert_eq!(
        crate::edge::mutable_csr::serialization::MUTABLE_CSR_FORMAT_RAW_VERSION,
        9
    );
}

#[test]
fn memory_intent_serving_load_stays_correct() {
    use crate::edge::edge_table::config::MemoryIntent;
    use graphdb_core::types::Timestamp;
    for intent in [
        MemoryIntent::HeapDefault,
        MemoryIntent::ReadServing,
        MemoryIntent::BulkLoad,
    ] {
        let mut config = EdgeTableConfig::default();
        config.memory_intent = intent;
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: graphdb_core::types::DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        };
        let mut table = EdgeStore::with_config(schema.clone(), config).unwrap();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let mut loaded = EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();
        loaded.load(dir.path()).expect("load should succeed");
        assert_eq!(
            loaded.out_edges(0, 200).len(),
            1,
            "{:?} serves reads",
            intent
        );
        assert!(loaded.audit_copy_drift().is_empty());
    }
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
fn dirty_group_roundtrip_preserves_cross_group_edges() {
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
    assert!(dir.path().join(out_group_file(1)).exists());

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(5000, 6000, 0, 200));
    assert_eq!(loaded.edge_count(), 2);
    assert!(loaded.out_csr.dirty_group_ids().is_empty());
    assert!(loaded.in_csr.dirty_group_ids().is_empty());
}

#[test]
fn missing_manifest_is_rejected() {
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    std::fs::create_dir_all(dir.path()).unwrap();
    let mut payload = Vec::new();
    let variant = table.out_csr.group_variant(0).unwrap().clone();
    persistence::serialize_csr(
        &variant,
        crate::persistence::section::EDGE_OUT_CSR,
        &mut payload,
    )
    .unwrap();
    persistence::write_pages_to_file(
        &dir.path().join("out_g0_foreign.bin"),
        &payload,
        crate::compression::DEFAULT_PAGE_SIZE,
        3,
        1,
    )
    .unwrap();

    let mut loaded = make_table();
    let err = loaded
        .load(dir.path())
        .expect_err("missing manifest must be rejected");
    assert!(err.to_string().contains("missing group manifest"));
}

#[test]
fn properties_file_skipped_when_clean() {
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let props = dir.path().join(props_group_file(0));
    assert!(props.exists());
    let stamp = props.metadata().unwrap().modified().unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("second flush should succeed");
    assert_eq!(props.metadata().unwrap().modified().unwrap(), stamp);
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
fn unpublished_column_is_dropped_on_reload() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    // Fill the physical column but never publish: a crash here must be
    // equivalent to aborting the staged change.
    table
        .prepare_add_property("score".to_string(), graphdb_core::DataType::Int, true, None)
        .unwrap();
    table.fill_pending_add_property().unwrap();
    assert!(table.properties.has_property("score"));
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(!loaded.properties.has_property("score"));
    assert!(!loaded.schema.properties.iter().any(|p| p.name == "score"));
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.pending_add_column().is_none());
}

#[test]
fn published_column_survives_reload_with_stats() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(3.0))], 100)
        .unwrap();
    table
        .prepare_add_property(
            "score".to_string(),
            graphdb_core::DataType::Int,
            true,
            Some(Value::Int(7)),
        )
        .unwrap();
    table.fill_pending_add_property().unwrap();
    table.publish_pending_add_property().unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    // A fresh table only knows the published schema when loading.
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![
            StoragePropertyDef::new("weight".to_string(), graphdb_core::types::DataType::Double),
            StoragePropertyDef::new("score".to_string(), graphdb_core::types::DataType::Int),
        ],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut loaded =
        EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.properties.has_property("score"));
    let snapshot = loaded
        .column_stats_snapshot("weight")
        .expect("flushed stats should be queryable");
    assert_eq!(snapshot.row_count, 1);
    assert_eq!(snapshot.min_value, Some(Value::Double(3.0)));
    assert_eq!(snapshot.max_value, Some(Value::Double(3.0)));
}

#[test]
fn encoded_values_survive_reload_with_encoding() {
    let mut table = make_table();
    for i in 0..20 {
        table
            .insert_edge(
                i,
                i + 100,
                0,
                &[("weight".to_string(), Value::Double(i as f64))],
                100,
            )
            .unwrap();
    }
    let encoded = table.encode_property_columns();
    assert!(encoded > 0);
    let before = table.properties.column_encoding_type("weight");
    assert!(before.is_some_and(|enc| enc != crate::encoding::EncodingType::None));
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(loaded.properties.column_encoding_type("weight"), before);
    let record = loaded.get_edge(3, 103, 0, 200).expect("edge survives");
    assert!(record
        .properties
        .iter()
        .any(|(k, v)| k == "weight" && *v == Value::Double(3.0)));
    let snapshot = loaded
        .column_stats_snapshot("weight")
        .expect("flushed stats should be queryable");
    assert_eq!(snapshot.row_count, 20);
    assert!(snapshot.null_count.is_some());
}

#[test]
fn truncated_properties_payload_is_rejected() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let payload = table.properties.dump();
    assert!(!payload.is_empty());
    let mut reloaded = make_table();
    assert!(reloaded
        .properties
        .load(&payload[..payload.len() / 2])
        .is_err());
}

#[test]
fn stable_column_ids_survive_drop_and_reload() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .add_property("score".to_string(), graphdb_core::DataType::Int, true)
        .expect("add score should succeed");
    let score_id = table
        .properties
        .get_property_id("score")
        .expect("score id should exist");
    table
        .remove_property("weight")
        .expect("drop should succeed");
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef::new(
            "score".to_string(),
            graphdb_core::types::DataType::Int,
        )],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut loaded =
        EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(loaded.properties.get_property_id("score"), Some(score_id));
    loaded
        .add_property("extra".to_string(), graphdb_core::DataType::Int, true)
        .expect("add extra should succeed");
    let extra_id = loaded
        .properties
        .get_property_id("extra")
        .expect("extra id should exist");
    assert!(extra_id != score_id);
    assert!(extra_id > score_id);
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
fn successful_flush_loads_consistent_triple() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(0, 2, 0, 200));
    let record = loaded.get_edge(0, 1, 0, 200).expect("edge survives");
    assert!(record
        .properties
        .iter()
        .any(|(k, v)| k == "weight" && *v == Value::Double(1.0)));
    assert_eq!(
        loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(0)),
        Some(100)
    );
    assert_eq!(
        loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(1)),
        Some(110)
    );
    assert_eq!(loaded.edge_count(), 2);
}

#[test]
fn torn_manifest_tail_recovers_new_snapshot() {
    use crate::edge::node_group::TableShardManifest;
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

    // Simulate a crash between the metadata write and the manifest
    // publish: the manifest file carries stale state while the metadata
    // tail already describes the durable groups. The load recovers the
    // new snapshot via the tail instead of mixing or rejecting.
    let manifest_path = dir.path().join(GROUPS_MANIFEST_FILE);
    let bytes = std::fs::read(&manifest_path).expect("manifest readable");
    let mut manifest = TableShardManifest::decode(&bytes).expect("manifest decodes");
    manifest.out_groups.push(9999);
    crate::compression::write_shadow_file(&manifest_path, &manifest.encode())
        .expect("torn manifest writable");

    let mut loaded = make_table();
    loaded
        .load(dir.path())
        .expect("torn commit recovers via tail");
    assert!(loaded.has_edge(0, 1, 0, 200));
}

#[test]
fn meta_without_tail_is_rejected() {
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

    // Strip the manifest commit tail to mimic a torn metadata write:
    // the loader must reject it explicitly.
    let meta_path = dir.path().join("meta.bin");
    let (mut payload, _) = persistence::read_pages_from_file(&meta_path).expect("meta readable");
    assert!(payload.len() > 20);
    payload.truncate(payload.len() - 16);
    persistence::write_pages_to_file(
        &meta_path,
        &payload,
        crate::compression::DEFAULT_PAGE_SIZE,
        3,
        1,
    )
    .expect("torn meta writable");

    let mut loaded = make_table();
    let err = loaded
        .load(dir.path())
        .expect_err("torn meta must be rejected");
    assert!(err.to_string().contains("manifest commit tail"));
}

#[test]
fn crash_before_manifest_publish_recovers_new_snapshot() {
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
    let old_manifest =
        std::fs::read(dir.path().join(GROUPS_MANIFEST_FILE)).expect("old manifest readable");

    table
        .insert_edge(
            5000,
            6000,
            0,
            &[("weight".to_string(), Value::Double(2.0))],
            110,
        )
        .unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("second flush should succeed");

    // Crash between the second metadata write and its manifest publish:
    // restore the old manifest file while the new metadata tail stays.
    // Loading recovers the new snapshot via the tail with topology,
    // properties and timestamps consistent.
    std::fs::write(dir.path().join(GROUPS_MANIFEST_FILE), &old_manifest)
        .expect("manifest restore works");
    let mut loaded = make_table();
    loaded
        .load(dir.path())
        .expect("torn second commit recovers new snapshot");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(5000, 6000, 0, 200));

    // A clean retry of the whole flush from live memory still publishes
    // both files and stays loadable.
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("retry flush should succeed");
    let mut reloaded = make_table();
    reloaded.load(dir.path()).expect("load should succeed");
    assert!(reloaded.has_edge(0, 1, 0, 200));
    assert!(reloaded.has_edge(5000, 6000, 0, 200));
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
fn torn_sidecar_is_rejected_not_replayed() {
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
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
        .unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("append flush should succeed");
    let sidecar = dir.path().join(out_append_file(0));
    assert!(sidecar.exists());
    // Corrupt the sidecar payload (valid pages, broken ops section).
    let (mut raw, _) = persistence::read_pages_from_file(&sidecar).expect("sidecar readable");
    assert!(raw.len() > 32);
    raw.truncate(raw.len() - 4);
    persistence::write_pages_to_file(&sidecar, &raw, crate::compression::DEFAULT_PAGE_SIZE, 3, 1)
        .expect("torn sidecar writable");

    let mut loaded = make_table();
    assert!(
        loaded.load(dir.path()).is_err(),
        "torn sidecar must fail the load"
    );
}

#[test]
fn corrupt_manifest_is_rejected() {
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
    // Truncate the manifest file: the loader must fail closed.
    let current = std::fs::read(dir.path().join(GROUPS_MANIFEST_FILE)).expect("manifest readable");
    std::fs::write(
        dir.path().join(GROUPS_MANIFEST_FILE),
        &current[..current.len() / 2],
    )
    .expect("torn manifest writable");

    let mut loaded = make_table();
    assert!(
        loaded.load(dir.path()).is_err(),
        "corrupt manifest must be rejected"
    );
}

#[test]
fn foreign_properties_file_is_ignored() {
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    std::fs::write(dir.path().join("properties.bin"), b"third-party")
        .expect("foreign file writable");
    let mut loaded = make_table();
    loaded
        .load(dir.path())
        .expect("foreign files must not fail the load");
    assert!(loaded.has_edge(0, 1, 0, 200));
}

#[test]
fn truncated_meta_is_rejected() {
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let meta_path = dir.path().join("meta.bin");
    let (mut payload, _) = persistence::read_pages_from_file(&meta_path).expect("meta readable");
    payload.truncate(payload.len() / 2);
    persistence::write_pages_to_file(
        &meta_path,
        &payload,
        crate::compression::DEFAULT_PAGE_SIZE,
        3,
        1,
    )
    .expect("torn meta writable");
    let mut loaded = make_table();
    assert!(
        loaded.load(dir.path()).is_err(),
        "truncated meta must be rejected"
    );
}

#[test]
fn sparse_endpoints_produce_no_hole_files() {
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(9000, 9001, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    assert!(dir.path().join(out_group_file(0)).exists());
    assert!(dir.path().join(out_group_file(2)).exists());
    assert!(!dir.path().join(out_group_file(1)).exists());
    assert!(!dir.path().join(ts_group_file(1)).exists());
    assert!(!dir.path().join(props_group_file(1)).exists());
    assert!(dir.path().join(ts_group_file(0)).exists());
    assert!(dir.path().join(ts_group_file(2)).exists());
    assert!(dir.path().join(props_group_file(0)).exists());
    assert!(dir.path().join(props_group_file(2)).exists());

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(9000, 9001, 0, 200));
    assert!(loaded.out_edges(5000, 200).is_empty());
    assert_eq!(loaded.edge_count(), 2);
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
fn wal_recovers_committed_unflushed_writes_idempotently() {
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
        .expect("baseline flush should succeed");

    // Committed after the checkpoint, never flushed: redo log owns them.
    table
        .insert_edge(2, 3, 0, &[("weight".to_string(), Value::Double(2.0))], 200)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 210).unwrap());
    drop(table);

    let mut recovered = make_table();
    recovered.load(dir.path()).expect("load replays the log");
    assert!(!recovered.has_edge(0, 1, 0, 300));
    assert!(recovered.has_edge(2, 3, 0, 300));

    // Log survives until the next checkpoint: a repeated replay of the
    // same ops (insert then delete) must land in the identical state.
    let mut second = make_table();
    second.load(dir.path()).expect("second load replays again");
    assert_eq!(second.edge_count(), recovered.edge_count());
    assert!(!second.has_edge(0, 1, 0, 300));
    assert!(second.has_edge(2, 3, 0, 300));
    let (mappings, rows, live_orphans) = second.copy_audit();
    assert_eq!((mappings, rows, live_orphans), (0, 0, 0));

    // Checkpoint truncates the log: afterwards recovery needs no replay.
    second
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .unwrap();
    assert!(!crate::edge::edge_table::wal::wal_path(dir.path()).exists());
}

#[test]
fn torn_edge_wal_tail_rejects_load() {
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .unwrap();
    table.insert_edge(2, 3, 0, &[], 200).unwrap();
    // Simulate a torn tail: an entry header claiming more bytes than the
    // file holds after the last durable commit.
    std::fs::OpenOptions::new()
        .append(true)
        .open(crate::edge::edge_table::wal::wal_path(dir.path()))
        .expect("wal exists after the second commit")
        .write_all(&64u64.to_le_bytes())
        .expect("append torn header");
    drop(table);

    let mut recovered = make_table();
    let err = recovered
        .load(dir.path())
        .expect_err("torn edge WAL tail must fail closed");
    assert!(err.to_string().contains("edge WAL"));
}

#[test]
fn reshard_roundtrip_preserves_snapshot() {
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
        .expect("baseline flush should succeed");
    let stats = table.reshard(9).expect("reshard should succeed");
    assert_eq!(stats.old_bits, 12);
    assert_eq!(stats.new_bits, 9);
    assert_eq!(stats.edges, 2);
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("post-reshard flush should succeed");

    let mut loaded = EdgeStore::with_config(
        crate::edge::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::types::StoragePropertyDef {
                name: "weight".to_string(),
                data_type: graphdb_core::types::DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: crate::edge::EdgeStrategy::Multiple,
            ie_strategy: crate::edge::EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        },
        EdgeTableConfig {
            node_group_bits: 9,
            ..EdgeTableConfig::default()
        },
    )
    .expect("table builds");
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(5000, 6000, 0, 200));
    let record = loaded.get_edge(0, 1, 0, 200).expect("edge survives");
    assert!(record
        .properties
        .iter()
        .any(|(k, v)| k == "weight" && *v == Value::Double(1.0)));
    assert_eq!(loaded.edge_count(), 2);
}

fn make_bounded_table(bound: usize) -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let config = EdgeTableConfig {
        max_append_ops_per_group: bound,
        ..EdgeTableConfig::default()
    };
    EdgeStore::with_config(schema, config).unwrap()
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

#[test]
fn missing_groups_leave_no_files() {
    // Sparse file promise: only materialized groups produce base, append,
    // timestamp, property and serving files. Hole groups must leave nothing
    // behind, and the manifest roundtrip must list existing groups alone.
    let mut table = make_table();
    table
        .insert_edge(1_000_000, 1_000_001, 0, &[], 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("sparse flush should succeed");
    for hole in [1usize, 100, 243] {
        for name in [
            out_group_file(hole),
            in_group_file(hole),
            out_append_file(hole),
            in_append_file(hole),
            ts_group_file(hole as u32),
            props_group_file(hole as u32),
        ] {
            assert!(
                !dir.path().join(&name).exists(),
                "hole group {hole} must leave no file, found {name}"
            );
        }
    }
    // Mutable groups never carry a serving sidecar either.
    assert!(!serving_path_for(&dir.path().join(out_group_file(244))).exists());
    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(
        loaded.out_csr.existing_group_ids(),
        table.out_csr.existing_group_ids()
    );
    assert_eq!(loaded.edge_count(), 1);
    assert!(loaded.audit_copy_drift().is_empty());
}

#[test]
fn reshard_drill_syncs_routes_manifest_and_serving() {
    use graphdb_core::types::Timestamp;
    // Width-change drill: frozen serving exists before the switch, point
    // lookups stay correct immediately after (fresh route cache rides with
    // the rebuilt sets, no reload), the next flush drops the stale serving
    // sidecar, and the rebuilt manifest reloads at the new width.
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
    table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("baseline flush should succeed");
    assert!(serving_path_for(&dir.path().join(out_group_file(0))).exists());

    let stats = table.reshard(9).expect("reshard should succeed");
    assert_eq!(stats.old_bits, 12);
    assert_eq!(stats.new_bits, 9);
    assert_eq!(stats.edges, 2);
    assert_eq!(table.out_csr.group_bits(), 9);
    assert!(table.has_edge(0, 1, 0, 200));
    assert!(table.has_edge(5000, 6000, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
    assert!(table.audit_copy_drift().is_empty());

    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("post-reshard flush should succeed");
    assert!(
        !serving_path_for(&dir.path().join(out_group_file(0))).exists(),
        "pre-reshard serving sidecar must not survive the width change"
    );

    let mut loaded = EdgeStore::with_config(
        crate::edge::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::types::StoragePropertyDef {
                name: "weight".to_string(),
                data_type: graphdb_core::types::DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: crate::edge::EdgeStrategy::Multiple,
            ie_strategy: crate::edge::EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        },
        EdgeTableConfig {
            node_group_bits: 9,
            ..EdgeTableConfig::default()
        },
    )
    .expect("table builds");
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(5000, 6000, 0, 200));
    assert_eq!(loaded.edge_count(), 2);
    assert!(loaded.audit_copy_drift().is_empty());
}

#[test]
fn cross_group_flush_load_audits_clean_with_owner_counts() {
    // Recovery across groups: edges in widely separated groups flush and
    // reload with the owner map fully derived from topology, zero relocated
    // orphans, and an empty drift audit.
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(9000, 9001, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(loaded.edge_count(), 2);
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.has_edge(9000, 9001, 0, 200));
    assert_eq!(loaded.live_authority_orphans(), 0);
    assert!(loaded.audit_copy_drift().is_empty());
    let owner_stats = loaded.rebuild_owner_map_with_stats();
    assert_eq!(owner_stats.mapped, 2);
    assert_eq!(owner_stats.relocated_orphans, 0);
}

use super::common::make_table;
use crate::edge::edge_table::checkpoint::out_group_file;
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::{
    frozen_serving::serving_path_for, CsrVariant, EdgeSchema, EdgeStrategy, RecordForm,
};
use crate::types::StoragePropertyDef;
use graphdb_core::Value;

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

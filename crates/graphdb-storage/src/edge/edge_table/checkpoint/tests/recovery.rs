use super::common::make_table;
use crate::edge::edge_table::checkpoint::{
    in_append_file, in_group_file, out_append_file, out_group_file, props_group_file,
    ts_group_file, GROUPS_MANIFEST_FILE,
};
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::edge_table::persistence;
use crate::edge::{frozen_serving::serving_path_for, RecordForm};
use graphdb_core::Value;
use std::io::Write as _;

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

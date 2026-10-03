use super::super::ShardedVertexTable;
use super::commit_manifest::{
    commit_manifest_checksum, CommitKind, CommitManifestInput, COMMIT_MANIFEST_FILE_NAME,
};
use super::sidecar::SnapshotSidecarRecord;
use super::table_manifest::{
    table_manifest_checksum, TableManifestInput, TABLE_MANIFEST_FILE_NAME,
};
use crate::compression::CompressionType;
use crate::types::StoragePropertyDef;
use graphdb_core::types::Timestamp;
use graphdb_core::{DataType, Value};

fn test_schema() -> crate::vertex::VertexSchema {
    crate::vertex::VertexSchema {
        label_id: 1,
        label_name: "person".to_string(),
        properties: vec![StoragePropertyDef::new(
            "name".to_string(),
            DataType::String,
        )],
        primary_key_index: 0,
        schema_version: 1,
    }
}

fn unique_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "commit_manifest_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

#[test]
fn commit_manifest_pinned_and_strict_on_corrupt() {
    let dir = unique_dir("strict");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            7,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
    assert!(manifest_path.exists());
    let manifest = ShardedVertexTable::read_commit_manifest(&dir)
        .unwrap()
        .expect("commit manifest present");
    assert_eq!(manifest.epoch, 7);
    assert_eq!(manifest.kind, CommitKind::Full);
    assert!(!manifest.files.is_empty());

    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    reloaded.load(&dir).unwrap();
    assert!(reloaded.get_internal_id("v1", ts).is_some());

    let victim = dir.join("shard_0").join("columns.bin");
    if victim.exists() {
        std::fs::write(&victim, b"corrupt").unwrap();
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains('7') && err.contains("shard"),
            "strict error must carry epoch and shard location: {err}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_commit_manifest_refuses_open() {
    let dir = unique_dir("missing-manifest");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert(
            "v_missing",
            &[("name".to_string(), Value::from("v_missing"))],
            ts,
        )
        .unwrap();
    table
        .flush(&dir, CompressionType::Zstd { level: 0 })
        .unwrap();
    std::fs::remove_file(dir.join(COMMIT_MANIFEST_FILE_NAME)).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("commit manifest"),
        "missing manifest must refuse: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn orphan_tmp_cleaned_tolerantly() {
    let dir = unique_dir("orphan");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stray.tmp"), b"x").unwrap();
    std::fs::create_dir_all(dir.join("left.staging")).unwrap();
    ShardedVertexTable::cleanup_orphans(&dir);
    assert!(!dir.join("stray.tmp").exists());
    assert!(!dir.join("left.staging").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offline_inspection_reports_healthy_and_halfway_stores() {
    let dir = unique_dir("health");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            11,
            CommitKind::Full,
            None,
        )
        .unwrap();

    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(report.manifest_present && report.manifest_decodable);
    assert_eq!(report.epoch, Some(11));
    assert_eq!(report.kind.as_deref(), Some("full"));
    assert!(report.missing_files.is_empty());
    assert!(report.is_healthy());

    std::fs::write(dir.join("half.tmp"), b"x").unwrap();
    std::fs::write(dir.join("shard_0").join("page.tmp"), b"x").unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(report.is_healthy());
    assert_eq!(report.orphan_tmp_files.len(), 2);
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    reloaded.load(&dir).unwrap();
    assert!(reloaded.get_internal_id("v1", ts).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fault_matrix_missing_listed_file_refuses_open() {
    let dir = unique_dir("fault-missing");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            13,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest = ShardedVertexTable::read_commit_manifest(&dir)
        .unwrap()
        .expect("manifest");
    let victim = manifest.files.first().expect("listed file").clone();
    std::fs::remove_file(dir.join(&victim)).unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(!report.is_healthy());
    assert_eq!(report.missing_files, vec![victim.clone()]);
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("13") && err.contains("missing"),
        "refusal must carry epoch and cause: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fault_matrix_broken_incremental_falls_back_to_baseline() {
    let base = unique_dir("fault-base");
    let incr = unique_dir("fault-incr");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&incr);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &base,
            CompressionType::Zstd { level: 0 },
            21,
            CommitKind::Full,
            None,
        )
        .unwrap();
    table
        .insert("v2", &[("name".to_string(), Value::from("v2"))], ts)
        .unwrap();
    table
        .flush_incremental_with_epoch(&incr, CompressionType::Zstd { level: 0 }, 22, Some(21))
        .unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&incr).unwrap();
    assert_eq!(report.epoch, Some(22));
    assert_eq!(report.base_epoch, Some(21));

    for entry in std::fs::read_dir(&incr).unwrap().flatten() {
        let delta = entry.path().join("id_indexer.delta");
        if delta.exists() {
            std::fs::write(&delta, b"corrupt").unwrap();
            let strict = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
            strict.load(&base).unwrap();
            let err = strict.apply_delta_pages(&incr).unwrap_err().to_string();
            assert!(err.contains("22"), "strict error carries epoch: {err}");
            break;
        }
    }
    let _ = std::fs::remove_file(incr.join(COMMIT_MANIFEST_FILE_NAME));
    let reloaded_missing = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    reloaded_missing.load(&base).unwrap();
    assert!(reloaded_missing.apply_delta_pages(&incr).is_err());
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&incr);
}

#[test]
fn fault_matrix_corrupt_manifest_refuses_open() {
    let dir = unique_dir("fault-corrupt");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            17,
            CommitKind::Full,
            None,
        )
        .unwrap();
    std::fs::write(dir.join(COMMIT_MANIFEST_FILE_NAME), b"{broken").unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(report.manifest_present);
    assert!(!report.manifest_decodable);
    assert!(!report.is_healthy());
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("commit manifest"),
        "corrupt manifest must refuse with manifest cause: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fault_matrix_tampered_manifest_checksum_refuses_open() {
    let dir = unique_dir("fault-tamper");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            19,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["epoch"] = serde_json::Value::from(20u64);
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(report.manifest_present);
    assert!(!report.manifest_decodable);
    assert!(!report.is_healthy());
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("checksum"),
        "tampered manifest must refuse on checksum: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn table_manifest_pins_router_version_and_generation() {
    let dir = unique_dir("lineage-pinned");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            21,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let table_manifest = ShardedVertexTable::read_table_manifest(&dir)
        .unwrap()
        .expect("table manifest present");
    assert_eq!(
        table_manifest.router_version,
        super::super::routing::ROUTER_VERSION
    );
    assert_eq!(table_manifest.generation, 0);
    let commit_manifest = ShardedVertexTable::read_commit_manifest(&dir)
        .unwrap()
        .expect("commit manifest present");
    assert_eq!(
        commit_manifest.generation, table_manifest.generation,
        "commit and table manifests must pin the same lineage generation"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tampered_router_version_refuses_open() {
    let dir = unique_dir("lineage-router");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            23,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest_path = dir.join(TABLE_MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["router_version"] = serde_json::Value::from(99u64);
    manifest["checksum"] = serde_json::Value::from(table_manifest_checksum(TableManifestInput {
        format_version: manifest["format_version"].as_u64().unwrap() as u8,
        label: manifest["label"].as_u64().unwrap() as _,
        label_name: manifest["label_name"].as_str().unwrap(),
        num_shards: manifest["num_shards"].as_u64().unwrap() as usize,
        segment_slots_bits: manifest["segment_slots_bits"].as_u64().unwrap() as u32,
        total_segments: manifest["total_segments"].as_u64().unwrap() as u32,
        router_version: 99,
        generation: manifest["generation"].as_u64().unwrap(),
        last_full_flush_ms: manifest["last_full_flush_ms"].as_u64(),
    }));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("router"),
        "unknown router version must refuse with router cause: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commit_generation_mismatch_refuses_open() {
    let dir = unique_dir("lineage-mismatch");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            25,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["generation"] = serde_json::Value::from(7u64);
    let kind = match manifest["kind"].as_str().unwrap() {
        "full" => CommitKind::Full,
        "incremental" => CommitKind::Incremental,
        other => panic!("unexpected commit kind {other}"),
    };
    let files: Vec<String> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let sidecars: Vec<SnapshotSidecarRecord> =
        serde_json::from_value(manifest.get("sidecars").cloned().unwrap_or_default())
            .unwrap_or_default();
    manifest["checksum"] = serde_json::Value::from(commit_manifest_checksum(CommitManifestInput {
        format_version: manifest["format_version"].as_u64().unwrap() as u8,
        epoch: manifest["epoch"].as_u64().unwrap(),
        kind,
        base_epoch: manifest["base_epoch"].as_u64(),
        generation: 7,
        files: &files,
        sidecars: &sidecars,
        written_at_ms: manifest["written_at_ms"].as_u64().unwrap(),
    }));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("generation"),
        "cross-generation checkpoint must refuse with lineage cause: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn flushed_table_manifest_dir(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = unique_dir(tag);
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            31,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest_path = dir.join(TABLE_MANIFEST_FILE_NAME);
    (dir, manifest_path)
}

#[test]
fn table_manifest_version_mismatch_refuses_open_and_health() {
    let (dir, manifest_path) = flushed_table_manifest_dir("table-version");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["format_version"] = serde_json::Value::from(9u64);
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("version"),
        "unknown table manifest version must refuse: {err}"
    );
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(
        report
            .lineage_issues
            .iter()
            .any(|m| m.contains("rejected") && m.contains("version")),
        "health must locate the version defect: {:?}",
        report.lineage_issues
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tampered_table_manifest_checksum_refuses_open_and_health() {
    let (dir, manifest_path) = flushed_table_manifest_dir("table-checksum");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["checksum"] = serde_json::Value::from(0u64);
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("checksum mismatch"),
        "tampered table manifest must refuse: {err}"
    );
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(
        report
            .lineage_issues
            .iter()
            .any(|m| m.contains("rejected") && m.contains("checksum")),
        "health must locate the checksum defect: {:?}",
        report.lineage_issues
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_table_manifest_refuses_open_and_health() {
    let (dir, manifest_path) = flushed_table_manifest_dir("table-missing");
    std::fs::remove_file(&manifest_path).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("missing table manifest"),
        "missing table manifest must refuse: {err}"
    );
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(
        report
            .lineage_issues
            .iter()
            .any(|m| m.contains("table manifest missing")),
        "health must report the missing file, not a generic defect: {:?}",
        report.lineage_issues
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commit_manifest_version_mismatch_refuses_open_and_health() {
    let dir = unique_dir("commit-version");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            33,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["format_version"] = serde_json::Value::from(9u64);
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("version"),
        "unknown commit manifest version must refuse: {err}"
    );
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(
        report.manifest_present && !report.manifest_decodable,
        "undecodable commit manifest must stay visible: {report:?}"
    );
    assert!(
        report
            .lineage_issues
            .iter()
            .any(|m| m.contains("undecodable")),
        "health must flag the unprovable lineage: {:?}",
        report.lineage_issues
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reshard_rebuild_opens_under_new_layout() {
    let staging = unique_dir("reshard-stage");
    let _ = std::fs::remove_dir_all(&staging);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    for i in 0..10 {
        table
            .insert(
                &format!("s_{i}"),
                &[("name".to_string(), Value::from(format!("s_{i}")))],
                10,
            )
            .unwrap();
    }
    let (rebuilt, mapping) = table.reshard_to(4).expect("reshard succeeds");
    assert_eq!(mapping.len(), 10);
    rebuilt
        .flush(&staging, CompressionType::Zstd { level: 0 })
        .expect("rebuilt checkpoint flushes");
    let adopted = ShardedVertexTable::with_layout(
        1,
        "t".to_string(),
        test_schema(),
        super::super::routing::ShardLayout::for_new_table(4),
        1,
    );
    adopted.load(&staging).expect("adopted lineage opens");
    assert_eq!(adopted.approximate_total_count(), 10);
    let rebuilt_ts = graphdb_core::types::MAX_TIMESTAMP - 1;
    assert!(adopted.get_internal_id("s_3", rebuilt_ts).is_some());
    let via_open_at = ShardedVertexTable::open_at(1, "t".to_string(), test_schema(), &staging)
        .expect("open adopts manifest layout and generation");
    assert_eq!(via_open_at.num_shards(), 4);
    assert_eq!(via_open_at.generation(), 1);
    assert_eq!(via_open_at.approximate_total_count(), 10);
    assert!(via_open_at.get_internal_id("s_3", rebuilt_ts).is_some());
    let _ = std::fs::remove_dir_all(&staging);
}

#[test]
fn health_report_proves_lineage_on_healthy_checkpoint() {
    let dir = unique_dir("health-lineage");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            27,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(report.is_healthy());
    assert_eq!(
        report.router_version,
        Some(crate::vertex::vertex_table::sharded::routing::ROUTER_VERSION)
    );
    assert_eq!(report.generation, Some(0));
    assert_eq!(report.commit_generation, Some(0));
    assert!(report.lineage_issues.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn health_report_flags_generation_mismatch_before_open() {
    let dir = unique_dir("health-mismatch");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            29,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest_path = dir.join(COMMIT_MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["generation"] = serde_json::Value::from(7u64);
    let kind = match manifest["kind"].as_str().unwrap() {
        "full" => CommitKind::Full,
        "incremental" => CommitKind::Incremental,
        other => panic!("unexpected commit kind {other}"),
    };
    let files: Vec<String> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let sidecars: Vec<SnapshotSidecarRecord> =
        serde_json::from_value(manifest.get("sidecars").cloned().unwrap_or_default())
            .unwrap_or_default();
    manifest["checksum"] = serde_json::Value::from(commit_manifest_checksum(CommitManifestInput {
        format_version: manifest["format_version"].as_u64().unwrap() as u8,
        epoch: manifest["epoch"].as_u64().unwrap(),
        kind,
        base_epoch: manifest["base_epoch"].as_u64(),
        generation: 7,
        files: &files,
        sidecars: &sidecars,
        written_at_ms: manifest["written_at_ms"].as_u64().unwrap(),
    }));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(!report.is_healthy());
    assert!(report.manifest_decodable);
    assert_eq!(report.commit_generation, Some(7));
    assert!(
        report.lineage_issues.iter().any(|m| m.contains("differs")),
        "mismatch must be named before open refuses: {:?}",
        report.lineage_issues
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn flush_signals_track_delta_and_baseline_age() {
    let dir = unique_dir("flush-signals");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    let pending = table.flush_signals();
    assert!(pending.delta_entries > 0);
    assert_eq!(pending.millis_since_baseline, u64::MAX);
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            31,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let anchored = table.flush_signals();
    assert_eq!(anchored.delta_entries, 0);
    assert!(anchored.millis_since_baseline < 60_000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn baseline_timestamp_survives_restart_and_drives_age() {
    let dir = unique_dir("baseline-ts");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            41,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(TABLE_MANIFEST_FILE_NAME)).unwrap())
            .unwrap();
    assert!(
        manifest.get("last_full_flush_ms").is_some(),
        "full flush must persist the baseline timestamp"
    );
    let reopened = ShardedVertexTable::with_layout(
        1,
        "t".to_string(),
        test_schema(),
        super::super::routing::ShardLayout::for_new_table(2),
        0,
    );
    reopened.load(&dir).expect("timestamped manifest opens");
    let signals = reopened.flush_signals();
    assert!(
        signals.millis_since_baseline < 60_000,
        "restarted age signal must be continuous, got {}",
        signals.millis_since_baseline
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manifest_without_timestamp_refuses_open() {
    let dir = unique_dir("baseline-ts-missing");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            43,
            CommitKind::Full,
            None,
        )
        .unwrap();
    // Stripping the timestamp breaks the checksum: manifests without the
    // field refuse the open and require a rebuild.
    let manifest_path = dir.join(TABLE_MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest
        .as_object_mut()
        .unwrap()
        .remove("last_full_flush_ms");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let reopened = ShardedVertexTable::with_layout(
        1,
        "t".to_string(),
        test_schema(),
        super::super::routing::ShardLayout::for_new_table(2),
        0,
    );
    reopened
        .load(&dir)
        .expect_err("manifest without timestamp refuses");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn flush_plan_advises_baseline_before_first_full_flush() {
    use crate::vertex::vertex_table::flush_trigger::{FlushKind, FlushReason};
    let dir = unique_dir("flush-plan");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], 10)
        .unwrap();
    // No baseline has ever been anchored in this process: the unknown
    // age reads as infinitely old, so the coordinator must escalate.
    let plan = table.flush_plan();
    assert_eq!(plan.kind, FlushKind::Full);
    assert_eq!(plan.reason, FlushReason::BaselineAge);
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            37,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let settled = table.flush_plan();
    assert_eq!(settled.kind, FlushKind::Skip);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offline_store_inspection_aggregates_labels_and_chain() {
    let root = unique_dir("store-health");
    let _ = std::fs::remove_dir_all(&root);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            root.join("label_1"),
            CompressionType::Zstd { level: 0 },
            31,
            CommitKind::Full,
            None,
        )
        .unwrap();
    table
        .flush_incremental_with_epoch(
            root.join("label_2"),
            CompressionType::Zstd { level: 0 },
            32,
            Some(31),
        )
        .unwrap();
    let health = ShardedVertexTable::inspect_store_health(&root).unwrap();
    assert_eq!(health.tables.len(), 2);
    assert!(health.is_healthy());
    assert!(health.chain_ok);
    assert!(health.issues.is_empty());
    std::fs::write(root.join("label_2").join("half.tmp"), b"x").unwrap();
    let health = ShardedVertexTable::inspect_store_health(&root).unwrap();
    assert!(health.is_healthy());
    let manifest = ShardedVertexTable::read_commit_manifest(root.join("label_1"))
        .unwrap()
        .expect("manifest");
    let victim = manifest.files.first().expect("listed file").clone();
    std::fs::remove_file(root.join("label_1").join(&victim)).unwrap();
    let health = ShardedVertexTable::inspect_store_health(&root).unwrap();
    assert!(!health.is_healthy());
    assert!(health.issues.iter().any(|m| m.contains("missing")));
    let _ = std::fs::remove_dir_all(&root);
}

fn write_enveloped_delta(path: &std::path::Path, raw: &[u8]) {
    use crate::persistence::{section, write_header_to};
    let mut payload = Vec::new();
    write_header_to(&mut payload, section::VERTEX_ID_INDEXER_DELTA).unwrap();
    payload.extend_from_slice(raw);
    let page_size = crate::compression::DEFAULT_PAGE_SIZE;
    let mut writer = crate::compression::PageWriter::new(page_size, 3);
    let mut pages_buf = Vec::new();
    writer.write_all(&mut pages_buf, &payload).unwrap();
    let mut final_buf = Vec::new();
    crate::compression::ColumnFileHeader {
        page_size,
        page_count: writer.page_count(),
        total_rows: 1,
    }
    .serialize(&mut final_buf)
    .unwrap();
    final_buf.extend_from_slice(&pages_buf);
    crate::compression::write_shadow_file(path, &final_buf).unwrap();
}

#[test]
fn pk_baseline_corrupt_refuses_open_and_health() {
    let dir = unique_dir("pk-base-corrupt");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            41,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(report.is_healthy());
    assert!(report.pk_index_ok);
    assert!(report.pk_issues.is_empty());

    std::fs::write(dir.join("shard_0").join("id_indexer.bin"), b"corrupt").unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("41") && err.contains("shard"),
        "baseline corruption must refuse with epoch and shard: {err}"
    );
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(!report.is_healthy());
    assert!(!report.pk_index_ok);
    assert!(report.pk_issues.iter().any(|m| m.contains("pk baseline")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pk_diverging_delta_refuses_apply_and_health() {
    use crate::vertex::id_indexer::{IdKey, IdManager};

    let base = unique_dir("pk-div-base");
    let incr = unique_dir("pk-div-incr");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&incr);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &base,
            CompressionType::Zstd { level: 0 },
            42,
            CommitKind::Full,
            None,
        )
        .unwrap();
    table
        .insert("v2", &[("name".to_string(), Value::from("v2"))], ts)
        .unwrap();
    table
        .flush_incremental_with_epoch(&incr, CompressionType::Zstd { level: 0 }, 43, Some(42))
        .unwrap();

    let mgr = IdManager::new();
    mgr.insert(IdKey::Text("v1".to_string())).unwrap();
    let mut raw = mgr.serialize_delta();
    raw[5..9].copy_from_slice(&7u32.to_le_bytes());
    write_enveloped_delta(&incr.join("shard_0").join("id_indexer.delta"), &raw);

    let strict = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    strict.load(&base).unwrap();
    let err = strict.apply_delta_pages(&incr).unwrap_err().to_string();
    assert!(
        err.contains("diverges"),
        "divergent delta must refuse on divergence: {err}"
    );
    // Per-directory health only checks decodability here (the anchor
    // lives in the base directory); the divergence refuses at apply.
    let report = ShardedVertexTable::inspect_commit_health(&incr).unwrap();
    assert!(report.pk_index_ok);
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&incr);
}

#[test]
fn pk_lingering_delta_flagged_by_health() {
    use crate::vertex::id_indexer::{IdKey, IdManager};

    let dir = unique_dir("pk-linger");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            44,
            CommitKind::Full,
            None,
        )
        .unwrap();

    let mgr = IdManager::new();
    mgr.insert(IdKey::Text("v1".to_string())).unwrap();
    let mut raw = mgr.serialize_delta();
    raw[5..9].copy_from_slice(&7u32.to_le_bytes());
    write_enveloped_delta(&dir.join("shard_0").join("id_indexer.delta"), &raw);

    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(!report.is_healthy());
    assert!(!report.pk_index_ok);
    assert!(report.pk_issues.iter().any(|m| m.contains("diverges")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshot_sidecars_stay_outside_manifest_and_load() {
    let dir = unique_dir("snap-manifest");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            45,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest = ShardedVertexTable::read_commit_manifest(&dir)
        .unwrap()
        .expect("commit manifest present");
    assert!(manifest.files.iter().all(|f| !f.ends_with(".snapshot")));

    std::fs::write(dir.join("shard_0").join("name.snapshot"), b"junk").unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    reloaded.load(&dir).unwrap();
    assert!(reloaded.get_internal_id("v1", ts).is_some());
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    // Unpinned junk is discardable: healthy for strict open, visible as
    // a sidecar issue for observability.
    assert!(report.is_healthy());
    assert!(!report.sidecar_issues.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pinned_sidecar_corruption_still_opens_with_discard() {
    let dir = unique_dir("snap-pinned");
    let _ = std::fs::remove_dir_all(&dir);
    let schema = crate::vertex::VertexSchema {
        label_id: 1,
        label_name: "person".to_string(),
        properties: vec![
            crate::types::StoragePropertyDef::new("name".to_string(), DataType::String),
            crate::types::StoragePropertyDef::new("group".to_string(), DataType::String),
        ],
        primary_key_index: 0,
        schema_version: 1,
    };
    let table = ShardedVertexTable::with_config(1, "t".to_string(), schema, 1);
    let ts: Timestamp = 10;
    for i in 0..5000 {
        let id = format!("v{i:05}");
        table
            .insert(
                &id,
                &[
                    ("name".to_string(), Value::from(id.clone())),
                    ("group".to_string(), Value::from("same")),
                ],
                ts,
            )
            .unwrap();
    }
    // First flush encodes in-memory chunks; eviction needs encoded
    // chunks, so force-encode the low-cardinality column and evict it
    // directly, then flush again to pin.
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            46,
            CommitKind::Full,
            None,
        )
        .unwrap();
    table.force_encode_evict_for_test("group");
    let _ = table.evict_cold_chunks(u64::MAX);
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            47,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let manifest = ShardedVertexTable::read_commit_manifest(&dir)
        .unwrap()
        .expect("commit manifest present");
    assert!(
        !manifest.sidecars.is_empty(),
        "evicted flush must pin sidecars"
    );
    // Corrupt the first pinned sidecar: strict open must still succeed
    // with resident fallback, and health must flag the discard.
    let victim = dir.join(&manifest.sidecars[0].file);
    std::fs::write(&victim, b"tampered").unwrap();
    let reload_schema = crate::vertex::VertexSchema {
        label_id: 1,
        label_name: "person".to_string(),
        properties: vec![
            crate::types::StoragePropertyDef::new("name".to_string(), DataType::String),
            crate::types::StoragePropertyDef::new("group".to_string(), DataType::String),
        ],
        primary_key_index: 0,
        schema_version: 1,
    };
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), reload_schema, 1);
    reloaded.load(&dir).unwrap();
    assert!(reloaded.get_internal_id("v00042", ts).is_some());
    let report = ShardedVertexTable::inspect_commit_health(&dir).unwrap();
    assert!(report.is_healthy());
    assert!(!report.sidecar_issues.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn strict_open_reports_shard_damage_class() {
    let dir = unique_dir("strict-shard-damage");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            47,
            CommitKind::Full,
            None,
        )
        .unwrap();
    // Corrupt one shard's authoritative pages: strict open refuses with
    // a machine-readable isolatable class naming the shard.
    std::fs::write(dir.join("shard_0").join("columns.bin"), b"corrupt").unwrap();
    let strict = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
    let err = strict.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("class=isolatable") && err.contains("shard"),
        "strict shard failure must carry machine-readable class and shard: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn two_column_schema() -> crate::vertex::VertexSchema {
    crate::vertex::VertexSchema {
        label_id: 1,
        label_name: "person".to_string(),
        properties: vec![
            StoragePropertyDef::new("name".to_string(), DataType::String),
            StoragePropertyDef {
                name: "bio".to_string(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            },
        ],
        primary_key_index: 0,
        schema_version: 1,
    }
}

#[test]
fn column_overflow_missing_degrades_single_column() {
    let dir = unique_dir("col-isolate");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), two_column_schema(), 1);
    let ts: Timestamp = 10;
    let big_bio = "x".repeat(2000);
    table
        .insert(
            "v1",
            &[
                ("name".to_string(), Value::from("v1")),
                ("bio".to_string(), Value::from(big_bio.clone())),
            ],
            ts,
        )
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            51,
            CommitKind::Full,
            None,
        )
        .unwrap();
    let overflow = dir.join("shard_0").join("bio.overflow");
    assert!(
        overflow.exists(),
        "large string must spill to a per-column overflow sidecar"
    );
    std::fs::remove_file(&overflow).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), two_column_schema(), 1);
    reloaded
        .load(&dir)
        .expect("single-column loss must not refuse the table");
    assert!(
        reloaded.get_internal_id("v1", ts).is_some(),
        "identity must stay usable after column isolation"
    );
    let gid = reloaded.get_internal_id("v1", ts).unwrap();
    let manager = graphdb_transaction::VersionManager::new();
    let guard = crate::mvcc_visibility::VisibilityGuard::new(
        ts,
        crate::mvcc_visibility::PendingGate::new(&manager, None),
    );
    let healthy = reloaded
        .resolve_projected_batch(&[gid], &guard, Some(&["name".to_string()]))
        .expect("healthy projection must still decode");
    assert!(healthy[0].is_some(), "healthy column must stay readable");
    let manager2 = graphdb_transaction::VersionManager::new();
    let guard2 = crate::mvcc_visibility::VisibilityGuard::new(
        ts,
        crate::mvcc_visibility::PendingGate::new(&manager2, None),
    );
    let err = reloaded
        .resolve_projected_batch(&[gid], &guard2, Some(&["bio".to_string()]))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("bio"),
        "strict read of the degraded column must name it: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn table_critical_columns_bin_missing_still_refuses() {
    let dir = unique_dir("table-critical");
    let _ = std::fs::remove_dir_all(&dir);
    let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    let ts: Timestamp = 10;
    table
        .insert("v1", &[("name".to_string(), Value::from("v1"))], ts)
        .unwrap();
    table
        .flush_with_epoch(
            &dir,
            CompressionType::Zstd { level: 0 },
            52,
            CommitKind::Full,
            None,
        )
        .unwrap();
    std::fs::remove_file(dir.join("shard_0").join("columns.bin")).unwrap();
    let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
    let err = reloaded.load(&dir).unwrap_err().to_string();
    assert!(
        err.contains("columns.bin") || err.contains("missing"),
        "table-critical loss must still refuse the open: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn isolatable_file_classification_names_columns() {
    use super::commit_manifest::column_for_isolatable_file;
    assert_eq!(
        column_for_isolatable_file("shard_0/bio.overflow").as_deref(),
        Some("bio")
    );
    assert_eq!(
        column_for_isolatable_file("shard_3/columns_pages/bio_12.page").as_deref(),
        Some("bio")
    );
    assert!(column_for_isolatable_file("shard_0/columns.bin").is_none());
    assert!(column_for_isolatable_file("shard_0/meta.bin").is_none());
    assert!(column_for_isolatable_file("shard_0/id_indexer.bin").is_none());
    assert!(column_for_isolatable_file("table_manifest.json").is_none());
}

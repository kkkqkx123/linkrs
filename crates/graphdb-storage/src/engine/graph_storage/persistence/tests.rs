//! Tests for persistence bootstrap, WAL recovery and checkpoint metadata.

use std::path::Path;

use super::recovery::{needs_recovery, read_checkpoint_metadata, recover_from_wal};
use crate::engine::PersistenceConfig;
use crate::types::StoragePropertyDef;
use graphdb_core::types::CommitLsn;
use graphdb_core::types::VertexId;
use graphdb_core::{DataType, StorageError, StorageResult, Value};
use graphdb_sync::checkpoint_manifest::CheckpointManifest;
use graphdb_sync::checkpoint_manifest::CheckpointManifestManager;
use graphdb_transaction::wal::recovery::RecoveryStats;
use graphdb_transaction::wal::writer::WalWriter;
use graphdb_transaction::wal::{InsertVertexRedo, LocalWalWriter, Lsn, WalOpType};
use postcard::to_allocvec;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::persistence_coordinator::CHECKPOINT_FORMAT_VERSION;

fn write_insert_vertex_wal(
    wal_dir: &Path,
    timestamp: u64,
    label: u32,
    vid: i64,
    name: &str,
) -> StorageResult<Lsn> {
    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer
        .open()
        .map_err(|e| StorageError::wal_error(format!("Failed to open WAL: {:?}", e)))?;

    let redo = InsertVertexRedo {
        label,
        vid: VertexId::try_from_int64(vid).expect("test vertex id"),
        properties: vec![("name".into(), Value::string(name))],
    };
    let payload = to_allocvec(&redo).map_err(|e| StorageError::serialize_error(e.to_string()))?;

    writer
        .append_entry(WalOpType::InsertVertex, timestamp, &payload)
        .map_err(|e| StorageError::wal_error(format!("Failed to append WAL: {:?}", e)))?;

    let lsn = writer.current_lsn();
    writer
        .sync()
        .map_err(|e| StorageError::wal_error(format!("Failed to sync WAL: {:?}", e)))?;
    writer.close();

    Ok(lsn)
}

fn write_checkpoint_metadata(
    checkpoint_dir: &Path,
    checkpoint_id: u64,
    wal_lsn: Lsn,
) -> StorageResult<()> {
    let checkpoint_path = checkpoint_dir.join(format!("checkpoint_{}", checkpoint_id));
    fs::create_dir_all(&checkpoint_path)?;

    let metadata_path = checkpoint_path.join("checkpoint.meta");
    let mut file = fs::File::create(metadata_path)?;
    writeln!(file, "format_version={}", CHECKPOINT_FORMAT_VERSION)?;
    writeln!(file, "checkpoint_id={}", checkpoint_id)?;
    writeln!(file, "wal_lsn={}", wal_lsn.as_u64())?;

    let storage_ref =
        CheckpointManifest::storage_snapshot_from_directory(&checkpoint_path, checkpoint_id, 0, 0)
            .map_err(StorageError::db_error)?;
    let manifest = CheckpointManifest::new(
        checkpoint_id,
        CommitLsn::new(wal_lsn.as_u64()),
        storage_ref,
        None,
        Vec::new(),
    )
    .map_err(StorageError::db_error)?;
    let manager = CheckpointManifestManager::new(checkpoint_dir.join("manifests"));
    manager.init().map_err(StorageError::db_error)?;
    manager.publish(&manifest).map_err(StorageError::db_error)?;

    Ok(())
}

fn create_context(temp_dir: &TempDir) -> StorageResult<GraphStorageContext> {
    let config = PersistenceConfig::for_work_dir(temp_dir.path());
    GraphStorageContext::new_with_persistence(temp_dir.path().to_path_buf(), config)
}

#[test]
fn test_needs_recovery_false_after_checkpoint() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ctx = create_context(&temp_dir).expect("Failed to create storage context");

    let (wal_dir, checkpoint_dir) = {
        let persistence = ctx
            .persistence()
            .as_ref()
            .expect("Persistence should exist");
        let coordinator = persistence.read();
        (coordinator.wal_dir(), coordinator.checkpoint_dir())
    };

    let wal_lsn =
        write_insert_vertex_wal(&wal_dir, 1, 1, 1001, "Alice").expect("Failed to write WAL");
    write_checkpoint_metadata(&checkpoint_dir, 1, wal_lsn)
        .expect("Failed to write checkpoint metadata");

    assert!(!needs_recovery(&ctx));
}

#[test]
fn test_needs_recovery_true_when_wal_is_ahead_of_checkpoint() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ctx = create_context(&temp_dir).expect("Failed to create storage context");

    let (wal_dir, checkpoint_dir) = {
        let persistence = ctx
            .persistence()
            .as_ref()
            .expect("Persistence should exist");
        let coordinator = persistence.read();
        (coordinator.wal_dir(), coordinator.checkpoint_dir())
    };

    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer.open().expect("Failed to open WAL");

    let first_redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::try_from_int64(1001).expect("test vertex id"),
        properties: vec![("name".into(), Value::string("Alice"))],
    };
    let first_payload = to_allocvec(&first_redo).expect("Failed to serialize first redo");
    writer
        .append_entry(WalOpType::InsertVertex, 1, &first_payload)
        .expect("Failed to append first WAL entry");
    let checkpoint_lsn = writer.current_lsn();

    let second_redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::try_from_int64(1002).expect("test vertex id"),
        properties: vec![("name".into(), Value::string("Bob"))],
    };
    let second_payload = to_allocvec(&second_redo).expect("Failed to serialize second redo");
    writer
        .append_entry(WalOpType::InsertVertex, 2, &second_payload)
        .expect("Failed to append second WAL entry");
    writer.close();

    write_checkpoint_metadata(&checkpoint_dir, 1, checkpoint_lsn)
        .expect("Failed to write checkpoint metadata");

    assert!(needs_recovery(&ctx));
}

#[test]
fn test_recover_from_wal_persists_checkpoint_baseline() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ctx = create_context(&temp_dir).expect("Failed to create storage context");

    let (wal_dir, checkpoint_dir) = {
        let persistence = ctx
            .persistence()
            .as_ref()
            .expect("Persistence should exist");
        let coordinator = persistence.read();
        (coordinator.wal_dir(), coordinator.checkpoint_dir())
    };

    ctx.create_vertex_type_with_id(
        "space_1:tag:person",
        "person",
        1,
        vec![
            StoragePropertyDef::new("id".into(), DataType::BigInt),
            StoragePropertyDef::new("name".into(), DataType::String),
        ],
        "id",
    )
    .expect("Failed to create vertex type");

    let _wal_lsn =
        write_insert_vertex_wal(&wal_dir, 1, 1, 1001, "Alice").expect("Failed to write WAL");
    write_checkpoint_metadata(&checkpoint_dir, 1, Lsn::ZERO)
        .expect("Failed to write checkpoint metadata");

    let stats: RecoveryStats = recover_from_wal(&ctx).expect("Recovery should succeed");
    assert_eq!(stats.wal_entries_replayed, 1);
    assert!(!needs_recovery(&ctx));
}

#[test]
fn test_read_checkpoint_metadata_rejects_malformed_fields() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let checkpoint_dir = temp_dir.path().join("checkpoint");
    let checkpoint_path = checkpoint_dir.join("checkpoint_1");
    fs::create_dir_all(&checkpoint_path).expect("Failed to create checkpoint dir");

    let metadata_path = checkpoint_path.join("checkpoint.meta");
    let mut file = fs::File::create(&metadata_path).expect("Failed to create metadata file");
    writeln!(file, "checkpoint_id=1").expect("Failed to write checkpoint id");
    writeln!(file, "wal_lsn=not-a-number").expect("Failed to write wal lsn");

    let result = read_checkpoint_metadata(&checkpoint_path);
    assert!(result.is_err());
}

#[test]
fn test_migration_history_survives_restart() {
    use crate::migration_history::{MigrationHistoryRecord, MigrationStatus};
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let ctx = create_context(&temp_dir).expect("Failed to create storage context");
    let record = MigrationHistoryRecord::new(
        "test_space".to_string(),
        "person".to_string(),
        false,
        1,
        2,
        "hash123".to_string(),
        "safe".to_string(),
        1,
        100,
        MigrationStatus::Applied,
    );
    ctx.record_migration_history(record.clone())
        .expect("Failed to record migration history");
    let before = ctx.list_all_migration_history();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].space, "test_space");
    // Simulate restart with same work_dir
    let ctx2 = create_context(&temp_dir).expect("Failed to create second context");
    ctx2.load_migration_history()
        .expect("Failed to load migration history");
    let after = ctx2.list_all_migration_history();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].space, "test_space");
    assert_eq!(after[0].label, "person");
    assert_eq!(after[0].from_version, 1);
    assert_eq!(after[0].to_version, 2);
}

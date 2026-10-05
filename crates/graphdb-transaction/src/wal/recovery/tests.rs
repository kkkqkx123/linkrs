//! Recovery orchestration tests with a recording applier

use postcard::to_allocvec;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

use super::{RecoveryApplier, RecoveryConfig, RecoveryManager};
use crate::wal::writer::{LocalWalWriter, WalWriter};
use crate::wal::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeIndexRedo,
    CreateEdgeTypeRedo, CreateSpaceRedo, CreateTagIndexRedo, CreateVertexTypeRedo,
    DeleteEdgePropRedo, DeleteEdgeRedo, DeleteEdgeTypeRedo, DeleteVertexPropRedo,
    DeleteVertexTypeRedo, DropEdgeIndexRedo, DropSpaceRedo, DropTagIndexRedo, InsertEdgeRedo,
    InsertVertexRedo, LabelId, Lsn, RenameEdgePropRedo, RenameEdgeTypeRedo, RenameTagRedo,
    RenameVertexPropRedo, Timestamp, TransactionWalEntry, UpdateEdgePropRedo, VertexId, WalOpType,
    WalRecoveryMode,
};
use graphdb_core::{StorageError, StorageResult, Value};

#[derive(Default)]
struct RecordingApplier {
    replayed_vertices: Arc<Mutex<Vec<(LabelId, VertexId, Timestamp)>>>,
}

impl RecordingApplier {
    fn replayed_vertices(&self) -> Vec<(LabelId, VertexId, Timestamp)> {
        self.replayed_vertices
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }
}

macro_rules! ok_methods {
    ($($name:ident($($arg:ident : $ty:ty),*)),* $(,)?) => {
        $(
            fn $name(&self, $($arg: $ty),*) -> StorageResult<()> {
                let _ = ($( &$arg ),*);
                Ok(())
            }
        )*
    };
}

impl RecoveryApplier for RecordingApplier {
    fn replay_insert_vertex(
        &self,
        label: LabelId,
        vid: VertexId,
        _properties: &[(std::sync::Arc<str>, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        self.replayed_vertices
            .lock()
            .map(|mut entries| entries.push((label, vid, ts)))
            .map_err(|e| StorageError::db_error(format!("Failed to record replay: {}", e)))?;
        Ok(())
    }

    ok_methods! {
        replay_insert_edge(redo: &InsertEdgeRedo, ts: Timestamp),
        replay_update_vertex_prop(
            label: LabelId,
            vid: VertexId,
            prop_name: &str,
            value: &Value,
            ts: Timestamp
        ),
        replay_update_edge_prop(redo: &UpdateEdgePropRedo, ts: Timestamp),
        replay_delete_vertex(label: LabelId, vid: VertexId, ts: Timestamp),
        replay_delete_edge(redo: &DeleteEdgeRedo, ts: Timestamp),
        replay_create_space(redo: &CreateSpaceRedo, ts: Timestamp),
        replay_drop_space(redo: &DropSpaceRedo, ts: Timestamp),
        replay_clear_space(redo: &ClearSpaceRedo, ts: Timestamp),
        replay_alter_space_comment(redo: &AlterSpaceCommentRedo, ts: Timestamp),
        replay_create_vertex_type(redo: &CreateVertexTypeRedo, ts: Timestamp),
        replay_create_edge_type(redo: &CreateEdgeTypeRedo, ts: Timestamp),
        replay_delete_vertex_type(redo: &DeleteVertexTypeRedo, ts: Timestamp),
        replay_delete_edge_type(redo: &DeleteEdgeTypeRedo, ts: Timestamp),
        replay_add_vertex_prop(redo: &AddVertexPropRedo, ts: Timestamp),
        replay_add_edge_prop(redo: &AddEdgePropRedo, ts: Timestamp),
        replay_delete_vertex_prop(redo: &DeleteVertexPropRedo, ts: Timestamp),
        replay_delete_edge_prop(redo: &DeleteEdgePropRedo, ts: Timestamp),
        replay_rename_vertex_prop(redo: &RenameVertexPropRedo, ts: Timestamp),
        replay_rename_edge_prop(redo: &RenameEdgePropRedo, ts: Timestamp),
        replay_rename_tag(redo: &RenameTagRedo, ts: Timestamp),
        replay_rename_edge_type(redo: &RenameEdgeTypeRedo, ts: Timestamp),
        replay_create_tag_index(redo: &CreateTagIndexRedo, ts: Timestamp),
        replay_drop_tag_index(redo: &DropTagIndexRedo, ts: Timestamp),
        replay_create_edge_index(redo: &CreateEdgeIndexRedo, ts: Timestamp),
        replay_drop_edge_index(redo: &DropEdgeIndexRedo, ts: Timestamp),
    }
}

fn write_insert_vertex_wal(
    wal_dir: &Path,
    timestamp: Timestamp,
    label: LabelId,
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
    let lsn = writer
        .append_transaction_batch(
            graphdb_core::types::TransactionId::new(timestamp),
            vec![TransactionWalEntry {
                op_type: WalOpType::InsertVertex,
                timestamp,
                payload,
                transaction_id: None,
                mutation_sequence: None,
            }],
            &[],
        )
        .map_err(|e| StorageError::wal_error(format!("Failed to append WAL: {:?}", e)))?;
    writer.close();

    Ok(Lsn::new(lsn.get()))
}

#[test]
fn test_recover_with_start_lsn_skips_checkpointed_entries() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let wal_dir = temp_dir.path().join("wal");
    let data_dir = temp_dir.path().join("data");

    std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");
    std::fs::create_dir_all(&data_dir).expect("Failed to create data dir");

    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer.open().expect("Failed to open WAL");

    let first_redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::try_from_int64(1001).expect("test vertex id"),
        properties: vec![("name".into(), Value::string("Alice"))],
    };
    let first_payload = to_allocvec(&first_redo).expect("Failed to serialize first redo");
    let first_lsn = writer
        .append_transaction_batch(
            graphdb_core::types::TransactionId::new(1),
            vec![TransactionWalEntry {
                op_type: WalOpType::InsertVertex,
                timestamp: 1,
                payload: first_payload,
                transaction_id: None,
                mutation_sequence: None,
            }],
            &[],
        )
        .expect("Failed to append first WAL transaction");

    let second_redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::try_from_int64(1002).expect("test vertex id"),
        properties: vec![("name".into(), Value::string("Bob"))],
    };
    let second_payload = to_allocvec(&second_redo).expect("Failed to serialize second redo");
    let second_lsn = writer
        .append_transaction_batch(
            graphdb_core::types::TransactionId::new(2),
            vec![TransactionWalEntry {
                op_type: WalOpType::InsertVertex,
                timestamp: 2,
                payload: second_payload,
                transaction_id: None,
                mutation_sequence: None,
            }],
            &[],
        )
        .expect("Failed to append second WAL transaction");
    writer.close();

    let mut manager = RecoveryManager::new(RecoveryConfig {
        wal_dir: wal_dir.clone(),
        data_dir: data_dir.clone(),
        recovery_mode: WalRecoveryMode::default(),
        parallel_recovery: false,
        verify_checksum: true,
        start_lsn: Some(Lsn::new(first_lsn.get())),
        ..Default::default()
    });

    let applier = RecordingApplier::default();
    let stats = manager
        .recover_with_applier(&applier)
        .expect("Recovery should succeed");

    let replayed = applier.replayed_vertices();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].0, 1);
    assert_eq!(
        replayed[0].1,
        VertexId::try_from_int64(1002).expect("test vertex id")
    );
    assert_eq!(stats.wal_entries_replayed, 1);
    assert_eq!(stats.last_lsn, Lsn::new(second_lsn.get()));
}

#[test]
fn test_recover_with_start_lsn_after_last_entry_replays_nothing() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let wal_dir = temp_dir.path().join("wal");
    let data_dir = temp_dir.path().join("data");

    std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");
    std::fs::create_dir_all(&data_dir).expect("Failed to create data dir");

    let last_lsn =
        write_insert_vertex_wal(&wal_dir, 1, 1, 1001, "Alice").expect("Failed to write WAL entry");

    let mut manager = RecoveryManager::new(RecoveryConfig {
        wal_dir: wal_dir.clone(),
        data_dir: data_dir.clone(),
        recovery_mode: WalRecoveryMode::default(),
        parallel_recovery: false,
        verify_checksum: true,
        start_lsn: Some(last_lsn),
        ..Default::default()
    });

    let applier = RecordingApplier::default();
    let stats = manager
        .recover_with_applier(&applier)
        .expect("Recovery should succeed");

    assert!(applier.replayed_vertices().is_empty());
    assert_eq!(stats.wal_entries_replayed, 0);
    assert_eq!(stats.last_lsn, last_lsn);
}

#[test]
fn recovery_ignores_uncommitted_tail() {
    let temp_dir = TempDir::new().expect("temporary directory should be created");
    let wal_dir = temp_dir.path().join("wal");
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&wal_dir).expect("WAL directory should be created");
    std::fs::create_dir_all(&data_dir).expect("data directory should be created");
    write_insert_vertex_wal(&wal_dir, 1, 1, 1001, "committed")
        .expect("committed WAL should be written");

    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer.open().expect("WAL should reopen");
    let redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::try_from_int64(1002).expect("test vertex id"),
        properties: vec![("name".into(), Value::string("uncommitted"))],
    };
    writer
        .append_entry(
            WalOpType::InsertVertex,
            2,
            &to_allocvec(&redo).expect("redo should serialize"),
        )
        .expect("uncommitted redo should append");
    writer.sync().expect("uncommitted tail should reach disk");
    writer.close();

    let mut manager = RecoveryManager::new(RecoveryConfig {
        wal_dir,
        data_dir,
        recovery_mode: WalRecoveryMode::default(),
        parallel_recovery: false,
        verify_checksum: true,
        start_lsn: None,
        ..Default::default()
    });
    let applier = RecordingApplier::default();
    manager
        .recover_with_applier(&applier)
        .expect("recovery should succeed");
    let replayed = applier.replayed_vertices();
    assert_eq!(replayed.len(), 1);
    assert_eq!(
        replayed[0].1,
        VertexId::try_from_int64(1001).expect("test vertex id")
    );
}

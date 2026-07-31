//! End-to-End Crash Recovery Tests
//!
//! Uses the WAL RecoveryManager directly to simulate crash/recovery cycles.
//! Verifies that committed WAL entries are replayed and uncommitted ones are discarded.

use graphdb::core::Value;
use graphdb::storage::StorageError;
use graphdb::transaction::wal::recovery::{RecoveryApplier, RecoveryConfig, RecoveryManager};
use graphdb::transaction::wal::writer::{LocalWalWriter, WalWriter};
use graphdb::transaction::wal::WalRecoveryMode;
use graphdb::transaction::wal::{
    InsertVertexRedo, LabelId, Timestamp, TransactionWalEntry, VertexId, WalOpType,
};
use postcard::to_allocvec;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

type ReplayedVertices = Vec<(LabelId, VertexId, Timestamp)>;
type ReplayedVertexProps = Vec<Vec<(String, Value)>>;

#[derive(Default)]
struct RecordingApplier {
    replayed_vertices: Arc<Mutex<ReplayedVertices>>,
    replayed_vertex_props: Arc<Mutex<ReplayedVertexProps>>,
}

impl RecordingApplier {
    fn replayed_vertices(&self) -> Vec<(LabelId, VertexId, Timestamp)> {
        self.replayed_vertices.lock().unwrap().clone()
    }
    fn replayed_vertex_props(&self) -> Vec<Vec<(String, Value)>> {
        self.replayed_vertex_props.lock().unwrap().clone()
    }
}

impl RecoveryApplier for RecordingApplier {
    fn replay_insert_vertex(
        &self,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> Result<(), StorageError> {
        self.replayed_vertices
            .lock()
            .unwrap()
            .push((label, vid, ts));
        self.replayed_vertex_props
            .lock()
            .unwrap()
            .push(properties.to_vec());
        Ok(())
    }

    fn replay_insert_edge(
        &self,
        _redo: &graphdb::transaction::wal::InsertEdgeRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_update_vertex_prop(
        &self,
        _label: LabelId,
        _vid: VertexId,
        _prop_name: &str,
        _value: &Value,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_update_edge_prop(
        &self,
        _redo: &graphdb::transaction::wal::UpdateEdgePropRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_delete_vertex(
        &self,
        _label: LabelId,
        _vid: VertexId,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_delete_edge(
        &self,
        _redo: &graphdb::transaction::wal::DeleteEdgeRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_create_space(
        &self,
        _redo: &graphdb::transaction::wal::CreateSpaceRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_drop_space(
        &self,
        _redo: &graphdb::transaction::wal::DropSpaceRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_clear_space(
        &self,
        _redo: &graphdb::transaction::wal::ClearSpaceRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_alter_space_comment(
        &self,
        _redo: &graphdb::transaction::wal::AlterSpaceCommentRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_create_vertex_type(
        &self,
        _redo: &graphdb::transaction::wal::CreateVertexTypeRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_create_edge_type(
        &self,
        _redo: &graphdb::transaction::wal::CreateEdgeTypeRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_delete_vertex_type(
        &self,
        _redo: &graphdb::transaction::wal::DeleteVertexTypeRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_delete_edge_type(
        &self,
        _redo: &graphdb::transaction::wal::DeleteEdgeTypeRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_add_vertex_prop(
        &self,
        _redo: &graphdb::transaction::wal::AddVertexPropRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_add_edge_prop(
        &self,
        _redo: &graphdb::transaction::wal::AddEdgePropRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_delete_vertex_prop(
        &self,
        _redo: &graphdb::transaction::wal::DeleteVertexPropRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_delete_edge_prop(
        &self,
        _redo: &graphdb::transaction::wal::DeleteEdgePropRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_rename_vertex_prop(
        &self,
        _redo: &graphdb::transaction::wal::RenameVertexPropRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_rename_edge_prop(
        &self,
        _redo: &graphdb::transaction::wal::RenameEdgePropRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_create_tag_index(
        &self,
        _redo: &graphdb::transaction::wal::CreateTagIndexRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_drop_tag_index(
        &self,
        _redo: &graphdb::transaction::wal::DropTagIndexRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_create_edge_index(
        &self,
        _redo: &graphdb::transaction::wal::CreateEdgeIndexRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn replay_drop_edge_index(
        &self,
        _redo: &graphdb::transaction::wal::DropEdgeIndexRedo,
        _ts: Timestamp,
    ) -> Result<(), StorageError> {
        Ok(())
    }
}

fn write_wal_entries(
    wal_dir: &Path,
    entries: &[(Timestamp, LabelId, i64, &str)],
) -> Result<(), Box<dyn std::error::Error>> {
    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer.open()?;

    for &(ts, label, vid, name) in entries {
        let redo = InsertVertexRedo {
            label,
            vid: VertexId::from_int64(vid),
            properties: vec![("name".to_string(), Value::string(name))],
        };
        let payload = to_allocvec(&redo)?;
        writer.append_transaction_batch(
            graphdb::core::types::TransactionId::new(ts),
            vec![TransactionWalEntry {
                op_type: WalOpType::InsertVertex,
                timestamp: ts,
                payload,
            }],
            &[],
        )?;
    }
    writer.sync()?;
    writer.close();
    Ok(())
}

fn recover(
    wal_dir: &Path,
    data_dir: &Path,
) -> (ReplayedVertices, ReplayedVertexProps) {
    let mut manager = RecoveryManager::new(RecoveryConfig {
        wal_dir: wal_dir.to_path_buf(),
        data_dir: data_dir.to_path_buf(),
        recovery_mode: WalRecoveryMode::default(),
        parallel_recovery: false,
        verify_checksum: true,
        start_lsn: None,
    });
    let applier = RecordingApplier::default();
    manager
        .recover_with_applier(&applier)
        .expect("recovery failed");
    (applier.replayed_vertices(), applier.replayed_vertex_props())
}

/// TC-CR01: All committed transactions should survive a simulated crash
#[test]
fn test_committed_wal_entries_survive_recovery() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&wal_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    // Write 3 committed entries at different timestamps
    write_wal_entries(
        &wal_dir,
        &[
            (1, 1, 1001, "Alice"),
            (2, 1, 1002, "Bob"),
            (3, 1, 1003, "Charlie"),
        ],
    )
    .unwrap();

    let (replayed, _) = recover(&wal_dir, &data_dir);
    assert_eq!(replayed.len(), 3);
    assert_eq!(replayed[0].2, 1);
    assert_eq!(replayed[1].2, 2);
    assert_eq!(replayed[2].2, 3);
}

/// TC-CR02: Uncommitted tail entries should be discarded on recovery
#[test]
fn test_uncommitted_tail_discarded() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&wal_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    // Write 2 committed entries
    write_wal_entries(
        &wal_dir,
        &[(1, 1, 1001, "committed_1"), (2, 1, 1002, "committed_2")],
    )
    .unwrap();

    // Append an uncommitted entry (open writer, write, but don't finish the batch)
    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer.open().unwrap();
    let redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::from_int64(1003),
        properties: vec![("name".to_string(), Value::string("uncommitted"))],
    };
    writer
        .append_entry(WalOpType::InsertVertex, 3, &to_allocvec(&redo).unwrap())
        .unwrap();
    writer.sync().unwrap();
    writer.close();

    let (replayed, _) = recover(&wal_dir, &data_dir);
    assert_eq!(
        replayed.len(),
        2,
        "only committed entries should be recovered"
    );
    assert_eq!(replayed[0].1, VertexId::from_int64(1001));
    assert_eq!(replayed[1].1, VertexId::from_int64(1002));
}

/// TC-CR03: Properties are preserved through recovery
#[test]
fn test_vertex_properties_preserved_after_recovery() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&wal_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer.open().unwrap();

    let redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::from_int64(1001),
        properties: vec![
            ("name".to_string(), Value::string("Alice")),
            ("age".to_string(), Value::Int(30)),
        ],
    };
    let payload = to_allocvec(&redo).unwrap();
    writer
        .append_transaction_batch(
            graphdb::core::types::TransactionId::new(1),
            vec![TransactionWalEntry {
                op_type: WalOpType::InsertVertex,
                timestamp: 1,
                payload,
            }],
            &[],
        )
        .unwrap();
    writer.sync().unwrap();
    writer.close();

    let (_, props) = recover(&wal_dir, &data_dir);
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].len(), 2);
    assert!(props[0]
        .iter()
        .any(|(k, v)| k == "name" && v == &Value::string("Alice")));
    assert!(props[0]
        .iter()
        .any(|(k, v)| k == "age" && v == &Value::Int(30)));
}

/// TC-CR04: Corrupted trailing bytes should not prevent recovery of committed entries
#[test]
fn test_corrupted_trailing_bytes_recovery() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&wal_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    // Write committed entries
    write_wal_entries(&wal_dir, &[(1, 1, 1001, "before_crash")]).unwrap();

    // Open a new writer and write an incomplete entry, then truncate to simulate corruption
    let wal_uri = wal_dir.to_string_lossy().to_string();
    let mut writer = LocalWalWriter::new(&wal_uri, 0);
    writer.open().unwrap();
    let redo = InsertVertexRedo {
        label: 1,
        vid: VertexId::from_int64(1002),
        properties: vec![("name".to_string(), Value::string("corrupted"))],
    };
    writer
        .append_entry(WalOpType::InsertVertex, 2, &to_allocvec(&redo).unwrap())
        .unwrap();
    writer.sync().unwrap();
    writer.close();

    // Truncate the last 10 bytes of the WAL file to simulate crash
    let wal_entries: Vec<_> = std::fs::read_dir(&wal_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("wal"))
        .collect();
    for entry in wal_entries {
        let path = entry.path();
        let meta = std::fs::metadata(&path).unwrap();
        if meta.len() > 20 {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_len(meta.len() - 10).unwrap();
            drop(file);
        }
    }

    // Recovery should succeed; at minimum the committed entry is recovered
    let (replayed, _) = recover(&wal_dir, &data_dir);
    assert!(
        !replayed.is_empty(),
        "committed entry should survive corruption"
    );
    assert_eq!(replayed[0].1, VertexId::from_int64(1001));
}

/// TC-CR05: Multiple committed batches across timestamp ranges
#[test]
fn test_multiple_timestamp_ranges_recovery() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&wal_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    write_wal_entries(
        &wal_dir,
        &[
            (10, 1, 1001, "batch1_a"),
            (20, 1, 1002, "batch2_a"),
            (30, 1, 1003, "batch3_a"),
        ],
    )
    .unwrap();

    let (replayed, _) = recover(&wal_dir, &data_dir);
    assert_eq!(replayed.len(), 3);
    // Timestamps should be in order
    assert_eq!(replayed[0].2, 10);
    assert_eq!(replayed[1].2, 20);
    assert_eq!(replayed[2].2, 30);
}

/// TC-CR06: Empty WAL directory should recover with zero entries
#[test]
fn test_empty_wal_recovery() {
    let tmp = TempDir::new().unwrap();
    let wal_dir = tmp.path().join("wal");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&wal_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let (replayed, _) = recover(&wal_dir, &data_dir);
    assert_eq!(replayed.len(), 0);
}

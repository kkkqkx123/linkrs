use std::path::PathBuf;

use postcard::from_bytes;
use serde::de::DeserializeOwned;

use crate::core::types::Timestamp;
use crate::core::{StorageError, StorageResult};
use crate::transaction::wal::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeIndexRedo,
    CreateEdgeTypeRedo, CreateSpaceRedo, CreateTagIndexRedo, CreateVertexTypeRedo,
    DeleteEdgePropRedo, DeleteEdgeRedo, DeleteEdgeTypeRedo, DeleteVertexPropRedo, DeleteVertexRedo,
    DeleteVertexTypeRedo, DropEdgeIndexRedo, DropSpaceRedo, DropTagIndexRedo, InsertEdgeRedo,
    InsertVertexRedo, LocalWalParser, Lsn, ParallelWalParser, ParsedWalEntry, RecoveryResult,
    RenameEdgePropRedo, RenameVertexPropRedo, UpdateEdgePropRedo, UpdateVertexPropRedo, WalOpType,
    WalParser, WalRecoveryMode,
};

macro_rules! recovery_arm_ref {
    ($applier:expr, $op:expr, $entry:expr, $payload:expr, $ts:expr, $stats:expr, $redo_type:ty, $replay_fn:ident) => {{
        match deserialize_redo::<$redo_type>($payload) {
            Ok(redo) => {
                $applier.$replay_fn(&redo, $ts)?;
                $stats.wal_entries_replayed += 1;
                $stats.last_lsn = $entry.lsn;
            }
            Err(e) => {
                return Err(recovery_deserialize_error(
                    &mut $stats, $entry.lsn, $op, e,
                ))
            }
        }
    }};
}

#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    pub wal_dir: PathBuf,
    pub data_dir: PathBuf,
    pub recovery_mode: WalRecoveryMode,
    pub parallel_recovery: bool,
    pub verify_checksum: bool,
    pub start_lsn: Option<Lsn>,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            wal_dir: PathBuf::from("./data/wal"),
            data_dir: PathBuf::from("./data"),
            recovery_mode: WalRecoveryMode::default(),
            parallel_recovery: true,
            verify_checksum: true,
            start_lsn: None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct RecoveryStats {
    pub wal_entries_replayed: usize,
    pub pages_restored: usize,
    pub checkpoints_processed: usize,
    pub recovery_time_ms: u64,
    pub errors_encountered: usize,
    pub last_lsn: Lsn,
    pub max_timestamp: Timestamp,
    pub max_transaction_id: u64,
}

pub use crate::core::wal::traits::RecoveryApplier;

pub struct RecoveryManager {
    config: RecoveryConfig,
    stats: RecoveryStats,
}

fn recovery_deserialize_error(
    stats: &mut RecoveryStats,
    lsn: Lsn,
    op_type: WalOpType,
    error: StorageError,
) -> StorageError {
    stats.errors_encountered += 1;
    StorageError::wal_error(format!(
        "Failed to deserialize {} at {}: {}",
        op_type, lsn, error
    ))
}

fn deserialize_redo<T: DeserializeOwned>(payload: &[u8]) -> StorageResult<T> {
    from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
}

impl RecoveryManager {
    pub fn new(config: RecoveryConfig) -> Self {
        Self {
            config,
            stats: RecoveryStats::default(),
        }
    }

    pub fn recover_with_applier(
        &mut self,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<RecoveryStats> {
        let start = std::time::Instant::now();

        self.stats = RecoveryStats::default();
        self.stats.last_lsn = self.config.start_lsn.unwrap_or(Lsn::ZERO);

        let wal_result = self.parse_wal_files()?;
        self.stats.max_timestamp = wal_result.last_timestamp;
        self.stats.max_transaction_id = self.max_transaction_id(&wal_result);
        self.stats.errors_encountered = wal_result
            .corrupted_count
            .saturating_add(wal_result.skipped_count);
        if self.stats.errors_encountered > 0 {
            log::warn!(
                "WAL recovery found {} recoverable tail/corruption markers",
                self.stats.errors_encountered
            );
        }

        self.restore_from_checkpoint(&wal_result)?;
        self.replay_wal_entries(&wal_result, applier)?;
        self.stats.recovery_time_ms = start.elapsed().as_millis() as u64;

        Ok(self.stats.clone())
    }

    fn max_transaction_id(&self, wal_result: &RecoveryResult) -> u64 {
        wal_result
            .all_entries
            .iter()
            .filter_map(|entry| {
                let op_type = WalOpType::try_from(entry.header.op_type).ok()?;
                match op_type {
                    WalOpType::OutboxIntent => {
                        from_bytes::<crate::core::wal::OutboxIntent>(&entry.payload)
                            .ok()
                            .map(|intent| intent.transaction_id.as_u64())
                    }
                    WalOpType::TransactionCommit => {
                        from_bytes::<crate::core::wal::TransactionCommit>(&entry.payload)
                            .ok()
                            .map(|commit| commit.transaction_id.as_u64())
                    }
                    WalOpType::TransactionAbort => {
                        from_bytes::<crate::core::wal::TransactionAbort>(&entry.payload)
                            .ok()
                            .map(|abort| abort.transaction_id.as_u64())
                    }
                    _ => None,
                }
            })
            .max()
            .unwrap_or(0)
    }

    fn parse_wal_files(&self) -> StorageResult<RecoveryResult> {
        if self.config.parallel_recovery {
            let parser = ParallelWalParser::new()
                .with_recovery_mode(self.config.recovery_mode)
                .with_verify_checksum(self.config.verify_checksum);
            parser
                .parse_parallel(&self.config.wal_dir)
                .map_err(|e| StorageError::db_error(format!("WAL parse error: {}", e)))
        } else {
            let mut parser = LocalWalParser::new();
            parser
                .open(&self.config.wal_dir.to_string_lossy())
                .map_err(|e| StorageError::db_error(format!("WAL open error: {}", e)))?;
            Ok(RecoveryResult {
                all_entries: parser.parse_all_entries(),
                last_timestamp: parser.last_timestamp(),
                last_lsn: parser.last_lsn(),
                corrupted_count: parser.corrupted_count(),
                skipped_count: parser.skipped_count(),
            })
        }
    }

    fn restore_from_checkpoint(&mut self, _wal_result: &RecoveryResult) -> StorageResult<()> {
        if !self.config.data_dir.exists() {
            std::fs::create_dir_all(&self.config.data_dir)?;
            return Ok(());
        }
        self.stats.checkpoints_processed = 1;
        Ok(())
    }

    fn replay_wal_entries(
        &mut self,
        wal_result: &RecoveryResult,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        let has_sync_envelope = wal_result.all_entries.iter().any(|entry| {
            matches!(
                WalOpType::try_from(entry.header.op_type),
                Ok(WalOpType::OutboxIntent
                    | WalOpType::TransactionCommit
                    | WalOpType::TransactionAbort)
            )
        });
        if !has_sync_envelope {
            self.replay_parsed_entries(&wal_result.all_entries, applier)?;
            self.stats.last_lsn = wal_result.last_lsn;
            return Ok(());
        }
        let transactions =
            crate::transaction::wal::collect_committed_transactions(&wal_result.all_entries)
                .map_err(|error| {
                    StorageError::wal_error(format!(
                        "Failed to validate committed WAL batches: {}",
                        error
                    ))
                })?;
        for transaction in transactions {
            self.replay_parsed_entries(&transaction.redo_entries, applier)?;
            self.stats.last_lsn = Lsn::new(transaction.commit_lsn.get());
        }
        Ok(())
    }

    fn replay_parsed_entries(
        &mut self,
        entries: &[ParsedWalEntry],
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        for entry in entries {
            if let Some(start_lsn) = self.config.start_lsn {
                if entry.lsn <= start_lsn {
                    continue;
                }
            }

            let op_type = match WalOpType::try_from(entry.header.op_type) {
                Ok(t) => t,
                Err(error) => {
                    self.stats.errors_encountered += 1;
                    return Err(StorageError::wal_error(format!(
                        "Invalid WAL operation at {}: {}",
                        entry.lsn, error
                    )));
                }
            };

            let ts = entry.header.timestamp;
            self.stats.max_timestamp = self.stats.max_timestamp.max(ts);
            let payload = &entry.payload;

            match op_type {
                WalOpType::InsertVertex => {
                    let redo: InsertVertexRedo = deserialize_redo(payload)?;
                    applier.replay_insert_vertex(redo.label, redo.vid, &redo.properties, ts)?;
                    self.stats.wal_entries_replayed += 1;
                    self.stats.last_lsn = entry.lsn;
                }
                WalOpType::InsertEdge => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        InsertEdgeRedo, replay_insert_edge)
                }
                WalOpType::UpdateVertexProp => {
                    let redo: UpdateVertexPropRedo = deserialize_redo(payload)?;
                    applier.replay_update_vertex_prop(
                        redo.label,
                        redo.vid,
                        &redo.prop_name,
                        &redo.value,
                        ts,
                    )?;
                    self.stats.wal_entries_replayed += 1;
                    self.stats.last_lsn = entry.lsn;
                }
                WalOpType::UpdateEdgeProp => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        UpdateEdgePropRedo, replay_update_edge_prop)
                }
                WalOpType::DeleteVertex => {
                    let redo: DeleteVertexRedo = deserialize_redo(payload)?;
                    applier.replay_delete_vertex(redo.label, redo.vid, ts)?;
                    self.stats.wal_entries_replayed += 1;
                    self.stats.last_lsn = entry.lsn;
                }
                WalOpType::DeleteEdge => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DeleteEdgeRedo, replay_delete_edge)
                }
                WalOpType::CreateVertexType => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        CreateVertexTypeRedo, replay_create_vertex_type)
                }
                WalOpType::CreateEdgeType => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        CreateEdgeTypeRedo, replay_create_edge_type)
                }
                WalOpType::DeleteVertexType => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DeleteVertexTypeRedo, replay_delete_vertex_type)
                }
                WalOpType::DeleteEdgeType => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DeleteEdgeTypeRedo, replay_delete_edge_type)
                }
                WalOpType::CreateSpace => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        CreateSpaceRedo, replay_create_space)
                }
                WalOpType::DropSpace => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DropSpaceRedo, replay_drop_space)
                }
                WalOpType::ClearSpace => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        ClearSpaceRedo, replay_clear_space)
                }
                WalOpType::AlterSpaceComment => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        AlterSpaceCommentRedo, replay_alter_space_comment)
                }
                WalOpType::AddVertexProp => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        AddVertexPropRedo, replay_add_vertex_prop)
                }
                WalOpType::AddEdgeProp => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        AddEdgePropRedo, replay_add_edge_prop)
                }
                WalOpType::DeleteVertexProp => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DeleteVertexPropRedo, replay_delete_vertex_prop)
                }
                WalOpType::DeleteEdgeProp => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DeleteEdgePropRedo, replay_delete_edge_prop)
                }
                WalOpType::RenameVertexProp => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        RenameVertexPropRedo, replay_rename_vertex_prop)
                }
                WalOpType::RenameEdgeProp => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        RenameEdgePropRedo, replay_rename_edge_prop)
                }
                WalOpType::CreateTagIndex => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        CreateTagIndexRedo, replay_create_tag_index)
                }
                WalOpType::DropTagIndex => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DropTagIndexRedo, replay_drop_tag_index)
                }
                WalOpType::CreateEdgeIndex => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        CreateEdgeIndexRedo, replay_create_edge_index)
                }
                WalOpType::DropEdgeIndex => {
                    recovery_arm_ref!(applier, op_type, entry, payload, ts, self.stats,
                        DropEdgeIndexRedo, replay_drop_edge_index)
                }
                WalOpType::Compact => {
                    applier.replay_compact(ts)?;
                    self.stats.wal_entries_replayed += 1;
                }
                WalOpType::OutboxIntent
                | WalOpType::TransactionCommit
                | WalOpType::TransactionAbort => {}
            }
        }

        Ok(())
    }

    pub fn stats(&self) -> &RecoveryStats {
        &self.stats
    }

    pub fn needs_recovery(&self) -> bool {
        self.config.wal_dir.exists()
            && std::fs::read_dir(&self.config.wal_dir)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false)
    }

    pub fn clear_wal_files(&self) -> StorageResult<()> {
        if !self.config.wal_dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.config.wal_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "wal") {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self::new(RecoveryConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::transaction::wal::writer::LocalWalWriter;
    use crate::transaction::wal::writer::WalWriter;
    use crate::transaction::wal::{
        InsertVertexRedo, LabelId, Timestamp, TransactionWalEntry, VertexId, WalOpType,
    };
    use postcard::to_allocvec;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

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
            _properties: &[(String, Value)],
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
            vid: VertexId::from_int64(vid),
            properties: vec![("name".to_string(), Value::string(name))],
        };

        let payload =
            to_allocvec(&redo).map_err(|e| StorageError::serialize_error(e.to_string()))?;
        let lsn = writer
            .append_transaction_batch(
                crate::core::types::TransactionId::new(timestamp),
                vec![TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp,
                    payload,
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
            vid: VertexId::from_int64(1001),
            properties: vec![("name".to_string(), Value::string("Alice"))],
        };
        let first_payload = to_allocvec(&first_redo).expect("Failed to serialize first redo");
        let first_lsn = writer
            .append_transaction_batch(
                crate::core::types::TransactionId::new(1),
                vec![TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp: 1,
                    payload: first_payload,
                }],
                &[],
            )
            .expect("Failed to append first WAL transaction");

        let second_redo = InsertVertexRedo {
            label: 1,
            vid: VertexId::from_int64(1002),
            properties: vec![("name".to_string(), Value::string("Bob"))],
        };
        let second_payload = to_allocvec(&second_redo).expect("Failed to serialize second redo");
        let second_lsn = writer
            .append_transaction_batch(
                crate::core::types::TransactionId::new(2),
                vec![TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp: 2,
                    payload: second_payload,
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
        });

        let applier = RecordingApplier::default();
        let stats = manager
            .recover_with_applier(&applier)
            .expect("Recovery should succeed");

        let replayed = applier.replayed_vertices();
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].0, 1);
        assert_eq!(replayed[0].1, VertexId::from_int64(1002));
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

        let last_lsn = write_insert_vertex_wal(&wal_dir, 1, 1, 1001, "Alice")
            .expect("Failed to write WAL entry");

        let mut manager = RecoveryManager::new(RecoveryConfig {
            wal_dir: wal_dir.clone(),
            data_dir: data_dir.clone(),
            recovery_mode: WalRecoveryMode::default(),
            parallel_recovery: false,
            verify_checksum: true,
            start_lsn: Some(last_lsn),
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
            vid: VertexId::from_int64(1002),
            properties: vec![("name".to_string(), Value::string("uncommitted"))],
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
        });
        let applier = RecordingApplier::default();
        manager
            .recover_with_applier(&applier)
            .expect("recovery should succeed");
        let replayed = applier.replayed_vertices();
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].1, VertexId::from_int64(1001));
    }
}

//! Recovery Manager
//!
//! Provides crash recovery functionality using WAL replay.

use std::path::PathBuf;

use postcard::from_bytes;

use crate::core::types::Timestamp;
use crate::core::{StorageError, StorageResult};
use crate::transaction::wal::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeTypeRedo,
    CreateSpaceRedo, CreateVertexTypeRedo, DeleteEdgePropRedo, DeleteEdgeRedo, DeleteEdgeTypeRedo,
    DeleteVertexPropRedo, DeleteVertexRedo, DeleteVertexTypeRedo, DropSpaceRedo, InsertEdgeRedo,
    InsertVertexRedo, LocalWalParser, Lsn, ParallelWalParser, ParsedWalEntry, RecoveryResult,
    RenameEdgePropRedo, RenameVertexPropRedo, UpdateEdgePropRedo, UpdateVertexPropRedo, WalOpType,
    WalParser, WalRecoveryMode,
};

/// Recovery configuration
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

/// Recovery statistics
#[derive(Debug, Default, Clone)]
pub struct RecoveryStats {
    pub wal_entries_replayed: usize,
    pub pages_restored: usize,
    pub checkpoints_processed: usize,
    pub recovery_time_ms: u64,
    pub errors_encountered: usize,
    pub last_lsn: crate::transaction::wal::Lsn,
    pub max_timestamp: Timestamp,
}

/// Trait for applying recovered operations to the storage engine.
pub use crate::core::wal::traits::RecoveryApplier;

/// Recovery manager for crash recovery
pub struct RecoveryManager {
    config: RecoveryConfig,
    stats: RecoveryStats,
}

impl RecoveryManager {
    pub fn new(config: RecoveryConfig) -> Self {
        Self {
            config,
            stats: RecoveryStats::default(),
        }
    }

    /// Perform crash recovery with a RecoveryApplier for WAL replay
    pub fn recover_with_applier(
        &mut self,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<RecoveryStats> {
        let start = std::time::Instant::now();

        self.stats = RecoveryStats::default();
        self.stats.last_lsn = self.config.start_lsn.unwrap_or(Lsn::ZERO);

        let wal_result = self.parse_wal_files()?;

        self.restore_from_checkpoint(&wal_result)?;

        self.replay_wal_entries(&wal_result, applier)?;

        self.stats.recovery_time_ms = start.elapsed().as_millis() as u64;

        Ok(self.stats.clone())
    }

    /// Parse WAL files
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

    /// Restore from checkpoint
    fn restore_from_checkpoint(&mut self, _wal_result: &RecoveryResult) -> StorageResult<()> {
        if !self.config.data_dir.exists() {
            std::fs::create_dir_all(&self.config.data_dir)?;
            return Ok(());
        }

        self.stats.checkpoints_processed = 1;

        Ok(())
    }

    /// Replay WAL entries using a RecoveryApplier
    fn replay_wal_entries(
        &mut self,
        wal_result: &RecoveryResult,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        self.replay_parsed_entries(&wal_result.all_entries, applier)
    }

    /// Replay parsed WAL entries (new format)
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
                Err(_) => {
                    self.stats.errors_encountered += 1;
                    continue;
                }
            };

            let ts = entry.header.timestamp;
            self.stats.max_timestamp = self.stats.max_timestamp.max(ts);
            let payload = &entry.payload;

            match op_type {
                WalOpType::InsertVertex => match self.deserialize_insert_vertex(payload) {
                    Ok(redo) => {
                        applier.replay_insert_vertex(redo.label, redo.vid, &redo.properties, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize InsertVertex redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::InsertEdge => match self.deserialize_insert_edge(payload) {
                    Ok(redo) => {
                        applier.replay_insert_edge(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize InsertEdge redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::UpdateVertexProp => match self.deserialize_update_vertex_prop(payload) {
                    Ok(redo) => {
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
                    Err(e) => {
                        log::warn!("Failed to deserialize UpdateVertexProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::UpdateEdgeProp => match self.deserialize_update_edge_prop(payload) {
                    Ok(redo) => {
                        applier.replay_update_edge_prop(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize UpdateEdgeProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::DeleteVertex => match self.deserialize_delete_vertex(payload) {
                    Ok(redo) => {
                        applier.replay_delete_vertex(redo.label, redo.vid, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize DeleteVertex redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::DeleteEdge => match self.deserialize_delete_edge(payload) {
                    Ok(redo) => {
                        applier.replay_delete_edge(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize DeleteEdge redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::CreateVertexType => match self.deserialize_create_vertex_type(payload) {
                    Ok(redo) => {
                        applier.replay_create_vertex_type(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize CreateVertexType redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::CreateEdgeType => match self.deserialize_create_edge_type(payload) {
                    Ok(redo) => {
                        applier.replay_create_edge_type(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize CreateEdgeType redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::DeleteVertexType => match self.deserialize_delete_vertex_type(payload) {
                    Ok(redo) => {
                        applier.replay_delete_vertex_type(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize DeleteVertexType redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::DeleteEdgeType => match self.deserialize_delete_edge_type(payload) {
                    Ok(redo) => {
                        applier.replay_delete_edge_type(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize DeleteEdgeType redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::CreateSpace => match self.deserialize_create_space(payload) {
                    Ok(redo) => {
                        applier.replay_create_space(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize CreateSpace redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::DropSpace => match self.deserialize_drop_space(payload) {
                    Ok(redo) => {
                        applier.replay_drop_space(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize DropSpace redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::ClearSpace => match self.deserialize_clear_space(payload) {
                    Ok(redo) => {
                        applier.replay_clear_space(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize ClearSpace redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::AlterSpaceComment => {
                    match self.deserialize_alter_space_comment(payload) {
                        Ok(redo) => {
                            applier.replay_alter_space_comment(&redo, ts)?;
                            self.stats.wal_entries_replayed += 1;
                            self.stats.last_lsn = entry.lsn;
                        }
                        Err(e) => {
                            log::warn!("Failed to deserialize AlterSpaceComment redo: {}", e);
                            self.stats.errors_encountered += 1;
                        }
                    }
                }
                WalOpType::AddVertexProp => match self.deserialize_add_vertex_prop(payload) {
                    Ok(redo) => {
                        applier.replay_add_vertex_prop(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize AddVertexProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::AddEdgeProp => match self.deserialize_add_edge_prop(payload) {
                    Ok(redo) => {
                        applier.replay_add_edge_prop(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize AddEdgeProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::DeleteVertexProp => match self.deserialize_delete_vertex_prop(payload) {
                    Ok(redo) => {
                        applier.replay_delete_vertex_prop(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize DeleteVertexProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::DeleteEdgeProp => match self.deserialize_delete_edge_prop(payload) {
                    Ok(redo) => {
                        applier.replay_delete_edge_prop(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize DeleteEdgeProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::RenameVertexProp => match self.deserialize_rename_vertex_prop(payload) {
                    Ok(redo) => {
                        applier.replay_rename_vertex_prop(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize RenameVertexProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::RenameEdgeProp => match self.deserialize_rename_edge_prop(payload) {
                    Ok(redo) => {
                        applier.replay_rename_edge_prop(&redo, ts)?;
                        self.stats.wal_entries_replayed += 1;
                        self.stats.last_lsn = entry.lsn;
                    }
                    Err(e) => {
                        log::warn!("Failed to deserialize RenameEdgeProp redo: {}", e);
                        self.stats.errors_encountered += 1;
                    }
                },
                WalOpType::Compact => {
                    applier.replay_compact(ts)?;
                    self.stats.wal_entries_replayed += 1;
                }
            }
        }

        Ok(())
    }

    fn deserialize_insert_vertex(&self, payload: &[u8]) -> StorageResult<InsertVertexRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_insert_edge(&self, payload: &[u8]) -> StorageResult<InsertEdgeRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_update_vertex_prop(
        &self,
        payload: &[u8],
    ) -> StorageResult<UpdateVertexPropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_update_edge_prop(&self, payload: &[u8]) -> StorageResult<UpdateEdgePropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_delete_vertex(&self, payload: &[u8]) -> StorageResult<DeleteVertexRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_delete_edge(&self, payload: &[u8]) -> StorageResult<DeleteEdgeRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_create_vertex_type(
        &self,
        payload: &[u8],
    ) -> StorageResult<CreateVertexTypeRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_create_edge_type(&self, payload: &[u8]) -> StorageResult<CreateEdgeTypeRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_delete_vertex_type(
        &self,
        payload: &[u8],
    ) -> StorageResult<DeleteVertexTypeRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_delete_edge_type(&self, payload: &[u8]) -> StorageResult<DeleteEdgeTypeRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_create_space(&self, payload: &[u8]) -> StorageResult<CreateSpaceRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_drop_space(&self, payload: &[u8]) -> StorageResult<DropSpaceRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_clear_space(&self, payload: &[u8]) -> StorageResult<ClearSpaceRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_alter_space_comment(
        &self,
        payload: &[u8],
    ) -> StorageResult<AlterSpaceCommentRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_add_vertex_prop(&self, payload: &[u8]) -> StorageResult<AddVertexPropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_add_edge_prop(&self, payload: &[u8]) -> StorageResult<AddEdgePropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_delete_vertex_prop(
        &self,
        payload: &[u8],
    ) -> StorageResult<DeleteVertexPropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_delete_edge_prop(&self, payload: &[u8]) -> StorageResult<DeleteEdgePropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_rename_vertex_prop(
        &self,
        payload: &[u8],
    ) -> StorageResult<RenameVertexPropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    fn deserialize_rename_edge_prop(&self, payload: &[u8]) -> StorageResult<RenameEdgePropRedo> {
        from_bytes(payload).map_err(|e| StorageError::deserialize_error(e.to_string()))
    }

    /// Get recovery statistics
    pub fn stats(&self) -> &RecoveryStats {
        &self.stats
    }

    /// Check if recovery is needed
    pub fn needs_recovery(&self) -> bool {
        self.config.wal_dir.exists()
            && std::fs::read_dir(&self.config.wal_dir)
                .map(|entries| entries.count() > 0)
                .unwrap_or(false)
    }

    /// Clear WAL files after successful recovery
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
    use crate::transaction::wal::writer::LocalWalWriter;
    use crate::transaction::wal::writer::WalWriter;
    use crate::transaction::wal::{InsertVertexRedo, LabelId, Timestamp, VertexId, WalOpType};
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
            _properties: &[(String, Vec<u8>)],
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
                value: &[u8],
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
        }
    }

    fn write_insert_vertex_wal(
        wal_dir: &Path,
        timestamp: u32,
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
            properties: vec![(
                "name".to_string(),
                to_allocvec(&name.to_string())
                    .map_err(|e| StorageError::serialize_error(e.to_string()))?,
            )],
        };

        let payload =
            to_allocvec(&redo).map_err(|e| StorageError::serialize_error(e.to_string()))?;
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
            properties: vec![("name".to_string(), b"Alice".to_vec())],
        };
        let first_payload = to_allocvec(&first_redo).expect("Failed to serialize first redo");
        writer
            .append_entry(WalOpType::InsertVertex, 1, &first_payload)
            .expect("Failed to append first WAL entry");
        let first_lsn = writer.current_lsn();

        let second_redo = InsertVertexRedo {
            label: 1,
            vid: VertexId::from_int64(1002),
            properties: vec![("name".to_string(), b"Bob".to_vec())],
        };
        let second_payload = to_allocvec(&second_redo).expect("Failed to serialize second redo");
        writer
            .append_entry(WalOpType::InsertVertex, 2, &second_payload)
            .expect("Failed to append second WAL entry");
        let second_lsn = writer.current_lsn();
        writer.close();

        let mut manager = RecoveryManager::new(RecoveryConfig {
            wal_dir: wal_dir.clone(),
            data_dir: data_dir.clone(),
            recovery_mode: WalRecoveryMode::default(),
            parallel_recovery: false,
            verify_checksum: true,
            start_lsn: Some(first_lsn),
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
        assert_eq!(stats.last_lsn, second_lsn);
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
}

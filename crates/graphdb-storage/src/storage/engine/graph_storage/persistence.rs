use std::path::{Path, PathBuf};

use crate::core::types::{CompactTarget, CompactConfig};
use crate::core::{StorageError, StorageResult};
use crate::storage::engine::paths::StoragePaths;
use crate::storage::engine::persistence_coordinator::{
    CheckpointData, CheckpointInfo, CheckpointStats,
};
use crate::transaction::compact_transaction::CompactTransaction;
use crate::transaction::wal::recovery::{RecoveryConfig, RecoveryManager, RecoveryStats};
use crate::transaction::wal::{Lsn, ParallelWalParser, WalRecoveryMode};

use super::context::GraphStorageContext;

fn load_schema_and_index_metadata(ctx: &GraphStorageContext) -> StorageResult<()> {
    if let Some(path) = ctx.work_dir().as_ref() {
        let paths = StoragePaths::new(path.clone());

        let schema_path = paths.schema_file();
        if schema_path.exists() {
            ctx.schema_manager().load_schema(&schema_path)?;
        }

        let index_meta_path = paths.index_meta_file();
        if index_meta_path.exists() {
            ctx.index_metadata_manager()
                .load_indexes(&index_meta_path)?;
        }
    }

    Ok(())
}

fn restore_full_state_from_disk(ctx: &GraphStorageContext) -> StorageResult<()> {
    if let Some(path) = ctx.work_dir().as_ref() {
        let paths = StoragePaths::new(path.clone());
        ctx.restore_from_checkpoint(path)?;
        ctx.user_storage().load_from_dir(paths.data_dir())?;

        let index_path = paths.indexes_dir();
        if index_path.exists() {
            ctx.index_data_manager().write().load(&index_path)?;
        }
    }

    Ok(())
}

pub(crate) fn bootstrap_from_disk(ctx: &GraphStorageContext) -> StorageResult<()> {
    load_schema_and_index_metadata(ctx)?;
    super::schema_writer::ensure_graph_types_from_schema(ctx)?;

    let checkpoint_info = load_latest_checkpoint(ctx)?;
    if let Some(ref info) = checkpoint_info {
        // Initialize the version manager with the checkpoint timestamp so that
        // persisted data (written at timestamps <= checkpoint timestamp) is visible
        // after reload. Without this, the fresh version manager's read_ts=1 would
        // not see data written at higher timestamps.
        let thread_num = 1;
        ctx.version_manager().init_ts(info.timestamp, thread_num);
    } else {
        restore_full_state_from_disk(ctx)?;
        // If data was restored from the main data directory (no checkpoints),
        // we can't recover the max timestamp. Use a default that ensures
        // data at ts=1 is visible (the minimum write timestamp).
        ctx.version_manager().init_ts(1, 1);
    }

    Ok(())
}

pub(crate) fn initialize_with_recovery(
    ctx: &GraphStorageContext,
) -> StorageResult<Option<RecoveryStats>> {
    bootstrap_from_disk(ctx)?;

    if !needs_recovery(ctx) {
        return Ok(None);
    }

    log::info!("WAL recovery needed, starting recovery...");
    let stats = recover_from_wal(ctx)?;

    log::info!(
        "WAL recovery completed: {} entries replayed in {}ms",
        stats.wal_entries_replayed,
        stats.recovery_time_ms
    );

    Ok(Some(stats))
}

pub(crate) fn save_data(ctx: &GraphStorageContext) -> StorageResult<()> {
    let paths = ctx
        .storage_paths()
        .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))?;

    save_data_to_dir(ctx, paths.root())
}

pub(crate) fn save_data_to_dir(ctx: &GraphStorageContext, dir: &Path) -> StorageResult<()> {
    use std::fs::{self, File};
    use std::io::Write;

    let paths = StoragePaths::new(dir);
    let data_dir = paths.data_dir();
    fs::create_dir_all(&data_dir)?;

    let version_file = paths.version_file();
    let mut file = File::create(&version_file)?;
    writeln!(file, "1")?;

    ctx.flush_tables_to_dir(&data_dir)?;
    ctx.user_storage().save_to_dir(&data_dir)?;

    if let Some(persistence) = ctx.persistence().as_ref() {
        let wal_lsn = {
            let coordinator = persistence.read();
            coordinator
                .wal_manager()
                .map(|w| w.read().current_lsn())
                .unwrap_or(Lsn::ZERO)
        };
        persistence.read().mark_flushed(wal_lsn);
    }

    log::info!("Data saved to {:?}", data_dir);
    Ok(())
}

pub(crate) fn flush(ctx: &GraphStorageContext) -> StorageResult<()> {
    save_data(ctx)
}

pub(crate) fn create_checkpoint(
    ctx: &GraphStorageContext,
) -> StorageResult<Option<CheckpointStats>> {
    let persistence = match ctx.persistence().as_ref() {
        Some(p) => p,
        None => return Ok(None),
    };

    let ts = ctx.get_write_timestamp();
    let graph = ctx.clone();
    let user_storage = ctx.user_storage().clone();

    let result = persistence.read().create_checkpoint(
        |checkpoint_dir, _timestamp| {
            let data_dir = StoragePaths::new(checkpoint_dir).data_dir();
            std::fs::create_dir_all(&data_dir)?;

            graph.flush_tables_to_dir(&data_dir)?;
            user_storage.save_to_dir(&data_dir)?;

            let vertex_count = graph.total_vertex_count() as u64;
            let edge_count = graph.total_edge_count() as u64;

            let data_size = std::fs::metadata(&data_dir).map(|m| m.len()).unwrap_or(0);

            Ok(CheckpointData {
                vertex_count,
                edge_count,
                data_size,
            })
        },
        ts,
    );

    ctx.version_manager().release_insert_timestamp(ts);

    let stats = result?;

    Ok(Some(stats))
}

pub(crate) fn verify_snapshot(ctx: &GraphStorageContext, snapshot_id: u64) -> StorageResult<bool> {
    let persistence = ctx
        .persistence()
        .as_ref()
        .ok_or_else(|| StorageError::not_supported("Snapshots are not available"))?;

    persistence.read().verify_snapshot(snapshot_id)
}

pub(crate) fn cleanup_snapshots(ctx: &GraphStorageContext) -> StorageResult<usize> {
    let persistence = ctx
        .persistence()
        .as_ref()
        .ok_or_else(|| StorageError::not_supported("Snapshots are not available"))?;

    persistence.read().cleanup_old_snapshots()
}

pub(crate) fn snapshot_stats(ctx: &GraphStorageContext) -> crate::storage::SnapshotStats {
    ctx.persistence()
        .as_ref()
        .map(|persistence| persistence.read().snapshot_stats())
        .unwrap_or_default()
}

pub(crate) fn load_latest_checkpoint(
    ctx: &GraphStorageContext,
) -> StorageResult<Option<CheckpointInfo>> {
    let persistence = match &ctx.persistence() {
        Some(p) => p,
        None => return Ok(None),
    };

    let graph = ctx.clone();
    let user_storage = ctx.user_storage().clone();

    persistence
        .read()
        .load_latest_checkpoint(|checkpoint_dir| {
            graph.restore_from_checkpoint(checkpoint_dir)?;
            user_storage.load_from_dir(StoragePaths::new(checkpoint_dir).data_dir())
        })
        .map(|result| {
            if let Some(ref info) = result {
                persistence.read().mark_checkpointed(info.lsn);
            }
            result
        })
}

pub(crate) fn should_flush(ctx: &GraphStorageContext) -> bool {
    if let Some(persistence) = ctx.persistence().as_ref() {
        persistence.read().should_flush()
    } else {
        false
    }
}

pub(crate) fn should_checkpoint(ctx: &GraphStorageContext) -> bool {
    if let Some(persistence) = ctx.persistence().as_ref() {
        persistence.read().should_checkpoint()
    } else {
        false
    }
}

pub(crate) fn auto_flush_if_needed(ctx: &GraphStorageContext) -> StorageResult<bool> {
    if should_flush(ctx) {
        flush(ctx)?;
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn auto_checkpoint_if_needed(
    ctx: &GraphStorageContext,
) -> StorageResult<Option<CheckpointStats>> {
    if should_checkpoint(ctx) {
        let stats = create_checkpoint(ctx)?;
        return Ok(stats);
    }
    Ok(None)
}

pub(crate) fn compact_transactional(
    ctx: &GraphStorageContext,
    config: &CompactConfig,
) -> StorageResult<()> {
    let persistence = ctx.persistence().as_ref().ok_or_else(|| {
        StorageError::db_error("Persistence not available for transactional compaction".to_string())
    })?;

    let wal_writer = {
        let coordinator = persistence.read();
        let wal_mgr = coordinator.wal_manager();
        let wal_reader = wal_mgr
            .as_ref()
            .ok_or_else(|| StorageError::db_error("WAL not enabled".to_string()))?
            .read();
        wal_reader
            .writer()
            .ok_or_else(|| StorageError::db_error("WAL writer not initialized".to_string()))?
    };

    let mut wal_writer_guard = wal_writer.write();
    let version_manager = ctx.version_manager().as_ref();

    let txn = CompactTransaction::new(
        ctx,
        version_manager,
        &mut *wal_writer_guard,
        config,
    )
    .map_err(|e| StorageError::db_error(format!("Failed to create compact transaction: {}", e)))?;

    let before_stats = txn.storage_stats();
    log::info!(
        "Starting transactional compaction: enable_structure_compaction={}, config={{ segment_merge_enabled: {} }}, size={}/{}",
        config.enable_structure_compaction,
        config.segment_merge_enabled,
        before_stats.used_size,
        before_stats.total_size
    );

    txn.commit()
        .map_err(|e| StorageError::db_error(format!("Compact transaction failed: {}", e)))?;

    let after_stats = ctx.get_compact_stats();
    log::info!(
        "Compaction completed: size={}/{} (freed {} bytes)",
        after_stats.used_size,
        after_stats.total_size,
        before_stats.used_size.saturating_sub(after_stats.used_size)
    );

    Ok(())
}

pub(crate) fn load_from_disk(ctx: &GraphStorageContext) -> StorageResult<()> {
    load_schema_and_index_metadata(ctx)?;
    super::schema_writer::ensure_graph_types_from_schema(ctx)?;
    restore_full_state_from_disk(ctx)
}

pub(crate) fn save_to_disk(ctx: &GraphStorageContext) -> StorageResult<()> {
    if let Some(path) = ctx.work_dir().as_ref() {
        let paths = StoragePaths::new(path.clone());
        std::fs::create_dir_all(paths.root()).map_err(|e| StorageError::io_error(e.to_string()))?;

        let schema_dir = paths.schema_dir();
        std::fs::create_dir_all(&schema_dir).map_err(|e| StorageError::io_error(e.to_string()))?;
        let schema_path = paths.schema_file();
        ctx.schema_manager().save_schema(&schema_path)?;

        let index_meta_dir = paths.index_meta_dir();
        std::fs::create_dir_all(&index_meta_dir)
            .map_err(|e| StorageError::io_error(e.to_string()))?;
        let index_meta_path = paths.index_meta_file();
        ctx.index_metadata_manager()
            .save_indexes(&index_meta_path)?;

        save_data_to_dir(ctx, paths.root())?;

        let index_path = paths.indexes_dir();
        std::fs::create_dir_all(&index_path).map_err(|e| StorageError::io_error(e.to_string()))?;
        ctx.index_data_manager().read().flush(&index_path)?;
    }
    Ok(())
}

pub(crate) fn recover_from_wal(ctx: &GraphStorageContext) -> StorageResult<RecoveryStats> {
    let (wal_dir, data_dir, checkpoint_dir) = persistence_dirs(ctx)
        .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))?;

    let start_lsn = latest_checkpoint_info_from_dir(&checkpoint_dir)?.map(|info| info.lsn);
    let config = RecoveryConfig {
        wal_dir,
        data_dir,
        start_lsn,
        ..Default::default()
    };

    let mut manager = RecoveryManager::new(config);

    let stats = manager.recover_with_applier(ctx)?;

    // Phase 2: Replay deferred edge operations (two-phase recovery)
    ctx.replay_deferred_edges()?;

    // Set the WAL writer's LSN to the last replayed position so that
    // create_checkpoint records the correct LSN instead of the fresh WAL's 0.
    if let Some(persistence) = ctx.persistence() {
        let coordinator = persistence.read();
        if let Some(wal_mgr) = coordinator.wal_manager() {
            let _ = wal_mgr.write().set_current_lsn(stats.last_lsn);
        }
    }

    // Advance write_ts past the max replayed timestamp so create_checkpoint
    // allocates a timestamp >= all recovered data, making recovered data
    // visible after reload.
    let current_ts = ctx.version_manager().write_timestamp();
    if stats.max_timestamp >= current_ts {
        ctx.version_manager().init_ts(stats.max_timestamp, 1);
    }

    // Persist the recovered state as a new checkpoint baseline so the next
    // startup does not replay the same WAL range again.
    let _ = create_checkpoint(ctx)?;

    // Update read_ts so recovered data is visible to subsequent reads.
    let checkpoint_ts = ctx.version_manager().write_timestamp().saturating_sub(1);
    ctx.version_manager().init_ts(checkpoint_ts, 1);

    Ok(stats)
}

pub(crate) fn recover_from_wal_with_config(
    ctx: &GraphStorageContext,
    mut config: RecoveryConfig,
) -> StorageResult<RecoveryStats> {
    if config.start_lsn.is_none() {
        let (_, _, checkpoint_dir) = persistence_dirs(ctx)
            .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))?;
        config.start_lsn = latest_checkpoint_info_from_dir(&checkpoint_dir)?.map(|info| info.lsn);
    }

    let mut manager = RecoveryManager::new(config);

    let stats = manager.recover_with_applier(ctx)?;

    // Phase 2: Replay deferred edge operations (two-phase recovery)
    ctx.replay_deferred_edges()?;

    // Set the WAL writer's LSN to the last replayed position so that
    // create_checkpoint records the correct LSN instead of the fresh WAL's 0.
    if let Some(persistence) = ctx.persistence() {
        let coordinator = persistence.read();
        if let Some(wal_mgr) = coordinator.wal_manager() {
            let _ = wal_mgr.write().set_current_lsn(stats.last_lsn);
        }
    }

    // Advance write_ts past the max replayed timestamp so create_checkpoint
    // allocates a timestamp >= all recovered data.
    let current_ts = ctx.version_manager().write_timestamp();
    if stats.max_timestamp >= current_ts {
        ctx.version_manager().init_ts(stats.max_timestamp, 1);
    }

    // Persist the recovered state as a new checkpoint baseline so the next
    // startup does not replay the same WAL range again.
    let _ = create_checkpoint(ctx)?;

    // Update read_ts so recovered data is visible to subsequent reads.
    let checkpoint_ts = ctx.version_manager().write_timestamp().saturating_sub(1);
    ctx.version_manager().init_ts(checkpoint_ts, 1);

    Ok(stats)
}

pub(crate) fn needs_recovery(ctx: &GraphStorageContext) -> bool {
    if let Some((wal_dir, _, checkpoint_dir)) = persistence_dirs(ctx) {
        if wal_dir.exists() {
            let latest_checkpoint_lsn = latest_checkpoint_info_from_dir(&checkpoint_dir)
                .ok()
                .flatten()
                .map(|info| info.lsn)
                .unwrap_or(Lsn::ZERO);

            match ParallelWalParser::new()
                .with_recovery_mode(WalRecoveryMode::default())
                .parse_parallel(&wal_dir)
            {
                Ok(result) => {
                    return result.last_lsn > latest_checkpoint_lsn;
                }
                Err(_) => {
                    return true;
                }
            }
        }
    }
    false
}

fn latest_checkpoint_info_from_dir(
    checkpoints_dir: &Path,
) -> StorageResult<Option<CheckpointInfo>> {
    if !checkpoints_dir.exists() {
        return Ok(None);
    }

    let mut checkpoints: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(checkpoints_dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name()?.to_string_lossy();
                if name.starts_with("checkpoint_") {
                    let id: u64 = name.trim_start_matches("checkpoint_").parse().ok()?;
                    Some((id, path))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    checkpoints.sort_by_key(|(id, _)| std::cmp::Reverse(*id));

    if let Some((_, checkpoint_path)) = checkpoints.first() {
        return read_checkpoint_metadata(checkpoint_path).map(Some);
    }

    Ok(None)
}

fn persistence_dirs(ctx: &GraphStorageContext) -> Option<(PathBuf, PathBuf, PathBuf)> {
    if let Some(persistence) = ctx.persistence().as_ref() {
        let coordinator = persistence.read();
        Some((
            coordinator.wal_dir(),
            coordinator.data_dir(),
            coordinator.checkpoint_dir(),
        ))
    } else {
        ctx.storage_paths().map(|paths| {
            let root = paths.root().to_path_buf();
            (paths.wal_dir(), paths.data_dir(), root.join("checkpoint"))
        })
    }
}

fn read_checkpoint_metadata(dir: &Path) -> StorageResult<CheckpointInfo> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let metadata_path = dir.join("checkpoint.meta");
    let file = File::open(metadata_path)?;
    let reader = BufReader::new(file);

    let mut checkpoint_id: Option<u64> = None;
    let mut lsn: Option<u64> = None;
    let mut timestamp: Option<u32> = None;

    for line in reader.lines() {
        let line = line?;
        let parts: Vec<&str> = line.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(StorageError::deserialize_error(format!(
                "Invalid checkpoint metadata line: {}",
                line
            )));
        }

        match parts[0] {
            "checkpoint_id" => {
                checkpoint_id = Some(parts[1].parse().map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "Invalid checkpoint_id in checkpoint metadata: {}",
                        e
                    ))
                })?);
            }
            "wal_lsn" => {
                lsn = Some(parts[1].parse().map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "Invalid wal_lsn in checkpoint metadata: {}",
                        e
                    ))
                })?);
            }
            "timestamp" => {
                timestamp = Some(parts[1].parse().map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "Invalid timestamp in checkpoint metadata: {}",
                        e
                    ))
                })?);
            }
            _ => {}
        }
    }

    let checkpoint_id = checkpoint_id.ok_or_else(|| {
        StorageError::deserialize_error("Missing checkpoint_id in checkpoint metadata".to_string())
    })?;
    let lsn = lsn.ok_or_else(|| {
        StorageError::deserialize_error("Missing wal_lsn in checkpoint metadata".to_string())
    })?;

    Ok(CheckpointInfo {
        checkpoint_id,
        lsn: Lsn::new(lsn),
        timestamp: timestamp.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;
    use crate::core::DataType;
    use crate::storage::engine::PersistenceConfig;
    use crate::storage::types::StoragePropertyDef;
    use crate::transaction::wal::writer::WalWriter;
    use crate::transaction::wal::{InsertVertexRedo, LocalWalWriter, WalOpType};
    use postcard::to_allocvec;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_insert_vertex_wal(
        wal_dir: &Path,
        timestamp: u32,
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
            vid: VertexId::from_int64(vid),
            properties: vec![("name".to_string(), name.as_bytes().to_vec())],
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

    fn write_checkpoint_metadata(
        checkpoint_dir: &Path,
        checkpoint_id: u64,
        wal_lsn: Lsn,
    ) -> StorageResult<()> {
        let checkpoint_path = checkpoint_dir.join(format!("checkpoint_{}", checkpoint_id));
        fs::create_dir_all(&checkpoint_path)?;

        let metadata_path = checkpoint_path.join("checkpoint.meta");
        let mut file = fs::File::create(metadata_path)?;
        writeln!(file, "checkpoint_id={}", checkpoint_id)?;
        writeln!(file, "wal_lsn={}", wal_lsn.as_u64())?;

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
            vid: VertexId::from_int64(1001),
            properties: vec![("name".to_string(), b"Alice".to_vec())],
        };
        let first_payload = to_allocvec(&first_redo).expect("Failed to serialize first redo");
        writer
            .append_entry(WalOpType::InsertVertex, 1, &first_payload)
            .expect("Failed to append first WAL entry");
        let checkpoint_lsn = writer.current_lsn();

        let second_redo = InsertVertexRedo {
            label: 1,
            vid: VertexId::from_int64(1002),
            properties: vec![("name".to_string(), b"Bob".to_vec())],
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
            vec![StoragePropertyDef::new(
                "name".to_string(),
                DataType::String,
            )],
            "name",
        )
        .expect("Failed to create vertex type");

        let _wal_lsn =
            write_insert_vertex_wal(&wal_dir, 1, 1, 1001, "Alice").expect("Failed to write WAL");
        write_checkpoint_metadata(&checkpoint_dir, 1, Lsn::ZERO)
            .expect("Failed to write checkpoint metadata");

        let stats = recover_from_wal(&ctx).expect("Recovery should succeed");
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
}

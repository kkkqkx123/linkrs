//! WAL recovery and checkpoint metadata parsing.

use std::path::{Path, PathBuf};

use crate::engine::persistence_coordinator::{
    CheckpointInfo, CHECKPOINT_FORMAT_VERSION, INCREMENTAL_CHECKPOINT_FORMAT_VERSION,
};
use linkrs_core::types::Timestamp;
use linkrs_core::{StorageError, StorageResult};
use linkrs_sync::checkpoint_manifest::CheckpointManifestManager;
use linkrs_transaction::wal::recovery::{RecoveryConfig, RecoveryManager, RecoveryStats};
use linkrs_transaction::wal::{Lsn, ParallelWalParser, WalRecoveryMode};

use super::checkpoint::create_checkpoint;
use super::GraphStorageContext;

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
    finish_recovery(ctx, config)
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
    finish_recovery(ctx, config)
}

/// Shared recovery tail: replay via the recovery manager, re-anchor WAL and
/// version timestamps, and re-establish the checkpoint baseline unless a
/// durable outbox still needs the WAL.
fn finish_recovery(
    ctx: &GraphStorageContext,
    config: RecoveryConfig,
) -> StorageResult<RecoveryStats> {
    let mut manager = RecoveryManager::new(config);

    let stats = manager.recover_with_applier(ctx)?;
    ctx.restore_auto_transaction_id(stats.max_transaction_id);

    // Replay deferred edge operations (two-phase recovery)
    ctx.replay_deferred_edges()?;

    // Advance the WAL writer's LSN to the last replayed position so that
    // create_checkpoint records the correct LSN instead of the fresh WAL's 0.
    // Use set_recovery_baseline_lsn to also update the file header's start_lsn,
    // preventing an LSN chain mismatch on subsequent recovery.
    if let Some(persistence) = ctx.persistence() {
        let coordinator = persistence.read();
        if let Some(wal_mgr) = coordinator.wal_manager() {
            let _ = wal_mgr.write().set_recovery_baseline_lsn(stats.last_lsn);
        }
    }

    // Advance write_ts past the max replayed timestamp so create_checkpoint
    // allocates a timestamp >= all recovered data, making recovered data
    // visible after reload.
    let current_ts = ctx.version_manager().write_timestamp();
    if stats.max_timestamp >= current_ts {
        ctx.version_manager().init_ts(stats.max_timestamp);
    }

    // When the durable outbox exists, storage recovery must leave the
    // remaining WAL available for the outbox projection recovery that runs
    // immediately after startup. A storage-only checkpoint here could
    // reclaim the very intents that SQLite still needs. The synchronized
    // checkpoint path creates the next combined baseline after projection
    // recovery completes.
    if !has_durable_outbox(ctx) {
        let _ = create_checkpoint(ctx)?;
    }

    // Update read_ts so recovered data is visible to subsequent reads.
    ctx.version_manager()
        .init_ts(ctx.version_manager().read_timestamp());

    // Replaying the WAL changed the physical layout; invalidate cached
    // plans that assumed the pre-recovery layout.
    ctx.bump_layout_version();

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
    let manifest_manager = CheckpointManifestManager::new(checkpoints_dir.join("manifests"));
    let Some(manifest) = manifest_manager
        .load_latest()
        .map_err(StorageError::db_error)?
    else {
        return Ok(None);
    };
    let checkpoint_path = manifest.storage_snapshot.path;
    let info = read_checkpoint_metadata(&checkpoint_path)?;
    if info.checkpoint_id != manifest.checkpoint_id {
        return Err(StorageError::deserialize_error(format!(
            "Checkpoint metadata id {} does not match manifest {}",
            info.checkpoint_id, manifest.checkpoint_id
        )));
    }
    Ok(Some(info))
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

fn has_durable_outbox(ctx: &GraphStorageContext) -> bool {
    let Some((_, data_dir, _)) = persistence_dirs(ctx) else {
        return false;
    };
    let work_dir = data_dir.parent().unwrap_or(&data_dir);
    work_dir.join("outbox/outbox.sqlite").exists() || work_dir.join("outbox_snapshots").is_dir()
}

pub(super) fn read_checkpoint_metadata(dir: &Path) -> StorageResult<CheckpointInfo> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let metadata_path = dir.join("checkpoint.meta");
    let file = File::open(metadata_path)?;
    let reader = BufReader::new(file);

    let mut checkpoint_id: Option<u64> = None;
    let mut lsn: Option<u64> = None;
    let mut timestamp: Option<Timestamp> = None;
    let mut format_version: Option<u32> = None;

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
            "format_version" => {
                format_version = Some(parts[1].parse().map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "Invalid checkpoint format version: {}",
                        e
                    ))
                })?);
            }
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
    // Accept both v1 (full) and v2 (incremental) checkpoints.
    // A missing format_version is rejected.
    let format_version = format_version.ok_or_else(|| {
        StorageError::deserialize_error("Missing format_version in checkpoint metadata".to_string())
    })?;
    if format_version != CHECKPOINT_FORMAT_VERSION
        && format_version != INCREMENTAL_CHECKPOINT_FORMAT_VERSION
    {
        return Err(StorageError::deserialize_error(format!(
            "Unsupported checkpoint format version: {}",
            format_version
        )));
    }

    Ok(CheckpointInfo {
        checkpoint_id,
        lsn: Lsn::new(lsn),
        timestamp: timestamp.unwrap_or(0),
    })
}

//! Startup bootstrap, save/flush and the version high-water sidecar.
//!
//! Checkpoint creation lives in [`checkpoint`]; WAL recovery and the
//! checkpoint.meta parser live in [`recovery`].

use std::path::Path;

use crate::engine::paths::StoragePaths;
use graphdb_core::{StorageError, StorageResult};
use graphdb_sync::checkpoint_manifest::CheckpointManifestManager;
use graphdb_transaction::wal::recovery::RecoveryStats;
use graphdb_transaction::wal::Lsn;

use super::context::GraphStorageContext;

mod checkpoint;
mod recovery;

#[cfg(test)]
mod tests;

pub(crate) use checkpoint::{
    auto_checkpoint_if_needed, auto_flush_if_needed, cleanup_snapshots, compact_transactional,
    create_checkpoint, create_checkpoint_with_guard, load_latest_checkpoint,
    persistence_diagnostics, should_checkpoint, should_flush, snapshot_stats, verify_snapshot,
};
pub(crate) use recovery::{needs_recovery, recover_from_wal, recover_from_wal_with_config};

fn load_schema_and_index_metadata(ctx: &GraphStorageContext) -> StorageResult<()> {
    if let Some(path) = ctx.work_dir().as_ref() {
        let paths = StoragePaths::new(path.clone());

        let latest_checkpoint = latest_published_checkpoint_dir(path)?;
        let schema_paths = latest_checkpoint
            .as_ref()
            .map(|checkpoint| StoragePaths::new(checkpoint).schema_file())
            .into_iter()
            .chain(std::iter::once(paths.schema_file()));
        for schema_path in schema_paths {
            if schema_path.exists() {
                ctx.schema_manager().load_schema(&schema_path)?;
                break;
            }
        }

        let index_paths = latest_checkpoint
            .as_ref()
            .map(|checkpoint| StoragePaths::new(checkpoint).index_meta_file())
            .into_iter()
            .chain(std::iter::once(paths.index_meta_file()));
        for index_path in index_paths {
            if index_path.exists() {
                ctx.index_metadata_manager().load_indexes(&index_path)?;
                break;
            }
        }

        ctx.load_migration_history()?;
    }

    Ok(())
}

fn latest_published_checkpoint_dir(work_dir: &Path) -> StorageResult<Option<std::path::PathBuf>> {
    let checkpoint_root = work_dir.join("checkpoint");
    if !checkpoint_root.exists() {
        return Ok(None);
    }
    let manifest_manager = CheckpointManifestManager::new(checkpoint_root.join("manifests"));
    manifest_manager
        .load_latest()
        .map_err(StorageError::db_error)
        .map(|manifest| manifest.map(|manifest| manifest.storage_snapshot.path))
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
        ctx.register_loaded_native_indexes()?;

        // The flush snapshot carries no checkpoint timestamp, so the fresh
        // version manager would restart at 1 and hide every row written
        // before the save. Re-anchor the frontier from the persisted
        // high-water mark; without it, post-reload reads and deletes lose
        // pre-save rows.
        let restored = read_version_high_water(&paths.data_dir()).unwrap_or(1);
        ctx.version_manager().init_ts(restored.max(1));
    }

    Ok(())
}

/// Name of the flush-sidecar file carrying the timestamp high-water mark.
const VERSION_META_FILE: &str = "version.meta";

fn write_version_high_water(data_dir: &Path, write_ts: u64) -> StorageResult<()> {
    let content = format!("write_ts={}\n", write_ts);
    std::fs::write(data_dir.join(VERSION_META_FILE), content)
        .map_err(|e| StorageError::io_error(format!("Failed to write version.meta: {e}")))?;
    Ok(())
}

fn read_version_high_water(data_dir: &Path) -> StorageResult<u64> {
    let content = std::fs::read_to_string(data_dir.join(VERSION_META_FILE))
        .map_err(|e| StorageError::io_error(format!("Failed to read version.meta: {e}")))?;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("write_ts=") {
            return value
                .trim()
                .parse::<u64>()
                .map_err(|e| StorageError::parse_error(format!("Invalid version.meta: {e}")));
        }
    }
    Err(StorageError::parse_error(
        "version.meta has no write_ts entry".to_string(),
    ))
}

pub(crate) fn bootstrap_from_disk(ctx: &GraphStorageContext) -> StorageResult<()> {
    load_schema_and_index_metadata(ctx)?;
    super::schema_writer::ensure_graph_types_from_schema(ctx)?;

    let checkpoint_info = load_latest_checkpoint(ctx)?;
    if let Some(ref info) = checkpoint_info {
        // A fully materialized checkpoint may have reclaimed every WAL record
        // from the active segment. Re-establish the logical WAL baseline so a
        // subsequent truncate or append does not treat the empty segment as
        // durable LSN zero.
        if let Some(persistence) = ctx.persistence() {
            let coordinator = persistence.read();
            if let Some(wal_manager) = coordinator.wal_manager() {
                wal_manager
                    .write()
                    .set_recovery_baseline_lsn(info.lsn)
                    .map_err(|error| {
                        StorageError::wal_error(format!(
                            "Failed to restore WAL checkpoint baseline: {}",
                            error
                        ))
                    })?;
            }
        }

        // Initialize the version manager with the checkpoint timestamp so that
        // persisted data (written at timestamps <= checkpoint timestamp) is visible
        // after reload. Without this, the fresh version manager's read_ts=1 would
        // not see data written at higher timestamps.
        ctx.version_manager().init_ts(info.timestamp);
    } else {
        restore_full_state_from_disk(ctx)?;
        // If data was restored from the main data directory (no checkpoints),
        // we can't recover the max timestamp. Use a default that ensures
        // data at ts=1 is visible (the minimum write timestamp).
        ctx.version_manager().init_ts(1);
    }

    Ok(())
}

pub(crate) fn initialize_with_recovery(
    ctx: &GraphStorageContext,
) -> StorageResult<Option<RecoveryStats>> {
    bootstrap_from_disk(ctx)?;

    if !needs_recovery(ctx) {
        super::serial::seed_serial_allocators(ctx)?;
        ctx.ensure_checkpoint_scheduler();
        return Ok(None);
    }

    log::info!("WAL recovery needed, starting recovery...");
    let stats = recover_from_wal(ctx)?;

    log::info!(
        "WAL recovery completed: {} entries replayed in {}ms",
        stats.wal_entries_replayed,
        stats.recovery_time_ms
    );

    // Seed SERIAL counters after replay so the column max includes replayed
    // rows (redo entries carry final property values; the counters themselves
    // are never replayed).
    super::serial::seed_serial_allocators(ctx)?;

    // Background checkpointing starts only after recovery has finished: the
    // recovery tail creates its own checkpoint, and a scheduler racing it
    // would either observe an already-active persistence state or collide on
    // the checkpoint directory rename.
    ctx.ensure_checkpoint_scheduler();

    Ok(Some(stats))
}

pub(crate) fn save_data(ctx: &GraphStorageContext) -> StorageResult<()> {
    let paths = ctx
        .storage_paths()
        .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))?;

    save_data_to_dir(ctx, paths.root())
}

pub(crate) fn save_data_to_dir(ctx: &GraphStorageContext, dir: &Path) -> StorageResult<()> {
    use std::fs::{self};

    let paths = StoragePaths::new(dir);
    let data_dir = paths.data_dir();
    fs::create_dir_all(&data_dir)?;

    ctx.flush_tables_to_dir(&data_dir)?;
    ctx.user_storage().save_to_dir(&data_dir)?;

    // Anchor the timestamp frontier for a future checkpoint-less restore:
    // without it the reloaded version manager restarts at 1 and every
    // pre-save row becomes invisible to new reads and deletes.
    write_version_high_water(&data_dir, ctx.version_manager().write_timestamp())?;

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

pub(crate) fn load_from_disk(ctx: &GraphStorageContext) -> StorageResult<()> {
    load_schema_and_index_metadata(ctx)?;
    super::schema_writer::ensure_graph_types_from_schema(ctx)?;
    restore_full_state_from_disk(ctx)?;
    super::serial::seed_serial_allocators(ctx)
}

pub(crate) fn save_to_disk(ctx: &GraphStorageContext) -> StorageResult<()> {
    if let Some(path) = ctx.work_dir().as_ref() {
        let paths = StoragePaths::new(path.clone());
        std::fs::create_dir_all(paths.root()).map_err(|e| StorageError::io_error(e.to_string()))?;

        let schema_dir = paths.schema_dir();
        std::fs::create_dir_all(&schema_dir).map_err(|e| StorageError::io_error(e.to_string()))?;
        let schema_path = paths.schema_file();
        ctx.schema_manager()
            .set_serial_next(super::serial::serial_next_snapshot(ctx));
        ctx.schema_manager().save_schema(&schema_path)?;

        let index_meta_dir = paths.index_meta_dir();
        std::fs::create_dir_all(&index_meta_dir)
            .map_err(|e| StorageError::io_error(e.to_string()))?;
        let index_meta_path = paths.index_meta_file();
        ctx.index_metadata_manager()
            .save_indexes(&index_meta_path)?;

        // Persist migration history.
        ctx.save_migration_history()?;

        save_data_to_dir(ctx, paths.root())?;

        let index_path = paths.indexes_dir();
        std::fs::create_dir_all(&index_path).map_err(|e| StorageError::io_error(e.to_string()))?;
        ctx.index_data_manager().read().flush(&index_path)?;
    }
    Ok(())
}

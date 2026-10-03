//! Checkpoint creation, snapshot diagnostics and auto-flush thresholds.

use std::path::Path;

use crate::engine::paths::StoragePaths;
use crate::engine::persistence_coordinator::{CheckpointData, CheckpointInfo, CheckpointStats};
use graphdb_core::types::{CompactConfig, CompactTarget};
use graphdb_core::{StorageError, StorageResult};

use super::GraphStorageContext;

pub(crate) fn create_checkpoint(
    ctx: &GraphStorageContext,
) -> StorageResult<Option<CheckpointStats>> {
    create_checkpoint_with_reason(ctx, graphdb_metrics::CheckpointTriggerReason::Explicit)
}

pub(crate) fn create_checkpoint_with_reason(
    ctx: &GraphStorageContext,
    reason: graphdb_metrics::CheckpointTriggerReason,
) -> StorageResult<Option<CheckpointStats>> {
    let persistence = match ctx.persistence().as_ref() {
        Some(p) => p,
        None => return Ok(None),
    };

    let ts = ctx.get_write_timestamp()?;
    let graph = ctx.clone();
    if let Some(mgr) = ctx.stats_manager().cloned() {
        let age = persistence.read().last_checkpoint_time.read().elapsed();
        mgr.record_checkpoint_trigger(reason, age);
    }

    let result = persistence.read().create_checkpoint_with_reason(
        |checkpoint_dir, _timestamp| write_checkpoint_payload(&graph, checkpoint_dir),
        ts,
        reason,
    );

    let stats = match result {
        Ok(stats) => {
            ctx.commit_write_timestamp_ordered(ts)?;
            if let Some(mgr) = ctx.stats_manager().cloned() {
                mgr.record_checkpoint_success(
                    stats.duration,
                    stats.bytes_flushed,
                    stats.wal_files_truncated as u64,
                );
            }
            stats
        }
        Err(error) => {
            ctx.abort_write_timestamp(ts);
            if let Some(mgr) = ctx.stats_manager().cloned() {
                mgr.record_checkpoint_failure();
            }
            return Err(error);
        }
    };

    Ok(Some(stats))
}

pub(crate) fn create_checkpoint_with_guard(
    ctx: &GraphStorageContext,
    guard: crate::engine::persistence_coordinator::PersistenceStateGuard,
    reason: graphdb_metrics::CheckpointTriggerReason,
) -> StorageResult<Option<CheckpointStats>> {
    let persistence = match ctx.persistence().as_ref() {
        Some(p) => p.clone(),
        None => return Ok(None),
    };
    let ts = ctx.get_write_timestamp()?;
    let graph = ctx.clone();
    let result = persistence.read().create_checkpoint_with_guard(
        guard,
        |checkpoint_dir, _timestamp| write_checkpoint_payload(&graph, checkpoint_dir),
        ts,
        reason,
    );
    match result {
        Ok(stats) => {
            ctx.commit_write_timestamp_ordered(ts)?;
            Ok(Some(stats))
        }
        Err(error) => {
            ctx.abort_write_timestamp(ts);
            Err(error)
        }
    }
}

/// Shared payload collector for the checkpoint variants; persists schema,
/// index metadata, migration history and table data into the checkpoint dir.
fn write_checkpoint_payload(
    graph: &GraphStorageContext,
    checkpoint_dir: &Path,
) -> StorageResult<CheckpointData> {
    let checkpoint_paths = StoragePaths::new(checkpoint_dir);
    std::fs::create_dir_all(checkpoint_paths.schema_dir())?;
    graph
        .schema_manager()
        .set_serial_next(super::super::serial::serial_next_snapshot(graph));
    graph
        .schema_manager()
        .save_schema(&checkpoint_paths.schema_file())?;
    std::fs::create_dir_all(checkpoint_paths.index_meta_dir())?;
    graph
        .index_metadata_manager()
        .save_indexes(&checkpoint_paths.index_meta_file())?;
    // Persist migration history alongside schema.
    let migration_file = checkpoint_paths.migration_history_file();
    if let Some(parent) = migration_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    graph
        .migration_history()
        .read()
        .save_to_file(&migration_file)?;

    let data_dir = StoragePaths::new(checkpoint_dir).data_dir();
    std::fs::create_dir_all(&data_dir)?;

    graph.flush_tables_to_checkpoint(&data_dir)?;
    graph.user_storage().save_to_dir(&data_dir)?;

    let vertex_count = graph.total_vertex_count() as u64;
    let edge_count = graph.total_edge_count() as u64;

    let data_size = std::fs::metadata(&data_dir).map(|m| m.len()).unwrap_or(0);

    Ok(CheckpointData {
        vertex_count,
        edge_count,
        data_size,
    })
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

pub(crate) fn snapshot_stats(ctx: &GraphStorageContext) -> crate::SnapshotStats {
    ctx.persistence()
        .as_ref()
        .map(|persistence| persistence.read().snapshot_stats())
        .unwrap_or_default()
}

pub(crate) fn persistence_diagnostics(
    ctx: &GraphStorageContext,
) -> Option<crate::PersistenceDiagnostics> {
    // Ensure checkpoint scheduler state is observable via diagnostics;
    // this wires CheckpointScheduler::pending/is_running and
    // PersistenceCoordinator::checkpoint_diagnostics into the main path.
    let _checkpoint_diag = ctx.checkpoint_diagnostics();
    let _scheduler_running = ctx.is_checkpoint_scheduler_running();
    ctx.persistence().as_ref().map(|persistence| {
        let mut diagnostics = persistence.read().diagnostics();
        let catalog = ctx.data_store().lock_metrics();
        diagnostics.catalog_lock_acquisitions = catalog.acquisitions;
        diagnostics.catalog_lock_wait_nanos = catalog.wait_nanos;
        diagnostics.catalog_lock_hold_nanos = catalog.hold_nanos;
        diagnostics.catalog_lock_contentions = catalog.contended;
        diagnostics.catalog_lock_by_operation =
            crate::engine::data_store::CatalogLockOperation::all()
                .into_iter()
                .enumerate()
                .map(|(index, operation)| {
                    let metric = catalog.by_operation[index];
                    crate::engine::persistence_coordinator::CatalogLockDiagnostic {
                        operation: operation.name().to_string(),
                        acquisitions: metric.acquisitions,
                        wait_nanos: metric.wait_nanos,
                        hold_nanos: metric.hold_nanos,
                        contentions: metric.contended,
                    }
                })
                .collect();
        let table_locks = ctx.data_store().table_lock_metrics();
        diagnostics.table_lock_read_acquisitions = table_locks.read_acquisitions;
        diagnostics.table_lock_read_wait_nanos = table_locks.read_wait_nanos;
        diagnostics.table_lock_read_contentions = table_locks.read_contended;
        diagnostics.table_lock_write_acquisitions = table_locks.write_acquisitions;
        diagnostics.table_lock_write_wait_nanos = table_locks.write_wait_nanos;
        diagnostics.table_lock_write_contentions = table_locks.write_contended;
        diagnostics
    })
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
        super::flush(ctx)?;
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn auto_checkpoint_if_needed(
    ctx: &GraphStorageContext,
) -> StorageResult<Option<CheckpointStats>> {
    if !should_checkpoint(ctx) {
        return Ok(None);
    }
    let persistence = match ctx.persistence().as_ref() {
        Some(p) => p.clone(),
        None => return Ok(None),
    };
    let config = persistence.read().config.clone();
    if config.async_checkpoint_enabled {
        let wal_bytes = {
            let coord = persistence.read();
            let lsn = *coord.last_checkpoint_lsn.read();
            coord.wal_bytes_since(lsn)
        };
        if wal_bytes >= config.max_wal_size {
            // Hard limit: fallback to synchronous checkpoint to prevent WAL explosion
            let stats = create_checkpoint(ctx)?;
            return Ok(stats);
        }
        let reason = persistence
            .read()
            .checkpoint_trigger_reason()
            .unwrap_or(graphdb_metrics::CheckpointTriggerReason::Explicit);
        ctx.request_async_checkpoint(reason);
        return Ok(None);
    }
    let stats = create_checkpoint(ctx)?;
    Ok(stats)
}

pub(crate) fn compact_transactional(
    ctx: &GraphStorageContext,
    config: &CompactConfig,
) -> StorageResult<()> {
    let persistence = ctx.persistence().as_ref().ok_or_else(|| {
        StorageError::db_error("Persistence not available for transactional compaction".to_string())
    })?;

    let version_manager = ctx.version_manager().as_ref();

    let timestamp = version_manager.acquire_insert_timestamp().map_err(|e| {
        StorageError::db_error(format!("Failed to acquire compaction timestamp: {}", e))
    })?;

    // Transactional compaction is global across spaces, so observers see the
    // global-compaction sentinel space id.
    persistence
        .read()
        .notify_compaction_started(crate::engine::storage_events::GLOBAL_COMPACTION_SPACE_ID);

    let before_stats = ctx.get_compact_stats();
    let result = {
        log::info!(
            "Starting transactional compaction: enable_structure_compaction={}, config={{ segment_merge_enabled: {} }}, size={}/{}",
            config.enable_structure_compaction,
            config.segment_merge_enabled,
            before_stats.used_size,
            before_stats.total_size
        );

        {
            let coordinator = persistence.read();
            let wal_mgr = coordinator.wal_manager();
            let wal_guard = wal_mgr
                .as_ref()
                .ok_or_else(|| StorageError::db_error("WAL not enabled".to_string()))?
                .read();

            wal_guard
                .append_entry(graphdb_core::wal::types::WalOpType::Compact, timestamp, &[])
                .map_err(|e| {
                    StorageError::wal_error(format!("Failed to append compact WAL: {}", e))
                })?;
        }

        ctx.compact(config, timestamp)
            .map_err(|e| StorageError::db_error(format!("Compaction failed: {}", e)))
    };

    match result {
        Ok(()) => {
            version_manager
                .commit_ordered(timestamp)
                .map_err(|error| StorageError::db_error(error.to_string()))?;

            let after_stats = ctx.get_compact_stats();
            let reclaimed_bytes = before_stats
                .total_size
                .saturating_sub(after_stats.total_size) as u64;
            log::info!(
                "Compaction completed: size={}/{} (freed {} bytes)",
                after_stats.used_size,
                after_stats.total_size,
                ctx.get_compact_stats()
                    .total_size
                    .saturating_sub(after_stats.used_size)
            );
            persistence.read().notify_compaction_completed(
                crate::engine::storage_events::GLOBAL_COMPACTION_SPACE_ID,
                reclaimed_bytes,
            );

            Ok(())
        }
        Err(e) => {
            version_manager.abort_write_timestamp(timestamp);
            Err(e)
        }
    }
}

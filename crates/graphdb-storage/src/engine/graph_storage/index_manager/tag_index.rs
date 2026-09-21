use crate::index::key_codec::{KeyBuilder, KeyParser};
use crate::index::manifest::{GenerationBuildState, IndexManifest, IndexShard};
use crate::index::VertexIndexOps;
use graphdb_core::metadata::index_manager::IndexMetadataManager;
use graphdb_core::types::{CommitLsn, Index, IndexStatus, SnapshotTimestamp};
use graphdb_core::{StorageError, StorageResult, Value};

use super::super::context::GraphStorageContext;
use super::checkpoint::{
    build_vertex_index_data, generation_output_paths, remove_generation_build_state,
    resolve_crash_recovery, save_generation_build_state, write_generation_checkpoint,
};
use super::generation::{
    current_wal_lsn, fail_if_generation_fault_is_injected, next_generation, GenerationFaultPoint,
};
use super::wal_replay::{replay_wal_partition, wal_intents_for_index};

pub(crate) fn create_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index: &Index,
) -> StorageResult<bool> {
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?
        .space_id;
    let mut index = index.clone();
    index.space_id = space_id;
    let created = ctx
        .index_metadata_manager()
        .create_tag_index(space_id, &index)?;
    if created {
        // Retrieve the stored index to get the assigned ID.
        let stored = ctx
            .index_metadata_manager()
            .get_tag_index(space_id, &index.name)?
            .unwrap_or(index);
        ctx.index_data_manager()
            .read()
            .register_native_index(space_id, &stored)?;
    }
    Ok(created)
}

pub(crate) fn drop_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index_id = ctx
        .index_data_manager()
        .read()
        .index_alias(space_id, index_name);
    // Clear runtime state before removing metadata: a failed clear must leave
    // the metadata intact so the drop can be retried without orphaning state.
    {
        let manager = ctx.index_data_manager().write();
        manager.clear_tag_index(space_id, index_name)?;
        manager.unregister_native_index(space_id, index_name);
        if let Some(index_id) = index_id {
            manager.remove_checkpoint_dirs_by_id(space_id, index_id);
        } else {
            manager.remove_index_checkpoint_dirs(space_id, index_name);
        }
    }
    ctx.index_metadata_manager()
        .drop_tag_index(space_id, index_name)
}

pub(crate) fn get_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<Option<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager()
        .get_tag_index(space_id, index_name)
}

pub(crate) fn list_tag_indexes(
    ctx: &GraphStorageContext,
    space: &str,
) -> StorageResult<Vec<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager().list_tag_indexes(space_id)
}

pub(crate) fn rebuild_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    vertices: &[graphdb_core::Vertex],
    snapshot_timestamp: SnapshotTimestamp,
    start_lsn: CommitLsn,
) -> StorageResult<bool> {
    if let Some(stats) = ctx.stats_manager() {
        stats.record_generation_build();
    }
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_tag_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Index {} not found", index_name)))?;

    ctx.index_data_manager()
        .read()
        .register_native_index(space_id, &index)?;

    // Resolve any incomplete generation build from a previous crash.
    resolve_crash_recovery(ctx, space_id, index_name)?;

    // ── Phase: Building ────────────────────────────────────────────────────
    // Record the WAL LSN at snapshot time so the catch-up phase knows where
    // to begin replaying committed writes.
    let generation = next_generation(ctx, space_id, index_name)?;

    let mut build_state = GenerationBuildState::new(generation, snapshot_timestamp, start_lsn);
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::Building,
    )?;

    // Build new index data from the snapshot without touching the active generation.
    let snapshot_ts = snapshot_timestamp.get();
    let (forward, reverse) = build_vertex_index_data(space_id, &index, vertices, snapshot_ts)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::SnapshotBuild)?;

    // ── Phase: CatchingUp ──────────────────────────────────────────────────
    build_state.transition_to_catching_up()?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::CatchingUp,
    )?;

    // Snapshot the active generation under a read lock and release it
    // before the WAL scan and merge below. The WAL is the source of truth
    // for which changes belong to this catch-up; active records only
    // provide their MVCC payload. Holding a write lock across the scan
    // would serialize all concurrent writers for the whole rebuild.
    let (active_forward, active_reverse) = ctx
        .index_data_manager()
        .read()
        .active_index_data(space_id, index.id)?;
    let active_had_data = !active_forward.is_empty() || !active_reverse.is_empty();
    let observed_barrier_lsn = current_wal_lsn(ctx);
    let barrier_lsn = if observed_barrier_lsn < start_lsn {
        start_lsn
    } else {
        observed_barrier_lsn
    };
    let intents = wal_intents_for_index(ctx, space_id, &index, start_lsn, barrier_lsn)?;
    let forward_prefix = KeyBuilder::build_vertex_index_prefix(space_id, index_name).0;
    let (merged_forward, merged_reverse) = replay_wal_partition(
        (active_forward, active_reverse),
        (forward, reverse),
        snapshot_ts,
        &intents,
        |key| key.starts_with(&forward_prefix),
        |key| {
            KeyParser::parse_vertex_reverse_key_v2(key)
                .is_ok_and(|(_, parsed_index_name)| parsed_index_name == index_name)
        },
    );
    fail_if_generation_fault_is_injected(GenerationFaultPoint::IncrementalReplay)?;
    // Refuse to publish an empty generation over live data: an empty
    // snapshot with no catch-up intents means the source yielded nothing,
    // and publishing it would silently truncate the index. A genuinely
    // empty index (no active data either) still rebuilds as a no-op.
    if merged_forward.is_empty() && merged_reverse.is_empty() && active_had_data {
        return Err(StorageError::invalid_operation(format!(
            "Rebuild of index {} would publish an empty generation over live data; \
             the snapshot source yielded no entries and no WAL intents were replayed. \
             Drop and recreate the index for intentional truncation",
            index_name
        )));
    }
    build_state.transition_to_publishing(barrier_lsn)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::BarrierEstablished)?;
    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::Publishing,
    )?;

    let (gen_dir, manifest_path) = generation_output_paths(ctx, space_id, index.id, generation);
    let persisted_forward = merged_forward
        .iter()
        .filter(|(key, _)| key.starts_with(&forward_prefix))
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    let persisted_reverse = merged_reverse
        .iter()
        .filter(|(key, _)| {
            KeyParser::parse_vertex_reverse_key_v2(key)
                .is_ok_and(|(_, parsed_index_name)| parsed_index_name == index_name)
        })
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    if manifest_path.is_some() {
        std::fs::create_dir_all(&gen_dir)?;
        write_generation_checkpoint(&gen_dir, &persisted_forward, &persisted_reverse)?;
    }
    fail_if_generation_fault_is_injected(GenerationFaultPoint::GenerationFsync)?;

    let manifest = IndexManifest::new(
        space_id,
        index.id,
        generation,
        vec![IndexShard {
            shard_id: 0,
            lower: None,
            upper: None,
            checkpoint_file: gen_dir.clone(),
            checksum: None,
        }],
    )?;

    fail_if_generation_fault_is_injected(GenerationFaultPoint::ManifestRename)?;
    if let Some(manifest_path) = manifest_path {
        manifest.store(&manifest_path)?;
    }
    // Publish under the write lock with a generation check: another rebuild
    // may have published while this one worked lock-free, in which case the
    // caller retries against the newer generation.
    {
        let manager = ctx.index_data_manager().write();
        let current = manager
            .manifest_catalog(space_id, index.id)
            .ok_or_else(|| StorageError::not_found(format!("Index {} has no manifest", index.id)))?
            .acquire()
            .manifest()
            .clone();
        if current.generation >= generation {
            return Err(StorageError::invalid_operation(
                "Index generation changed while rebuilding; retry the rebuild",
            ));
        }
        manager.publish_native_index(
            manifest.clone(),
            persisted_forward,
            persisted_reverse,
            barrier_lsn,
        )?;
    }
    log::info!(
        "Published new generation {} for index {} (space {})",
        generation,
        index_name,
        space_id
    );
    fail_if_generation_fault_is_injected(GenerationFaultPoint::FenceRelease)?;

    // ── Phase: Active ──────────────────────────────────────────────────────
    // Mark the index as active and remove the build state so it is not
    // re-processed on the next startup.
    build_state.transition_to_active()?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager()
        .set_tag_index_status(space_id, index_name, IndexStatus::Active)?;

    remove_generation_build_state(ctx, space_id, index_name)?;

    // The publish counter is recorded inside `publish_native_index`;
    // recording it here as well would double count each rebuild.
    log::info!(
        "Generation rebuild for index {} (gen {}) completed successfully",
        index_name,
        generation
    );

    Ok(true)
}

pub(crate) fn lookup_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    value: &Value,
) -> StorageResult<Vec<Value>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;

    let index = ctx
        .index_metadata_manager()
        .get_tag_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Index {} not found", index_name)))?;

    let results = ctx
        .index_data_manager()
        .read()
        .lookup_tag_index(space_id, &index, value)?;
    Ok(results)
}

use crate::core::metadata::index_manager::IndexMetadataManager;
use crate::core::types::{
    CommitLsn, Index, IndexGeneration, IndexStatus, ManifestEpoch, SnapshotTimestamp,
};
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::index_data_manager::IndexRecord;
use crate::storage::index::key_codec::{EdgeIndexKeyGen, KeyBuilder, KeyParser, VertexIndexKeyGen};
use crate::storage::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard,
};
use crate::storage::index::{EdgeIndexOps, VertexIndexOps};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::LazyLock;

use super::context::GraphStorageContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GenerationFaultPoint {
    SnapshotBuild,
    IncrementalReplay,
    BarrierEstablished,
    GenerationFsync,
    ManifestRename,
    FenceRelease,
}

static GENERATION_FAULTS: LazyLock<parking_lot::RwLock<HashSet<GenerationFaultPoint>>> =
    LazyLock::new(|| parking_lot::RwLock::new(HashSet::new()));

fn fail_if_generation_fault_is_injected(point: GenerationFaultPoint) -> StorageResult<()> {
    if GENERATION_FAULTS.read().contains(&point) {
        return Err(StorageError::db_error(format!(
            "Injected generation rebuild failure at {point:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
fn inject_generation_fault(point: GenerationFaultPoint) {
    GENERATION_FAULTS.write().insert(point);
}

#[cfg(test)]
fn clear_generation_faults() {
    GENERATION_FAULTS.write().clear();
}

pub(crate) fn current_wal_lsn(ctx: &GraphStorageContext) -> CommitLsn {
    if let Some(persistence) = ctx.persistence() {
        let coordinator = persistence.read();
        if let Some(wal) = coordinator.wal_manager() {
            let lsn = wal.read().current_lsn();
            return CommitLsn::new(lsn.into());
        }
    }
    CommitLsn::ZERO
}

fn save_generation_build_state(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
    state: &GenerationBuildState,
) -> StorageResult<()> {
    let serialized =
        serde_json::to_vec(state).map_err(|e| StorageError::serialize_error(e.to_string()))?;
    let dir = build_state_dir(ctx, space_id)?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{index_name}_generation_build.json"));
    let temporary = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&serialized)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &path)?;
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn load_generation_build_state(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<Option<GenerationBuildState>> {
    let dir = build_state_dir(ctx, space_id)?;
    let path = dir.join(format!("{index_name}_generation_build.json"));
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let state: GenerationBuildState = serde_json::from_slice(&bytes)
        .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
    Ok(Some(state))
}

fn remove_generation_build_state(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<()> {
    let dir = build_state_dir(ctx, space_id)?;
    let path = dir.join(format!("{index_name}_generation_build.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

fn build_state_dir(ctx: &GraphStorageContext, space_id: u64) -> StorageResult<std::path::PathBuf> {
    ctx.work_dir()
        .as_ref()
        .map(|dir| {
            dir.join("generation_build_state")
                .join(space_id.to_string())
        })
        .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))
}

fn resolve_crash_recovery(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<()> {
    let Some(build_state) = load_generation_build_state(ctx, space_id, index_name)? else {
        return Ok(());
    };
    // If the build was incomplete (Building or CatchingUp), discard it.
    // The caller will rebuild from scratch.
    if !build_state.is_active() && !matches!(build_state.state, GenerationState::Publishing) {
        log::warn!(
            "Discarding incomplete generation build for index {index_name} (state={:?})",
            build_state.state
        );
        remove_generation_build_state(ctx, space_id, index_name)?;
        return Ok(());
    }
    // If the build was in Publishing state, the manifest may have been published
    // but the metadata update was lost. The published manifest is the authority.
    if matches!(build_state.state, GenerationState::Publishing) {
        log::info!("Completing generation build for index {index_name} from Publishing state");
    }
    Ok(())
}

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
    if index.space_id != space_id {
        return Err(StorageError::invalid_operation(format!(
            "Index {} belongs to space {}, not space {}",
            index.name, index.space_id, space_id
        )));
    }
    let created = ctx
        .index_metadata_manager()
        .create_tag_index(space_id, index)?;
    if created {
        ctx.index_data_manager()
            .read()
            .register_native_index(space_id, index)?;
    }
    Ok(created)
}

pub(crate) fn drop_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let dropped = ctx
        .index_metadata_manager()
        .drop_tag_index(space_id, index_name)?;
    if dropped {
        let manager = ctx.index_data_manager().write();
        manager.clear_tag_index(space_id, index_name)?;
        manager.unregister_native_index(space_id, index_name);
    }
    Ok(dropped)
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

/// Build a physical index key by appending a unique version suffix.
fn make_physical_key(logical_key: &[u8], version: u64) -> Vec<u8> {
    let mut physical_key = Vec::with_capacity(logical_key.len() + 8);
    physical_key.extend_from_slice(logical_key);
    physical_key.extend_from_slice(&version.to_le_bytes());
    physical_key
}

fn record_changed_after(record: &IndexRecord, snapshot_timestamp: u32) -> bool {
    record.created_ts > snapshot_timestamp
        || record
            .deleted_ts
            .is_some_and(|deleted_ts| deleted_ts > snapshot_timestamp)
}

fn replay_partition_changes(
    target: &mut BTreeMap<Vec<u8>, IndexRecord>,
    changes: Vec<(Vec<u8>, IndexRecord)>,
    snapshot_timestamp: u32,
) {
    for (key, record) in changes {
        let logical_len = key.len().saturating_sub(std::mem::size_of::<u64>());
        let logical_key = &key[..logical_len];
        if record.created_ts > snapshot_timestamp {
            target.insert(key.clone(), record.clone());
        }
        if let Some(deleted_ts) = record
            .deleted_ts
            .filter(|deleted_ts| *deleted_ts > snapshot_timestamp)
        {
            for (candidate, entry) in target.iter_mut() {
                let candidate_len = candidate.len().saturating_sub(std::mem::size_of::<u64>());
                if candidate[..candidate_len] == *logical_key {
                    entry.mark_deleted(deleted_ts);
                }
            }
        }
    }
}

fn merge_rebuilt_partition<F, R>(
    mut active_forward: BTreeMap<Vec<u8>, IndexRecord>,
    mut active_reverse: BTreeMap<Vec<u8>, IndexRecord>,
    rebuilt_forward: BTreeMap<Vec<u8>, IndexRecord>,
    rebuilt_reverse: BTreeMap<Vec<u8>, IndexRecord>,
    snapshot_timestamp: u32,
    matches_forward: F,
    matches_reverse: R,
) -> (
    BTreeMap<Vec<u8>, IndexRecord>,
    BTreeMap<Vec<u8>, IndexRecord>,
)
where
    F: Fn(&[u8]) -> bool,
    R: Fn(&[u8]) -> bool,
{
    let forward_changes = active_forward
        .iter()
        .filter(|(key, record)| {
            matches_forward(key) && record_changed_after(record, snapshot_timestamp)
        })
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    let reverse_changes = active_reverse
        .iter()
        .filter(|(key, record)| {
            matches_reverse(key) && record_changed_after(record, snapshot_timestamp)
        })
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();

    active_forward.retain(|key, _| !matches_forward(key));
    active_reverse.retain(|key, _| !matches_reverse(key));
    active_forward.extend(rebuilt_forward);
    active_reverse.extend(rebuilt_reverse);
    replay_partition_changes(&mut active_forward, forward_changes, snapshot_timestamp);
    replay_partition_changes(&mut active_reverse, reverse_changes, snapshot_timestamp);
    (active_forward, active_reverse)
}

/// Build new index data from a snapshot of vertices.
fn build_vertex_index_data(
    space_id: u64,
    index: &Index,
    vertices: &[crate::core::Vertex],
    snapshot_timestamp: u32,
) -> StorageResult<(
    BTreeMap<Vec<u8>, IndexRecord>,
    BTreeMap<Vec<u8>, IndexRecord>,
)> {
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    let mut version_counter = 1u64;

    for vertex in vertices {
        let indexed_values: Vec<Value> = index
            .fields
            .iter()
            .filter_map(|field| vertex.properties.get(&field.name).cloned())
            .collect();
        let included_columns = index
            .properties
            .iter()
            .filter_map(|name| {
                vertex
                    .properties
                    .get(name)
                    .cloned()
                    .map(|value| (name.clone(), value))
            })
            .collect::<Vec<_>>();
        let vid_value = Value::from(vertex.vid);

        for prop_value in &indexed_values {
            let logical_forward_key =
                KeyBuilder::build_vertex_index_key(space_id, &index.name, prop_value, &vid_value)?;
            let logical_reverse_key =
                KeyBuilder::build_vertex_reverse_key_v2(space_id, &vid_value, &index.name)?;

            let entry = IndexRecord::new(snapshot_timestamp)
                .with_entity_version(snapshot_timestamp)
                .with_entity_ref(EntityRef::Vertex(vertex.vid));
            let mut entry = entry;
            entry.included_columns = included_columns.clone();
            let fwd_key = make_physical_key(&logical_forward_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);
            let rev_key = make_physical_key(&logical_reverse_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);

            forward.insert(fwd_key, entry.clone());
            reverse.insert(rev_key, entry);
        }
    }
    Ok((forward, reverse))
}

/// Get a fresh generation number derived from the active manifest's generation + 1.
fn next_generation_and_epoch(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<(IndexGeneration, ManifestEpoch)> {
    let index_id = {
        let mgr = ctx.index_data_manager().read();
        let alias = mgr.index_alias(space_id, index_name);
        alias
    };
    let Some(index_id) = index_id else {
        // No manifest catalog yet; start at generation 1.
        return Ok((IndexGeneration::new(1), ManifestEpoch::new(1)));
    };
    let catalog = ctx
        .index_data_manager()
        .read()
        .manifest_catalog(space_id, index_id);
    let Some(catalog) = catalog else {
        return Ok((IndexGeneration::new(1), ManifestEpoch::new(1)));
    };
    let stats = catalog.stats();
    Ok((
        IndexGeneration::new(stats.active_generation.get().saturating_add(1)),
        ManifestEpoch::new(stats.active_epoch.get().saturating_add(1)),
    ))
}

pub(crate) fn rebuild_tag_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    vertices: &[crate::core::Vertex],
    snapshot_timestamp: SnapshotTimestamp,
    start_lsn: CommitLsn,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_tag_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Index {} not found", index_name)))?;

    // Resolve any incomplete generation build from a previous crash.
    resolve_crash_recovery(ctx, space_id, index_name)?;

    // ── Phase: Building ────────────────────────────────────────────────────
    // Record the WAL LSN at snapshot time so the catch-up phase knows where
    // to begin replaying committed writes.
    let (generation, manifest_epoch) = next_generation_and_epoch(ctx, space_id, index_name)?;

    let mut build_state =
        GenerationBuildState::new(generation, manifest_epoch, snapshot_timestamp, start_lsn);
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::Building,
    )?;

    // Build new index data from the snapshot without touching the active generation.
    let snapshot_ts = u32::try_from(snapshot_timestamp.get()).map_err(|_| {
        StorageError::invalid_operation("Snapshot timestamp exceeds the MVCC timestamp range")
    })?;
    let (forward, reverse) = build_vertex_index_data(space_id, &index, vertices, snapshot_ts)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::SnapshotBuild)?;

    // ── Phase: CatchingUp ──────────────────────────────────────────────────
    build_state
        .transition_to_catching_up()
        .map_err(StorageError::invalid_operation)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::CatchingUp,
    )?;

    // Establish the publish fence by excluding native-index writers. The active
    // MVCC history is the native change log for writes after snapshot_ts.
    let manager = ctx.index_data_manager().write();
    let (active_forward, active_reverse) = manager.active_index_data(
        space_id,
        u64::try_from(index.id)
            .map_err(|_| StorageError::invalid_operation("Index ID cannot be negative"))?,
    )?;
    let forward_prefix = KeyBuilder::build_vertex_index_prefix(space_id, index_name).0;
    let (merged_forward, merged_reverse) = merge_rebuilt_partition(
        active_forward,
        active_reverse,
        forward,
        reverse,
        snapshot_ts,
        |key| key.starts_with(&forward_prefix),
        |key| {
            KeyParser::parse_vertex_reverse_key_v2(key)
                .is_ok_and(|(_, parsed_index_name)| parsed_index_name == index_name)
        },
    );
    fail_if_generation_fault_is_injected(GenerationFaultPoint::IncrementalReplay)?;
    let barrier_lsn = current_wal_lsn(ctx);
    build_state
        .transition_to_publishing(barrier_lsn)
        .map_err(StorageError::invalid_operation)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::BarrierEstablished)?;
    ctx.index_metadata_manager().set_tag_index_status(
        space_id,
        index_name,
        IndexStatus::Publishing,
    )?;

    let index_dir = ctx
        .storage_paths()
        .map(|paths| paths.indexes_dir())
        .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))?;
    std::fs::create_dir_all(&index_dir)?;
    let index_id = u64::try_from(index.id)
        .map_err(|_| StorageError::invalid_operation("Index ID cannot be negative".to_string()))?;
    let index_root = index_dir
        .join(space_id.to_string())
        .join(index_id.to_string());
    let gen_dir = index_root.join(format!("generation-{}", generation.get()));
    std::fs::create_dir_all(&gen_dir)?;
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
    GenericIndexManager::<VertexIndexKeyGen>::flush_data(
        &gen_dir,
        &persisted_forward,
        &persisted_reverse,
    )?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::GenerationFsync)?;

    let manifest = IndexManifest::new(
        space_id,
        index_id,
        generation,
        manifest_epoch,
        vec![IndexShard {
            shard_id: 0,
            lower: None,
            upper: None,
            checkpoint_file: gen_dir.clone(),
        }],
    )
    .map_err(StorageError::db_error)?;

    fail_if_generation_fault_is_injected(GenerationFaultPoint::ManifestRename)?;
    manifest.store(&index_root.join("manifest.json"))?;
    manager.publish_generation_data(manifest.clone(), persisted_forward, persisted_reverse)?;
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
    build_state
        .transition_to_active()
        .map_err(StorageError::invalid_operation)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager()
        .set_tag_index_status(space_id, index_name, IndexStatus::Active)?;

    remove_generation_build_state(ctx, space_id, index_name)?;

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

pub(crate) fn create_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index: &Index,
) -> StorageResult<bool> {
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?
        .space_id;
    if index.space_id != space_id {
        return Err(StorageError::invalid_operation(format!(
            "Index {} belongs to space {}, not space {}",
            index.name, index.space_id, space_id
        )));
    }
    let created = ctx
        .index_metadata_manager()
        .create_edge_index(space_id, index)?;
    if created {
        ctx.index_data_manager()
            .read()
            .register_native_index(space_id, index)?;
    }
    Ok(created)
}

pub(crate) fn drop_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let dropped = ctx
        .index_metadata_manager()
        .drop_edge_index(space_id, index_name)?;
    if dropped {
        let manager = ctx.index_data_manager().write();
        manager.clear_edge_index(space_id, index_name)?;
        manager.unregister_native_index(space_id, index_name);
    }
    Ok(dropped)
}

/// Build new index data from a snapshot of edges.
fn build_edge_index_data(
    space_id: u64,
    index: &Index,
    edges: &[crate::core::Edge],
    snapshot_timestamp: u32,
) -> StorageResult<(
    BTreeMap<Vec<u8>, IndexRecord>,
    BTreeMap<Vec<u8>, IndexRecord>,
)> {
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    let mut version_counter = 1u64;

    for edge in edges {
        let indexed_values: Vec<Value> = index
            .fields
            .iter()
            .filter_map(|field| edge.props.get(&field.name).cloned())
            .collect();
        let included_columns = index
            .properties
            .iter()
            .filter_map(|name| {
                edge.props
                    .get(name)
                    .cloned()
                    .map(|value| (name.clone(), value))
            })
            .collect::<Vec<_>>();
        let src_value = Value::from(edge.src);
        let dst_value = Value::from(edge.dst);

        for prop_value in &indexed_values {
            let logical_forward_key = KeyBuilder::build_edge_index_key(
                space_id,
                &index.name,
                prop_value,
                &src_value,
                &dst_value,
                &edge.edge_type,
                edge.ranking,
            )?;
            let logical_reverse_key = KeyBuilder::build_edge_reverse_key(
                space_id,
                &src_value,
                &dst_value,
                &edge.edge_type,
                edge.ranking,
                &index.name,
            )?;

            let mut entry =
                IndexRecord::new(snapshot_timestamp).with_entity_version(snapshot_timestamp);
            entry.included_columns = included_columns.clone();
            let fwd_key = make_physical_key(&logical_forward_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);
            let rev_key = make_physical_key(&logical_reverse_key.0, version_counter);
            version_counter = version_counter.wrapping_add(1);

            forward.insert(fwd_key, entry.clone());
            reverse.insert(rev_key, entry);
        }
    }
    Ok((forward, reverse))
}

pub(crate) fn rebuild_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    edges: &[crate::core::Edge],
    snapshot_timestamp: SnapshotTimestamp,
    start_lsn: CommitLsn,
) -> StorageResult<bool> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_edge_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Edge index {} not found", index_name)))?;

    // Resolve any incomplete generation build from a previous crash.
    resolve_crash_recovery(ctx, space_id, index_name)?;

    // ── Phase: Building ────────────────────────────────────────────────────
    let (generation, manifest_epoch) = next_generation_and_epoch(ctx, space_id, index_name)?;

    let mut build_state =
        GenerationBuildState::new(generation, manifest_epoch, snapshot_timestamp, start_lsn);
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Building,
    )?;

    let snapshot_ts = u32::try_from(snapshot_timestamp.get()).map_err(|_| {
        StorageError::invalid_operation("Snapshot timestamp exceeds the MVCC timestamp range")
    })?;
    let (forward, reverse) = build_edge_index_data(space_id, &index, edges, snapshot_ts)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::SnapshotBuild)?;

    // ── Phase: CatchingUp ──────────────────────────────────────────────────
    build_state
        .transition_to_catching_up()
        .map_err(StorageError::invalid_operation)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::CatchingUp,
    )?;

    let manager = ctx.index_data_manager().write();
    let (active_forward, active_reverse) = manager.active_index_data(
        space_id,
        u64::try_from(index.id)
            .map_err(|_| StorageError::invalid_operation("Index ID cannot be negative"))?,
    )?;
    let forward_prefix = KeyBuilder::build_edge_index_prefix(space_id, index_name).0;
    let (merged_forward, merged_reverse) = merge_rebuilt_partition(
        active_forward,
        active_reverse,
        forward,
        reverse,
        snapshot_ts,
        |key| key.starts_with(&forward_prefix),
        |key| {
            KeyParser::parse_edge_reverse_key(key)
                .is_ok_and(|(_, _, _, _, parsed_index_name)| parsed_index_name == index_name)
        },
    );
    fail_if_generation_fault_is_injected(GenerationFaultPoint::IncrementalReplay)?;
    let barrier_lsn = current_wal_lsn(ctx);
    build_state
        .transition_to_publishing(barrier_lsn)
        .map_err(StorageError::invalid_operation)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::BarrierEstablished)?;
    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Publishing,
    )?;

    let index_dir = ctx
        .storage_paths()
        .map(|paths| paths.indexes_dir())
        .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))?;
    std::fs::create_dir_all(&index_dir)?;
    let index_id = u64::try_from(index.id)
        .map_err(|_| StorageError::invalid_operation("Index ID cannot be negative".to_string()))?;
    let index_root = index_dir
        .join(space_id.to_string())
        .join(index_id.to_string());
    let gen_dir = index_root.join(format!("generation-{}", generation.get()));
    std::fs::create_dir_all(&gen_dir)?;
    let persisted_forward = merged_forward
        .iter()
        .filter(|(key, _)| key.starts_with(&forward_prefix))
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    let persisted_reverse = merged_reverse
        .iter()
        .filter(|(key, _)| {
            KeyParser::parse_edge_reverse_key(key)
                .is_ok_and(|(_, _, _, _, parsed_index_name)| parsed_index_name == index_name)
        })
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    GenericIndexManager::<EdgeIndexKeyGen>::flush_data(
        &gen_dir,
        &persisted_forward,
        &persisted_reverse,
    )?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::GenerationFsync)?;

    let manifest = IndexManifest::new(
        space_id,
        index_id,
        generation,
        manifest_epoch,
        vec![IndexShard {
            shard_id: 0,
            lower: None,
            upper: None,
            checkpoint_file: gen_dir.clone(),
        }],
    )
    .map_err(StorageError::db_error)?;

    fail_if_generation_fault_is_injected(GenerationFaultPoint::ManifestRename)?;
    manifest.store(&index_root.join("manifest.json"))?;
    manager.publish_generation_data(manifest.clone(), persisted_forward, persisted_reverse)?;
    log::info!(
        "Published new generation {} for edge index {} (space {})",
        generation,
        index_name,
        space_id
    );
    fail_if_generation_fault_is_injected(GenerationFaultPoint::FenceRelease)?;

    // ── Phase: Active ──────────────────────────────────────────────────────
    build_state
        .transition_to_active()
        .map_err(StorageError::invalid_operation)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Active,
    )?;

    remove_generation_build_state(ctx, space_id, index_name)?;

    log::info!(
        "Generation rebuild for edge index {} (gen {}) completed successfully",
        index_name,
        generation
    );

    Ok(true)
}

pub(crate) fn get_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
) -> StorageResult<Option<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager()
        .get_edge_index(space_id, index_name)
}

pub(crate) fn list_edge_indexes(
    ctx: &GraphStorageContext,
    space: &str,
) -> StorageResult<Vec<Index>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    ctx.index_metadata_manager().list_edge_indexes(space_id)
}

pub(crate) fn lookup_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    value: &Value,
) -> StorageResult<Vec<(Value, Value, String, i64)>> {
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_edge_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Edge index {} not found", index_name)))?;
    let results = ctx
        .index_data_manager()
        .read()
        .lookup_edge_index(space_id, &index, value)?;
    Ok(results)
}

#[cfg(test)]
mod tests {
    use crate::core::types::{
        CommitLsn, Index, IndexConfig, IndexField, IndexGeneration, IndexType, ManifestEpoch,
        SnapshotTimestamp,
    };
    use crate::core::StorageError;
    use crate::core::Value;
    use crate::storage::engine::graph_storage::context::GraphStorageContext;
    use crate::storage::index::manifest::{GenerationBuildState, GenerationState};

    fn setup_context() -> GraphStorageContext {
        GraphStorageContext::new()
    }

    fn generation_state(generation: u64, start_lsn: u64) -> GenerationBuildState {
        GenerationBuildState::new(
            IndexGeneration::new(generation),
            ManifestEpoch::new(generation),
            SnapshotTimestamp::new(start_lsn),
            CommitLsn::new(start_lsn),
        )
    }

    fn setup_persistent_context() -> (tempfile::TempDir, GraphStorageContext) {
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let config = crate::storage::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let ctx = GraphStorageContext::new_with_persistence(temp_dir.path().to_path_buf(), config)
            .expect("Failed to create persistent context");
        (temp_dir, ctx)
    }

    #[test]
    fn test_create_and_list_tag_index() {
        let ctx = setup_context();

        let index = Index::new(IndexConfig {
            id: 1,
            name: "person_name_idx".to_string(),
            space_id: 0,
            schema_name: "Person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::String(String::new()),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        });

        super::create_tag_index(&ctx, "test_space", &index)
            .expect_err("should fail because space does not exist");

        // Actually we need a space + schema adapter for full testing.
        // The index_manager functions require a schema_manager with registered space.
        // This is tested through integration tests (tests.rs).
    }

    #[test]
    fn test_get_tag_index_on_empty() {
        let ctx = setup_context();
        let result = super::get_tag_index(&ctx, "nonexistent", "some_index");
        assert!(result.is_err());
    }

    #[test]
    fn test_drop_tag_index_on_empty() {
        let ctx = setup_context();
        let result = super::drop_tag_index(&ctx, "nonexistent", "some_index");
        assert!(result.is_err());
    }

    #[test]
    fn test_lookup_index_on_nonexistent_space() {
        let ctx = setup_context();
        let result = super::lookup_index(
            &ctx,
            "no_space",
            "some_index",
            &Value::String("test".to_string()),
        );
        assert!(result.is_err());
    }

    // ── Generation build state persistence tests ───────────────────────────

    #[test]
    fn test_generation_build_state_save_load_remove_roundtrip() {
        let (_temp, ctx) = setup_persistent_context();
        let space_id = 1u64;
        let index_name = "test_idx";

        let state = generation_state(42, 100);

        // Save
        super::save_generation_build_state(&ctx, space_id, index_name, &state)
            .expect("build state should save");
        assert_eq!(state.state, GenerationState::Building);
        assert_eq!(state.generation, IndexGeneration::new(42));
        assert_eq!(state.start_lsn, CommitLsn::new(100));
        assert!(state.barrier_lsn.is_none());

        // Load
        let loaded = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("build state should load")
            .expect("build state should exist");
        assert_eq!(loaded.generation, state.generation);
        assert_eq!(loaded.start_lsn, state.start_lsn);
        assert_eq!(loaded.state, state.state);
        assert!(loaded.barrier_lsn.is_none());

        // Remove
        super::remove_generation_build_state(&ctx, space_id, index_name)
            .expect("build state should remove");
        let after_remove = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("load should succeed");
        assert!(after_remove.is_none());
    }

    #[test]
    fn test_generation_build_state_transitions_are_persistent() {
        let (_temp, ctx) = setup_persistent_context();
        let space_id = 1u64;
        let index_name = "transition_idx";

        let mut state = generation_state(1, 10);
        super::save_generation_build_state(&ctx, space_id, index_name, &state)
            .expect("Building state should save");

        // Transition to CatchingUp
        state
            .transition_to_catching_up()
            .expect("Building should transition to CatchingUp");
        assert_eq!(state.state, GenerationState::CatchingUp);
        super::save_generation_build_state(&ctx, space_id, index_name, &state)
            .expect("CatchingUp state should save");

        let loaded = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("load should succeed")
            .expect("build state should exist");
        assert_eq!(loaded.state, GenerationState::CatchingUp);

        // Transition to Publishing
        state
            .transition_to_publishing(CommitLsn::new(50))
            .expect("CatchingUp should transition to Publishing");
        assert_eq!(state.state, GenerationState::Publishing);
        assert_eq!(state.barrier_lsn, Some(CommitLsn::new(50)));
        super::save_generation_build_state(&ctx, space_id, index_name, &state)
            .expect("Publishing state should save");

        let loaded = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("load should succeed")
            .expect("build state should exist");
        assert_eq!(loaded.state, GenerationState::Publishing);
        assert_eq!(loaded.barrier_lsn, Some(CommitLsn::new(50)));

        // Transition to Active
        state
            .transition_to_active()
            .expect("Publishing should transition to Active");
        assert!(state.is_active());
    }

    #[test]
    fn test_crash_recovery_discards_incomplete_building_state() {
        let (_temp, ctx) = setup_persistent_context();
        let space_id = 1u64;
        let index_name = "building_crash_idx";

        // Simulate crash during Building: save Building state then recover
        let state = generation_state(1, 10);
        super::save_generation_build_state(&ctx, space_id, index_name, &state)
            .expect("Building state should save");

        // Recover (should discard incomplete build)
        super::resolve_crash_recovery(&ctx, space_id, index_name).expect("recovery should succeed");

        let after = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("load should succeed");
        assert!(
            after.is_none(),
            "Building state should be discarded on crash recovery"
        );
    }

    #[test]
    fn test_crash_recovery_discards_incomplete_catching_up_state() {
        let (_temp, ctx) = setup_persistent_context();
        let space_id = 1u64;
        let index_name = "catchup_crash_idx";

        // Simulate crash during CatchingUp
        let mut state = generation_state(1, 10);
        state
            .transition_to_catching_up()
            .expect("Building should transition to CatchingUp");
        super::save_generation_build_state(&ctx, space_id, index_name, &state)
            .expect("CatchingUp state should save");

        // Recover (should discard incomplete catch-up)
        super::resolve_crash_recovery(&ctx, space_id, index_name).expect("recovery should succeed");

        let after = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("load should succeed");
        assert!(
            after.is_none(),
            "CatchingUp state should be discarded on crash recovery"
        );
    }

    #[test]
    fn test_crash_recovery_preserves_publishing_state() {
        let (_temp, ctx) = setup_persistent_context();
        let space_id = 1u64;
        let index_name = "publishing_crash_idx";

        // Simulate crash during Publishing
        let mut state = generation_state(1, 10);
        state
            .transition_to_catching_up()
            .expect("Building should transition to CatchingUp");
        state
            .transition_to_publishing(CommitLsn::new(50))
            .expect("CatchingUp should transition to Publishing");
        super::save_generation_build_state(&ctx, space_id, index_name, &state)
            .expect("Publishing state should save");

        // Recover (should NOT discard Publishing state since the manifest may be published)
        super::resolve_crash_recovery(&ctx, space_id, index_name).expect("recovery should succeed");

        let after = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("load should succeed");
        assert!(
            after.is_some(),
            "Publishing state should be preserved on crash recovery"
        );
        assert_eq!(after.unwrap().state, GenerationState::Publishing);
    }

    #[test]
    fn test_generation_build_state_not_found_for_missing_index() {
        let (_temp, ctx) = setup_persistent_context();
        let space_id = 1u64;
        let index_name = "nonexistent_idx";

        let result = super::load_generation_build_state(&ctx, space_id, index_name)
            .expect("load should succeed");
        assert!(result.is_none());
    }

    #[test]
    fn test_remove_generation_build_state_on_clean_state_is_noop() {
        let (_temp, ctx) = setup_persistent_context();
        let result = super::remove_generation_build_state(&ctx, 1u64, "no_state_idx");
        assert!(result.is_ok());
    }

    #[test]
    fn test_generation_build_state_can_resume_semantics() {
        let building = generation_state(1, 10);
        assert!(!building.can_resume());
        assert!(!building.is_active());

        let mut catching_up = generation_state(1, 10);
        catching_up
            .transition_to_catching_up()
            .expect("Building should transition to CatchingUp");
        assert!(catching_up.can_resume());
        assert!(!catching_up.is_active());

        let mut active = generation_state(1, 10);
        active
            .transition_to_catching_up()
            .expect("Building should transition to CatchingUp");
        active
            .transition_to_publishing(CommitLsn::new(10))
            .expect("CatchingUp should transition to Publishing");
        active
            .transition_to_active()
            .expect("Publishing should transition to Active");
        assert!(!active.can_resume());
        assert!(active.is_active());
    }

    #[test]
    fn test_build_vertex_index_data_handles_empty_input() {
        let index = Index::new(IndexConfig {
            id: 1,
            name: "person_name_idx".to_string(),
            space_id: 0,
            schema_name: "Person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::String(String::new()),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        });

        let vertices = vec![];
        let (forward, reverse) =
            super::build_vertex_index_data(0, &index, &vertices, crate::core::types::MAX_TIMESTAMP)
                .expect("build should succeed with empty input");
        assert!(forward.is_empty(), "forward map should be empty");
        assert!(reverse.is_empty(), "reverse map should be empty");
    }

    #[test]
    fn test_flush_index_data_writes_valid_files() {
        use crate::core::types::MAX_TIMESTAMP;
        use crate::storage::index::generic_index_manager::GenericIndexManager;
        use crate::storage::index::index_data_manager::IndexRecord;
        use crate::storage::index::key_codec::VertexIndexKeyGen;
        use std::collections::BTreeMap;
        use std::fs;

        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let gen_dir = temp_dir.path().join("gen_1");
        fs::create_dir_all(&gen_dir).expect("dir should be created");

        let mut forward = BTreeMap::new();
        forward.insert(vec![0, 1, 2], IndexRecord::new(MAX_TIMESTAMP));
        let mut reverse = BTreeMap::new();
        reverse.insert(vec![3, 4, 5], IndexRecord::new(MAX_TIMESTAMP));

        GenericIndexManager::<VertexIndexKeyGen>::flush_data(&gen_dir, &forward, &reverse)
            .expect("flush should succeed");

        assert!(
            gen_dir.join("forward_index.bin").exists(),
            "forward file should exist"
        );
        assert!(
            gen_dir.join("reverse_index.bin").exists(),
            "reverse file should exist"
        );
    }
}

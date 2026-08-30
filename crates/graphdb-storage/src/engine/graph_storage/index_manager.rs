use crate::index::key_codec::{KeyBuilder, KeyParser};
use crate::index::manifest::{GenerationBuildState, IndexManifest, IndexShard};
use crate::index::types::IndexRecord;
use crate::index::{EdgeIndexOps, VertexIndexOps};
use graphdb_core::metadata::index_manager::IndexMetadataManager;
use graphdb_core::types::{CommitLsn, Index, IndexGeneration, IndexStatus, SnapshotTimestamp};
use graphdb_core::{StorageError, StorageResult, Value};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::LazyLock;

use super::context::GraphStorageContext;

pub(crate) type IndexDataMaps = (
    BTreeMap<Vec<u8>, IndexRecord>,
    BTreeMap<Vec<u8>, IndexRecord>,
);

pub mod checkpoint;
pub mod wal_replay;

pub(crate) use checkpoint::{
    build_edge_index_data, build_vertex_index_data, generation_output_paths,
    load_generation_build_state, remove_generation_build_state, resolve_crash_recovery,
    save_generation_build_state, write_generation_checkpoint,
};
pub(crate) use wal_replay::{replay_wal_partition, wal_intents_for_index};

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

pub(crate) fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Index IDs are persisted in SQLite INTEGER columns, so keep the
    // deterministic hash within the signed 64-bit range.
    hash & (i64::MAX as u64)
}

/// Get a fresh generation number derived from the active manifest's generation + 1.
fn next_generation(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<IndexGeneration> {
    let index_id = {
        let mgr = ctx.index_data_manager().read();

        mgr.index_alias(space_id, index_name)
    };
    let Some(index_id) = index_id else {
        // No manifest catalog yet; start at generation 1.
        return Ok(IndexGeneration::new(1));
    };
    let catalog = ctx
        .index_data_manager()
        .read()
        .manifest_catalog(space_id, index_id);
    let Some(catalog) = catalog else {
        return Ok(IndexGeneration::new(1));
    };
    let stats = catalog.stats();
    Ok(IndexGeneration::new(
        stats.active_generation.get().saturating_add(1),
    ))
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

    // Capture the active generation and WAL barrier while the index manager is
    // exclusively borrowed. The WAL is the source of truth for which changes
    // belong to this catch-up; active records only provide their MVCC payload.
    let manager = ctx.index_data_manager().write();
    let (active_forward, active_reverse) = manager.active_index_data(space_id, index.id)?;
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
    manager.publish_native_index(
        manifest.clone(),
        persisted_forward,
        persisted_reverse,
        barrier_lsn,
    )?;
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

    if let Some(stats) = ctx.stats_manager() {
        stats.record_generation_publish();
    }

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
        // Retrieve the stored index to get the assigned ID.
        let stored = ctx
            .index_metadata_manager()
            .get_edge_index(space_id, &index.name)?
            .unwrap_or_else(|| index.clone());
        ctx.index_data_manager()
            .read()
            .register_native_index(space_id, &stored)?;
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

pub(crate) fn rebuild_edge_index(
    ctx: &GraphStorageContext,
    space: &str,
    index_name: &str,
    edges: &[graphdb_core::Edge],
    snapshot_timestamp: SnapshotTimestamp,
    start_lsn: CommitLsn,
) -> StorageResult<bool> {
    if let Some(stats) = ctx.stats_manager() {
        stats.record_generation_build();
    }
    let space_id = ctx.schema_manager().get_space_id(space)?;
    let index = ctx
        .index_metadata_manager()
        .get_edge_index(space_id, index_name)?
        .ok_or_else(|| StorageError::not_found(format!("Edge index {} not found", index_name)))?;

    ctx.index_data_manager()
        .read()
        .register_native_index(space_id, &index)?;

    // Resolve any incomplete generation build from a previous crash.
    resolve_crash_recovery(ctx, space_id, index_name)?;

    // ── Phase: Building ────────────────────────────────────────────────────
    let generation = next_generation(ctx, space_id, index_name)?;

    let mut build_state = GenerationBuildState::new(generation, snapshot_timestamp, start_lsn);
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Building,
    )?;

    let snapshot_ts = snapshot_timestamp.get();
    let (forward, reverse) = build_edge_index_data(space_id, &index, edges, snapshot_ts)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::SnapshotBuild)?;

    // ── Phase: CatchingUp ──────────────────────────────────────────────────
    build_state.transition_to_catching_up()?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::CatchingUp,
    )?;

    let manager = ctx.index_data_manager().write();
    let (active_forward, active_reverse) = manager.active_index_data(space_id, index.id)?;
    let observed_barrier_lsn = current_wal_lsn(ctx);
    let barrier_lsn = if observed_barrier_lsn < start_lsn {
        start_lsn
    } else {
        observed_barrier_lsn
    };
    let intents = wal_intents_for_index(ctx, space_id, &index, start_lsn, barrier_lsn)?;
    let forward_prefix = KeyBuilder::build_edge_index_prefix(space_id, index_name).0;
    let (merged_forward, merged_reverse) = replay_wal_partition(
        (active_forward, active_reverse),
        (forward, reverse),
        snapshot_ts,
        &intents,
        |key| key.starts_with(&forward_prefix),
        |key| {
            KeyParser::parse_edge_reverse_key(key)
                .is_ok_and(|(_, _, _, _, parsed_index_name)| parsed_index_name == index_name)
        },
    );
    fail_if_generation_fault_is_injected(GenerationFaultPoint::IncrementalReplay)?;
    build_state.transition_to_publishing(barrier_lsn)?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;
    fail_if_generation_fault_is_injected(GenerationFaultPoint::BarrierEstablished)?;
    ctx.index_metadata_manager().set_edge_index_status(
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
            KeyParser::parse_edge_reverse_key(key)
                .is_ok_and(|(_, _, _, _, parsed_index_name)| parsed_index_name == index_name)
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
    manager.publish_native_index(
        manifest.clone(),
        persisted_forward,
        persisted_reverse,
        barrier_lsn,
    )?;
    log::info!(
        "Published new generation {} for edge index {} (space {})",
        generation,
        index_name,
        space_id
    );
    fail_if_generation_fault_is_injected(GenerationFaultPoint::FenceRelease)?;

    // ── Phase: Active ──────────────────────────────────────────────────────
    build_state.transition_to_active()?;
    save_generation_build_state(ctx, space_id, index_name, &build_state)?;

    ctx.index_metadata_manager().set_edge_index_status(
        space_id,
        index_name,
        IndexStatus::Active,
    )?;

    remove_generation_build_state(ctx, space_id, index_name)?;

    if let Some(stats) = ctx.stats_manager() {
        stats.record_generation_publish();
    }

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

#[cfg(test)]
mod tests {
    use crate::engine::graph_storage::context::GraphStorageContext;
    use crate::index::manifest::{GenerationBuildState, GenerationState};
    use crate::index::types::IndexRecord;
    use crate::{
        GraphStorage, StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
    };
    use graphdb_core::types::{
        CommitLsn, IdempotencyKey, Index, IndexConfig, IndexField, IndexGeneration, IndexType,
        OrderingKey, SnapshotTimestamp, TargetId, TransactionId, VertexId,
    };
    use graphdb_core::wal::{EntityRef, IndexMutation, IndexOperation, OutboxIntent};
    use graphdb_core::Value;

    fn setup_context() -> GraphStorageContext {
        GraphStorageContext::new()
    }

    fn generation_state(generation: u64, start_lsn: u64) -> GenerationBuildState {
        GenerationBuildState::new(
            IndexGeneration::new(generation),
            SnapshotTimestamp::new(start_lsn),
            CommitLsn::new(start_lsn),
        )
    }

    fn setup_persistent_context() -> (tempfile::TempDir, GraphStorageContext) {
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let ctx = GraphStorageContext::new_with_persistence(temp_dir.path().to_path_buf(), config)
            .expect("Failed to create persistent context");
        (temp_dir, ctx)
    }

    fn test_intent(transaction_id: TransactionId, index_id: u64, vertex_id: i64) -> OutboxIntent {
        OutboxIntent {
            wire_version: graphdb_core::wal::WAL_SYNC_WIRE_VERSION,
            transaction_id,
            intent_sequence: 0,
            mutation: IndexMutation {
                wire_version: graphdb_core::wal::WAL_SYNC_WIRE_VERSION,
                target: TargetId::new("native-index").expect("target should be valid"),
                index_id,
                index_generation: IndexGeneration::new(1),
                entity_ref: EntityRef::Vertex(VertexId::from_int64(vertex_id)),
                operation: IndexOperation::Upsert,
                document_or_vector: Vec::new(),
                idempotency_key: IdempotencyKey::new(format!(
                    "intent-{transaction_id}-{vertex_id}"
                ))
                .expect("idempotency key should be valid"),
                ordering_key: OrderingKey::new(format!("vertex-{vertex_id}"))
                    .expect("ordering key should be valid"),
            },
        }
    }

    #[test]
    fn wal_catch_up_uses_only_intent_entities() {
        let entity = EntityRef::Vertex(VertexId::from_int64(1));
        let other_entity = EntityRef::Vertex(VertexId::from_int64(2));
        let intents = vec![test_intent(TransactionId::new(1), 7, 1)];

        let mut active_forward = std::collections::BTreeMap::new();
        active_forward.insert(
            b"logged-forward".to_vec(),
            IndexRecord::new(20).with_entity_ref(entity.clone()),
        );
        active_forward.insert(
            b"memory-only-forward".to_vec(),
            IndexRecord::new(20).with_entity_ref(other_entity.clone()),
        );
        let mut active_reverse = std::collections::BTreeMap::new();
        active_reverse.insert(
            b"logged-reverse".to_vec(),
            IndexRecord::new(20).with_entity_ref(entity),
        );
        active_reverse.insert(
            b"memory-only-reverse".to_vec(),
            IndexRecord::new(20).with_entity_ref(other_entity),
        );

        let mut rebuilt_forward = std::collections::BTreeMap::new();
        rebuilt_forward.insert(b"snapshot-forward".to_vec(), IndexRecord::new(10));
        let mut rebuilt_reverse = std::collections::BTreeMap::new();
        rebuilt_reverse.insert(b"snapshot-reverse".to_vec(), IndexRecord::new(10));

        let (forward, reverse) = super::replay_wal_partition(
            (active_forward, active_reverse),
            (rebuilt_forward, rebuilt_reverse),
            10,
            &intents,
            |_| true,
            |_| true,
        );

        assert!(forward.contains_key(b"snapshot-forward".as_slice()));
        assert!(forward.contains_key(b"logged-forward".as_slice()));
        assert!(!forward.contains_key(b"memory-only-forward".as_slice()));
        assert!(reverse.contains_key(b"snapshot-reverse".as_slice()));
        assert!(reverse.contains_key(b"logged-reverse".as_slice()));
        assert!(!reverse.contains_key(b"memory-only-reverse".as_slice()));
    }

    #[test]
    fn wal_catch_up_reads_committed_intents_from_disk() {
        let (_temp_dir, ctx) = setup_persistent_context();
        let transaction_id = TransactionId::new(9);
        let intent = test_intent(transaction_id, 7, 42);

        ctx.commit_staged_writes(transaction_id, &[intent])
            .expect("transaction should be durable");
        let barrier_lsn = super::current_wal_lsn(&ctx);
        let index = Index::new(IndexConfig {
            id: 7,
            name: "test_index".to_string(),
            space_id: 1,
            schema_name: "Person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::string(""),
                false,
            )],
            properties: Vec::new(),
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        let filtered = super::wal_intents_for_index(&ctx, 1, &index, CommitLsn::ZERO, barrier_lsn)
            .expect("committed intents should be readable");

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].transaction_id, transaction_id);
        assert_eq!(filtered[0].mutation.index_id, 7);
    }

    #[test]
    fn concurrent_rebuild_and_writes_preserve_new_index_entries() {
        use std::collections::HashMap;
        use std::thread;

        let temp_dir = tempfile::TempDir::new().expect("temporary directory should be created");
        let mut storage = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
            .expect("persistent storage should be created");
        let mut space = graphdb_core::types::SpaceInfo::new("test_space".to_string())
            .with_vid_type(graphdb_core::DataType::BigInt);
        storage
            .create_space(&mut space)
            .expect("space should be created");
        let tag = graphdb_core::types::TagInfo::new("Person".to_string()).with_properties(vec![
            graphdb_core::types::PropertyDef::new(
                "name".to_string(),
                graphdb_core::DataType::String,
            ),
        ]);
        storage
            .create_tag("test_space", &tag)
            .expect("tag should be created");
        let index = Index::new(IndexConfig {
            id: 1,
            name: "person_name_idx".to_string(),
            space_id: 1,
            schema_name: "Person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::string(""),
                false,
            )],
            properties: Vec::new(),
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        storage
            .create_tag_index("test_space", &index)
            .expect("index should be created");

        for vertex_id in 0..500 {
            let mut properties = HashMap::new();
            properties.insert(
                "name".to_string(),
                Value::string(format!("initial-{vertex_id}")),
            );
            storage
                .insert_vertex(
                    "test_space",
                    graphdb_core::Vertex::new(
                        VertexId::from_int64(vertex_id),
                        vec![graphdb_core::vertex_edge_path::Tag::new(
                            "Person".to_string(),
                            properties,
                        )],
                    ),
                )
                .expect("initial vertex should be inserted");
        }

        let rebuild_storage = storage.clone();
        let writer_storage = storage.clone();
        let rebuild = thread::spawn(move || {
            let mut rebuild_storage = rebuild_storage;
            rebuild_storage
                .rebuild_tag_index("test_space", "person_name_idx")
                .expect("rebuild should succeed");
        });
        let writer = thread::spawn(move || {
            let mut writer_storage = writer_storage;
            for vertex_id in 10_000..10_050 {
                let mut properties = HashMap::new();
                properties.insert(
                    "name".to_string(),
                    Value::string(format!("concurrent-{vertex_id}")),
                );
                writer_storage
                    .insert_vertex(
                        "test_space",
                        graphdb_core::Vertex::new(
                            VertexId::from_int64(vertex_id),
                            vec![graphdb_core::vertex_edge_path::Tag::new(
                                "Person".to_string(),
                                properties,
                            )],
                        ),
                    )
                    .expect("concurrent vertex should be inserted");
            }
        });

        rebuild.join().expect("rebuild thread should finish");
        writer.join().expect("writer thread should finish");

        for vertex_id in 10_000..10_050 {
            let indexed = storage
                .lookup_index(
                    "test_space",
                    "person_name_idx",
                    &Value::string(format!("concurrent-{vertex_id}")),
                )
                .expect("index lookup should succeed");
            assert_eq!(indexed, vec![Value::from(VertexId::from_int64(vertex_id))]);
        }
    }

    #[test]
    fn rebuild_restarts_after_incremental_replay_failure() {
        let temp_dir = tempfile::TempDir::new().expect("temporary directory should be created");
        let index = Index::new(IndexConfig {
            id: 1,
            name: "person_name_idx".to_string(),
            space_id: 1,
            schema_name: "Person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::string(""),
                false,
            )],
            properties: Vec::new(),
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });

        {
            let mut storage = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
                .expect("persistent storage should be created");
            let mut space = graphdb_core::types::SpaceInfo::new("test_space".to_string())
                .with_vid_type(graphdb_core::DataType::BigInt);
            storage
                .create_space(&mut space)
                .expect("space should be created");
            let tag =
                graphdb_core::types::TagInfo::new("Person".to_string()).with_properties(vec![
                    graphdb_core::types::PropertyDef::new(
                        "name".to_string(),
                        graphdb_core::DataType::String,
                    ),
                ]);
            storage
                .create_tag("test_space", &tag)
                .expect("tag should be created");
            storage
                .create_tag_index("test_space", &index)
                .expect("index should be created");
            let vertex = graphdb_core::Vertex::new(
                VertexId::from_int64(1),
                vec![graphdb_core::vertex_edge_path::Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string("Alice"))]
                        .into_iter()
                        .collect(),
                )],
            );
            storage
                .insert_vertex("test_space", vertex)
                .expect("vertex should be inserted");
            storage
                .rebuild_tag_index("test_space", "person_name_idx")
                .expect("initial generation should build");
            storage.flush().expect("initial index state should flush");
            storage
                .create_checkpoint()
                .expect("initial checkpoint should succeed");

            super::inject_generation_fault(super::GenerationFaultPoint::IncrementalReplay);
            let result = storage.rebuild_tag_index("test_space", "person_name_idx");
            super::clear_generation_faults();
            assert!(
                result.is_err(),
                "injected catch-up failure should be returned"
            );
            storage
                .flush()
                .expect("storage should flush before restart");
        }

        let mut storage = GraphStorage::open(temp_dir.path().to_path_buf())
            .expect("storage should reopen after the failed build");
        assert!(storage
            .rebuild_tag_index("test_space", "person_name_idx")
            .expect("rebuild should restart after crash recovery"));
        let indexed = storage
            .lookup_index("test_space", "person_name_idx", &Value::string("Alice"))
            .expect("rebuilt index should be readable");
        assert_eq!(indexed, vec![Value::from(VertexId::from_int64(1))]);
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
                Value::string(""),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
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
        let result = super::lookup_index(&ctx, "no_space", "some_index", &Value::string("test"));
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
    fn test_generation_build_state_transitions() {
        let building = generation_state(1, 10);
        assert!(!building.is_active());

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
                Value::string(""),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });

        let vertices = vec![];
        let (forward, reverse) = super::build_vertex_index_data(
            0,
            &index,
            &vertices,
            graphdb_core::types::MAX_TIMESTAMP,
        )
        .expect("build should succeed with empty input");
        assert!(forward.is_empty(), "forward map should be empty");
        assert!(reverse.is_empty(), "reverse map should be empty");
    }

    #[test]
    fn test_flush_index_data_writes_valid_files() {
        use crate::index::types::IndexRecord;
        use graphdb_core::types::MAX_TIMESTAMP;
        use std::collections::BTreeMap;
        use std::fs;

        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let gen_dir = temp_dir.path().join("gen_1");
        fs::create_dir_all(&gen_dir).expect("dir should be created");

        let mut forward = BTreeMap::new();
        forward.insert(vec![0, 1, 2], IndexRecord::new(MAX_TIMESTAMP));
        let mut reverse = BTreeMap::new();
        reverse.insert(vec![3, 4, 5], IndexRecord::new(MAX_TIMESTAMP));

        super::write_generation_checkpoint(&gen_dir, &forward, &reverse)
            .expect("flush should succeed");

        assert!(
            gen_dir.join("forward_chunks/chunk_index.bin").exists(),
            "forward chunk index should exist"
        );
        assert!(
            gen_dir.join("reverse_chunks/chunk_index.bin").exists(),
            "reverse chunk index should exist"
        );
    }
}

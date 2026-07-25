use crate::core::stats::StatsManager;
use crate::core::types::{
    CommitLsn, Index, IndexGeneration, IndexType, SnapshotTimestamp, Timestamp,
};
use crate::core::wal::{EntityRef, OutboxIntent};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::helpers::{flush_split_generation, merge_split_wal_changes};
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser};
use crate::storage::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard, ManifestCatalog,
    ManifestHandle,
};
use crate::storage::index::shard_runtime::{
    generation_from_maps, GenerationRuntime, IndexBarrierRegistry, IndexMaps, IndexRuntime,
};
use crate::storage::index::types::{EdgeIdentity, IndexIdentity, IndexRecord};
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct IndexDataManagerImpl {
    pub(crate) index_root: Option<PathBuf>,
    pub(crate) manifest_catalogs: Arc<RwLock<HashMap<IndexIdentity, Arc<ManifestCatalog>>>>,
    pub(crate) runtimes: Arc<RwLock<HashMap<IndexIdentity, Arc<IndexRuntime>>>>,
    pub(crate) index_aliases: Arc<RwLock<HashMap<(u64, String), u64>>>,
    pub(crate) index_types: Arc<RwLock<HashMap<IndexIdentity, IndexType>>>,
    pub(crate) index_definitions: Arc<RwLock<HashMap<IndexIdentity, Index>>>,
    pub(crate) restored_generations: Arc<RwLock<HashMap<IndexIdentity, IndexGeneration>>>,
    pub(crate) barrier_registry: IndexBarrierRegistry,
    pub(crate) rebuild_gate: Arc<RwLock<()>>,
    pub(crate) stats_manager: Option<Arc<StatsManager>>,
}

impl IndexDataManagerImpl {
    pub fn new() -> Self {
        Self::new_with_optional_root(None)
    }

    pub fn new_with_root(index_root: impl Into<PathBuf>) -> Self {
        Self::new_with_optional_root(Some(index_root.into()))
    }

    fn new_with_optional_root(index_root: Option<PathBuf>) -> Self {
        Self {
            index_root,
            manifest_catalogs: Arc::new(RwLock::new(HashMap::new())),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
            index_aliases: Arc::new(RwLock::new(HashMap::new())),
            index_types: Arc::new(RwLock::new(HashMap::new())),
            index_definitions: Arc::new(RwLock::new(HashMap::new())),
            restored_generations: Arc::new(RwLock::new(HashMap::new())),
            barrier_registry: Arc::new(RwLock::new(HashMap::new())),
            rebuild_gate: Arc::new(RwLock::new(())),
            stats_manager: None,
        }
    }

    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
    }

    pub(crate) fn record_manifest_state(&self, catalog: &ManifestCatalog) {
        if let Some(stats) = &self.stats_manager {
            let state = catalog.stats();
            stats.set_manifest_state(state.active_readers, state.retired_generations);
        }
    }

    pub(crate) fn initial_checkpoint_path(&self, space_id: u64, index_id: u64) -> PathBuf {
        let relative = PathBuf::from(format!("{space_id}/{index_id}/generation-1/shard-0"));
        match self.index_root.as_ref() {
            Some(root) => root.join(relative),
            None => PathBuf::from("memory-index").join(relative),
        }
    }

    pub fn memory_usage_bytes(&self) -> u64 {
        self.runtimes
            .read()
            .values()
            .map(|runtime| runtime.memory_usage_bytes())
            .sum()
    }

    pub fn register_native_index(&self, space_id: u64, index: &Index) -> StorageResult<()> {
        if index.space_id != space_id {
            return Err(StorageError::invalid_operation(format!(
                "Index {} belongs to space {}, not space {}",
                index.name, index.space_id, space_id
            )));
        }
        let index_id = index.id;
        let identity = IndexIdentity { space_id, index_id };
        self.index_aliases
            .write()
            .insert((space_id, index.name.clone()), index_id);
        self.index_types
            .write()
            .insert(identity, index.index_type.clone());
        self.index_definitions
            .write()
            .insert(identity, index.clone());
        let mut catalogs = self.manifest_catalogs.write();
        if let std::collections::hash_map::Entry::Vacant(e) = catalogs.entry(identity) {
            let manifest = IndexManifest::new(
                space_id,
                index_id,
                IndexGeneration::new(1),
                vec![IndexShard {
                    shard_id: 0,
                    lower: None,
                    upper: None,
                    checkpoint_file: self.initial_checkpoint_path(space_id, index_id),
                    checksum: None,
                }],
            )
            .map_err(StorageError::db_error)?;
            e.insert(Arc::new(
                ManifestCatalog::new(manifest).map_err(StorageError::db_error)?,
            ));
        }
        drop(catalogs);
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        self.runtimes
            .write()
            .entry(identity)
            .or_insert_with(|| Arc::new(IndexRuntime::new(catalog.acquire().manifest())));
        self.restore_active_generation(identity, index)?;
        Ok(())
    }

    fn restore_active_generation(
        &self,
        identity: IndexIdentity,
        index: &Index,
    ) -> StorageResult<()> {
        let catalog = self
            .manifest_catalog(identity.space_id, identity.index_id)
            .ok_or_else(|| {
                StorageError::not_found(format!("Index {} has no manifest", identity.index_id))
            })?;
        let handle = catalog.acquire();
        let generation = handle.manifest().generation;
        if self.restored_generations.read().get(&identity) == Some(&generation) {
            return Ok(());
        }
        // If the existing runtime already has this generation installed
        // (e.g. via publish_native_index during rebuild), don't overwrite
        // it by loading from the manifest's checkpoint file, which may be
        // empty in in-memory mode.
        if let Some(runtime) = self.runtimes.read().get(&identity) {
            let has_gen = runtime.generation(generation).is_some();
            if has_gen {
                self.restored_generations
                    .write()
                    .insert(identity, generation);
                return Ok(());
            }
        }
        let runtime = match index.index_type {
            IndexType::TagIndex => IndexRuntime::load::<
                crate::storage::index::key_codec::VertexIndexKeyGen,
            >(handle.manifest())?,
            IndexType::EdgeIndex => IndexRuntime::load::<
                crate::storage::index::key_codec::EdgeIndexKeyGen,
            >(handle.manifest())?,
        };
        self.runtimes.write().insert(identity, Arc::new(runtime));
        self.restored_generations
            .write()
            .insert(identity, generation);
        Ok(())
    }

    pub fn unregister_native_index(&self, space_id: u64, index_name: &str) {
        if let Some(index_id) = self
            .index_aliases
            .write()
            .remove(&(space_id, index_name.to_string()))
        {
            let identity = IndexIdentity { space_id, index_id };
            self.manifest_catalogs.write().remove(&identity);
            self.runtimes.write().remove(&identity);
            self.index_types.write().remove(&identity);
            self.index_definitions.write().remove(&identity);
            self.restored_generations.write().remove(&identity);
            self.barrier_registry
                .write()
                .remove(&(identity.space_id, identity.index_id));
        }
    }

    pub fn manifest_catalog(&self, space_id: u64, index_id: u64) -> Option<Arc<ManifestCatalog>> {
        self.manifest_catalogs
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
    }

    pub fn index_alias(&self, space_id: u64, index_name: &str) -> Option<u64> {
        self.index_aliases
            .read()
            .get(&(space_id, index_name.to_string()))
            .copied()
    }

    pub(crate) fn barrier_registry(&self) -> IndexBarrierRegistry {
        Arc::clone(&self.barrier_registry)
    }

    pub(crate) fn rebuild_gate(&self) -> Arc<RwLock<()>> {
        Arc::clone(&self.rebuild_gate)
    }

    pub(crate) fn record_barrier_lsn(&self, identity: IndexIdentity, barrier_lsn: CommitLsn) {
        if barrier_lsn == CommitLsn::ZERO {
            return;
        }
        let mut barriers = self.barrier_registry.write();
        let entry = barriers
            .entry((identity.space_id, identity.index_id))
            .or_default();
        if barrier_lsn > *entry {
            *entry = barrier_lsn;
        }
    }

    pub(crate) fn advance_barriers(&self, commit_lsn: CommitLsn) {
        if commit_lsn == CommitLsn::ZERO {
            return;
        }
        for (identity, runtime) in self.runtimes.read().iter() {
            runtime.establish_barrier_lsn(commit_lsn);
            self.record_barrier_lsn(*identity, commit_lsn);
        }
    }

    pub(crate) fn wait_for_active_barrier(&self, runtime: &IndexRuntime) {
        let barrier_lsn = runtime.barrier_lsn();
        runtime.wait_for_barrier_lsn(barrier_lsn);
    }

    pub(crate) fn runtime(&self, space_id: u64, index_id: u64) -> StorageResult<Arc<IndexRuntime>> {
        self.runtimes
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no runtime")))
    }

    pub(crate) fn active_generation(
        &self,
        space_id: u64,
        index_id: u64,
    ) -> StorageResult<(ManifestHandle, Arc<IndexRuntime>, Arc<GenerationRuntime>)> {
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let handle = catalog.acquire();
        let runtime = self.runtime(space_id, index_id)?;
        let generation = runtime
            .generation(handle.manifest().generation)
            .ok_or_else(|| {
                StorageError::not_found(format!(
                    "Index {index_id} has no active runtime generation"
                ))
            })?;
        Ok((handle, runtime, generation))
    }

    pub(crate) fn active_index_data(
        &self,
        space_id: u64,
        index_id: u64,
    ) -> StorageResult<IndexMaps> {
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let (_handle, _runtime, generation) = self.active_generation(space_id, index_id)?;
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        for shard in generation.shards() {
            let (shard_forward, shard_reverse) = shard.snapshot();
            forward.extend(shard_forward);
            reverse.extend(shard_reverse);
        }
        Ok((forward, reverse))
    }

    pub(crate) fn publish_native_index(
        &self,
        manifest: IndexManifest,
        forward: BTreeMap<SecondaryIndexKey, IndexRecord>,
        reverse: BTreeMap<SecondaryIndexKey, IndexRecord>,
        barrier_lsn: CommitLsn,
    ) -> StorageResult<()> {
        let runtime = self.runtime(manifest.space_id, manifest.index_id)?;
        let _fence = runtime.write_fence();
        let identity = IndexIdentity {
            space_id: manifest.space_id,
            index_id: manifest.index_id,
        };
        let mut maps = HashMap::new();
        let shard = manifest
            .shards
            .first()
            .ok_or_else(|| StorageError::invalid_operation("Index manifest has no shards"))?;
        maps.insert(shard.shard_id, (forward, reverse));
        runtime.install_generation(generation_from_maps(&manifest, maps));
        self.manifest_catalog(manifest.space_id, manifest.index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?
            .publish(manifest)
            .map_err(StorageError::db_error)?;
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        runtime.establish_barrier_lsn(barrier_lsn);
        self.record_barrier_lsn(identity, barrier_lsn);
        runtime.wait_for_barrier_lsn(barrier_lsn);
        if let Some(catalog) = self.manifest_catalog(identity.space_id, identity.index_id) {
            self.record_manifest_state(&catalog);
        }
        Ok(())
    }

    pub(crate) fn build_state_path(&self, index_root: &Path) -> PathBuf {
        index_root.join("generation_build.bin")
    }

    pub(crate) fn save_build_state(
        &self,
        index_root: &Path,
        state: &GenerationBuildState,
    ) -> StorageResult<()> {
        std::fs::create_dir_all(index_root)?;
        let path = self.build_state_path(index_root);
        let temporary = path.with_extension("tmp");
        let serialized = postcard::to_allocvec(state)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        {
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

    pub(crate) fn load_build_state(
        &self,
        index_root: &Path,
    ) -> StorageResult<Option<GenerationBuildState>> {
        let path = self.build_state_path(index_root);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        let state: GenerationBuildState = postcard::from_bytes(&bytes)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        Ok(Some(state))
    }

    pub(crate) fn remove_build_state(&self, index_root: &Path) -> StorageResult<()> {
        let path = self.build_state_path(index_root);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    pub fn resolve_split_crash_recovery(&self, index_root: &Path) -> StorageResult<()> {
        let Some(build_state) = self.load_build_state(index_root)? else {
            return Ok(());
        };
        let partial_generation =
            index_root.join(format!("generation-{}", build_state.generation.get()));
        if matches!(
            build_state.state,
            GenerationState::Building
                | GenerationState::CatchingUp
                | GenerationState::Failed
                | GenerationState::Cancelled
        ) {
            log::warn!(
                "Discarding incomplete split build state for gen {} (state={:?})",
                build_state.generation,
                build_state.state
            );
            if partial_generation.exists() {
                std::fs::remove_dir_all(&partial_generation)?;
            }
            self.remove_build_state(index_root)?;
        }
        if matches!(build_state.state, GenerationState::Publishing) {
            let manifest_path = index_root.join("manifest.bin");
            let published_generation = manifest_path
                .is_file()
                .then(|| IndexManifest::load(&manifest_path))
                .transpose()?
                .map(|manifest| manifest.generation);
            if published_generation == Some(build_state.generation) {
                log::info!("Completing split build from Publishing state");
                self.remove_build_state(index_root)?;
            } else {
                log::warn!("Discarding split in Publishing state without its published generation");
                if partial_generation.exists() {
                    std::fs::remove_dir_all(&partial_generation)?;
                }
                self.remove_build_state(index_root)?;
            }
        }
        if matches!(build_state.state, GenerationState::Active) {
            self.remove_build_state(index_root)?;
        }
        Ok(())
    }

    pub fn split_native_index<F, G>(
        &self,
        identity: IndexIdentity,
        boundary: Vec<u8>,
        snapshot_timestamp: SnapshotTimestamp,
        start_lsn: CommitLsn,
        barrier_lsn: F,
        wal_intents: G,
    ) -> StorageResult<()>
    where
        F: FnOnce() -> StorageResult<CommitLsn>,
        G: FnOnce(CommitLsn, CommitLsn) -> StorageResult<Vec<OutboxIntent>>,
    {
        let IndexIdentity { space_id, index_id } = identity;
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_build();
        }
        if self.index_root.is_none() {
            return Err(StorageError::invalid_operation(
                "Native index splitting requires a persistent index root",
            ));
        }
        if snapshot_timestamp.get() == 0 || start_lsn == CommitLsn::ZERO {
            return Err(StorageError::invalid_operation(
                "Native index splitting requires non-zero snapshot timestamp and start LSN",
            ));
        }
        if boundary.is_empty() {
            return Err(StorageError::invalid_operation(
                "Index split boundary cannot be empty",
            ));
        }
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let runtime = self.runtime(space_id, index_id)?;
        let current = catalog.acquire().manifest().clone();
        let split_position = current
            .shards
            .iter()
            .position(|shard| shard.contains(&boundary))
            .ok_or_else(|| {
                StorageError::invalid_operation("Split boundary is outside key space")
            })?;
        let shard = current.shards[split_position].clone();
        if shard.lower.as_ref() == Some(&boundary) || shard.upper.as_ref() == Some(&boundary) {
            return Err(StorageError::invalid_operation(
                "Split boundary must be inside an existing shard",
            ));
        }

        let generation = IndexGeneration::new(current.generation.get().saturating_add(1));
        let next_shard_id = current
            .shards
            .iter()
            .map(|entry| entry.shard_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let checkpoint_parent = shard.checkpoint_file.parent().ok_or_else(|| {
            StorageError::invalid_operation("Index shard checkpoint path has no parent")
        })?;
        let index_root = shard
            .checkpoint_file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("shard-"))
            .then(|| {
                checkpoint_parent.parent().ok_or_else(|| {
                    StorageError::invalid_operation("Index shard generation path has no parent")
                })
            })
            .transpose()?
            .unwrap_or(checkpoint_parent);

        self.resolve_split_crash_recovery(index_root)?;
        let mut build_state = GenerationBuildState::new(generation, snapshot_timestamp, start_lsn);
        self.save_build_state(index_root, &build_state)?;

        let next_generation_dir = index_root.join(format!("generation-{}", generation.get()));
        let shard_a_path = next_generation_dir.join(format!("shard-{}", shard.shard_id));
        let shard_b_path = next_generation_dir.join(format!("shard-{next_shard_id}"));

        let index_type = self
            .index_types
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no type")))?;
        let index_definition = self
            .index_definitions
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no definition"))
            })?;

        let mut shards = current.shards.clone();
        shards.splice(
            split_position..=split_position,
            [
                IndexShard {
                    shard_id: shard.shard_id,
                    lower: shard.lower.clone(),
                    upper: Some(boundary.clone()),
                    checkpoint_file: shard_a_path,
                    checksum: None,
                },
                IndexShard {
                    shard_id: next_shard_id,
                    lower: Some(boundary.clone()),
                    upper: shard.upper.clone(),
                    checkpoint_file: shard_b_path,
                    checksum: None,
                },
            ],
        );
        let next = IndexManifest::new(space_id, index_id, generation, shards)
            .map_err(StorageError::db_error)?;
        let mut maps = {
            let _fence = runtime.read_fence();
            let active = runtime
                .generation(current.generation)
                .ok_or_else(|| StorageError::not_found("Active runtime generation is missing"))?;
            let mut maps: HashMap<u32, IndexMaps> = HashMap::new();
            for current_shard in &current.shards {
                let (forward, reverse) = active
                    .shard(current_shard.shard_id)
                    .ok_or_else(|| StorageError::not_found("Active runtime shard is missing"))?
                    .snapshot();
                if current_shard.shard_id != shard.shard_id {
                    maps.insert(current_shard.shard_id, (forward, reverse));
                    continue;
                }
                let (forward_a, forward_b): (BTreeMap<_, _>, BTreeMap<_, _>) = forward
                    .into_iter()
                    .partition(|(key, _)| key.as_slice() < boundary.as_slice());
                let mut entity_shards: Vec<(EntityRef, (Timestamp, u32))> = Vec::new();
                for entry in forward_a.values() {
                    if let Some(entity) = entry.entity_ref.clone() {
                        if let Some((_, (ts, shard_id))) = entity_shards
                            .iter_mut()
                            .find(|(candidate, _)| *candidate == entity)
                        {
                            if entry.created_ts >= *ts {
                                *ts = entry.created_ts;
                                *shard_id = shard.shard_id;
                            }
                        } else {
                            entity_shards.push((entity, (entry.created_ts, shard.shard_id)));
                        }
                    }
                }
                for entry in forward_b.values() {
                    if let Some(entity) = entry.entity_ref.clone() {
                        if let Some((_, (ts, shard_id))) = entity_shards
                            .iter_mut()
                            .find(|(candidate, _)| *candidate == entity)
                        {
                            if entry.created_ts >= *ts {
                                *ts = entry.created_ts;
                                *shard_id = next_shard_id;
                            }
                        } else {
                            entity_shards.push((entity, (entry.created_ts, next_shard_id)));
                        }
                    }
                }
                let mut reverse_a = BTreeMap::new();
                let mut reverse_b = BTreeMap::new();
                for (key, entry) in reverse {
                    let target_shard = entry
                        .entity_ref
                        .as_ref()
                        .and_then(|entity| {
                            entity_shards
                                .iter()
                                .find(|(candidate, _)| candidate == entity)
                        })
                        .map(|(_, (_, shard_id))| *shard_id);
                    if target_shard == Some(next_shard_id) || target_shard.is_none() {
                        reverse_b.insert(key, entry);
                    } else {
                        reverse_a.insert(key, entry);
                    }
                }
                maps.insert(shard.shard_id, (forward_a, reverse_a));
                maps.insert(next_shard_id, (forward_b, reverse_b));
            }
            maps
        };

        build_state
            .transition_to_catching_up()
            .map_err(StorageError::invalid_operation)?;
        self.save_build_state(index_root, &build_state)?;

        let _fence = runtime.write_fence();
        let active_manifest = catalog.acquire().manifest().clone();
        if active_manifest.generation != current.generation {
            return Err(StorageError::invalid_operation(
                "Index generation changed while splitting; retry the split",
            ));
        }
        let barrier_lsn = barrier_lsn()?;
        if barrier_lsn < start_lsn {
            return Err(StorageError::invalid_operation(
                "Split barrier LSN cannot precede its start LSN",
            ));
        }
        let intents = wal_intents(start_lsn, barrier_lsn)?;
        let active = runtime
            .generation(current.generation)
            .ok_or_else(|| StorageError::not_found("Active runtime generation is missing"))?;
        let mut active_forward = BTreeMap::new();
        let mut active_reverse = BTreeMap::new();
        for current_shard in &current.shards {
            let (forward, reverse) = active
                .shard(current_shard.shard_id)
                .ok_or_else(|| StorageError::not_found("Active runtime shard is missing"))?
                .snapshot();
            active_forward.extend(forward);
            active_reverse.extend(reverse);
        }
        match index_type {
            IndexType::TagIndex => {
                let forward_prefix =
                    KeyBuilder::build_vertex_index_prefix(space_id, &index_definition.name).0;
                merge_split_wal_changes(
                    &mut maps,
                    &next,
                    &active_forward,
                    &active_reverse,
                    &intents,
                    |key| key.starts_with(&forward_prefix),
                    |key| {
                        KeyParser::parse_vertex_reverse_key_v2(key)
                            .is_ok_and(|(_, name)| name == index_definition.name)
                    },
                )?;
            }
            IndexType::EdgeIndex => {
                let forward_prefix =
                    KeyBuilder::build_edge_index_prefix(space_id, &index_definition.name).0;
                merge_split_wal_changes(
                    &mut maps,
                    &next,
                    &active_forward,
                    &active_reverse,
                    &intents,
                    |key| key.starts_with(&forward_prefix),
                    |key| {
                        KeyParser::parse_edge_reverse_key(key)
                            .is_ok_and(|(_, _, _, _, name)| name == index_definition.name)
                    },
                )?;
            }
        }

        build_state
            .transition_to_publishing(barrier_lsn)
            .map_err(StorageError::invalid_operation)?;
        self.save_build_state(index_root, &build_state)?;

        let next_runtime = generation_from_maps(&next, maps);
        match index_type {
            IndexType::TagIndex => flush_split_generation::<
                crate::storage::index::key_codec::VertexIndexKeyGen,
            >(&next, &next_runtime)?,
            IndexType::EdgeIndex => flush_split_generation::<
                crate::storage::index::key_codec::EdgeIndexKeyGen,
            >(&next, &next_runtime)?,
        }

        next.store(&index_root.join("manifest.bin"))?;
        runtime.install_generation(next_runtime);
        catalog.publish(next).map_err(StorageError::db_error)?;
        runtime.establish_barrier_lsn(barrier_lsn);
        self.record_barrier_lsn(IndexIdentity { space_id, index_id }, barrier_lsn);
        runtime.wait_for_barrier_lsn(barrier_lsn);
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        self.record_manifest_state(&catalog);

        build_state
            .transition_to_active()
            .map_err(StorageError::invalid_operation)?;
        self.remove_build_state(index_root)?;
        Ok(())
    }

    pub fn flush<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        if self.index_root.is_none() && !self.manifest_catalogs.read().is_empty() {
            return Err(StorageError::invalid_operation(
                "Flushing native indexes requires a persistent index root",
            ));
        }
        let path = path.as_ref();
        let manifest_dir = path.join("native_index_manifests");
        std::fs::create_dir_all(&manifest_dir)?;
        for (identity, catalog) in self.manifest_catalogs.read().iter() {
            let manifest = catalog.acquire();
            let runtime = self.runtime(identity.space_id, identity.index_id)?;
            match self.index_types.read().get(identity) {
                Some(IndexType::TagIndex) => runtime
                    .flush_generation::<crate::storage::index::key_codec::VertexIndexKeyGen>(
                    manifest.manifest(),
                )?,
                Some(IndexType::EdgeIndex) => runtime
                    .flush_generation::<crate::storage::index::key_codec::EdgeIndexKeyGen>(
                    manifest.manifest(),
                )?,
                None => continue,
            }
            manifest.manifest().store(
                &manifest_dir.join(format!("{}-{}.bin", identity.space_id, identity.index_id)),
            )?;
        }
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        let manifest_dir = path.join("native_index_manifests");
        if manifest_dir.is_dir() {
            let mut catalogs = self.manifest_catalogs.write();
            for entry in std::fs::read_dir(manifest_dir)? {
                let manifest_path = entry?.path();
                if manifest_path.extension().and_then(|value| value.to_str()) != Some("bin") {
                    continue;
                }
                let manifest = IndexManifest::load(&manifest_path)?;
                catalogs.insert(
                    IndexIdentity {
                        space_id: manifest.space_id,
                        index_id: manifest.index_id,
                    },
                    Arc::new(ManifestCatalog::new(manifest).map_err(StorageError::db_error)?),
                );
            }
        }
        for space_entry in std::fs::read_dir(path)? {
            let space_path = space_entry?.path();
            if !space_path.is_dir() {
                continue;
            }
            for index_entry in std::fs::read_dir(space_path)? {
                let index_path = index_entry?.path();
                self.resolve_split_crash_recovery(&index_path)?;
                let candidate = index_path.join("manifest.bin");
                if !candidate.is_file() {
                    continue;
                }
                let manifest = IndexManifest::load(&candidate)?;
                self.manifest_catalogs.write().insert(
                    IndexIdentity {
                        space_id: manifest.space_id,
                        index_id: manifest.index_id,
                    },
                    Arc::new(ManifestCatalog::new(manifest).map_err(StorageError::db_error)?),
                );
            }
        }
        Ok(())
    }

    pub(crate) fn clear_vertex_entity(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        self.wait_for_active_barrier(&runtime);
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
        let reverse = KeyBuilder::build_vertex_reverse_key_v2(space_id, vertex_id, index_name)?;
        let reverse_end = KeyBuilder::build_range_end(&reverse);
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            let reverse_keys = shard
                .reverse()
                .read()
                .range(reverse.0.clone()..reverse_end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in reverse_keys {
                if let Some(entry) = shard.reverse().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
            let forward_keys = shard
                .forward()
                .read()
                .iter()
                .filter(|(key, entry)| {
                    entry.is_visible_at(write_ts)
                        && KeyParser::parse_vertex_id_from_key(key)
                            .is_ok_and(|candidate| candidate == *vertex_id)
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in forward_keys {
                if let Some(entry) = shard.forward().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn clear_edge_entity(
        &self,
        edge: &EdgeIdentity<'_>,
        index_name: &str,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let space_id = edge.space_id;
        let src = edge.src;
        let dst = edge.dst;
        let edge_type = edge.edge_type;
        let ranking = edge.ranking;
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        self.wait_for_active_barrier(&runtime);
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
        let reverse =
            KeyBuilder::build_edge_reverse_key(space_id, src, dst, edge_type, ranking, index_name)?;
        let reverse_end = KeyBuilder::build_range_end(&reverse);
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            let reverse_keys = shard
                .reverse()
                .read()
                .range(reverse.0.clone()..reverse_end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in reverse_keys {
                if let Some(entry) = shard.reverse().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
            let forward_keys = shard
                .forward()
                .read()
                .iter()
                .filter(|(key, entry)| {
                    entry.is_visible_at(write_ts)
                        && KeyParser::parse_edge_identity_from_key(key).is_ok_and(
                            |(candidate_src, candidate_dst, candidate_type, candidate_rank)| {
                                candidate_src == *src
                                    && candidate_dst == *dst
                                    && candidate_type == edge_type
                                    && candidate_rank == ranking
                            },
                        )
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in forward_keys {
                if let Some(entry) = shard.forward().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn clear_index(
        &self,
        index_id: u64,
        space_id: u64,
        index_name: &str,
        index_type: IndexType,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        self.wait_for_active_barrier(&runtime);
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
        let prefix = match index_type {
            IndexType::TagIndex => KeyBuilder::build_vertex_index_prefix(space_id, index_name),
            IndexType::EdgeIndex => KeyBuilder::build_edge_index_prefix(space_id, index_name),
        };
        let end = KeyBuilder::build_range_end(&prefix);
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            let forward_keys = shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in forward_keys {
                if let Some(entry) = shard.forward().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
            let reverse_keys = shard
                .reverse()
                .read()
                .iter()
                .filter(|(key, entry)| {
                    entry.is_visible_at(write_ts)
                        && match index_type {
                            IndexType::TagIndex => KeyParser::parse_vertex_reverse_key_v2(key)
                                .is_ok_and(|(_, name)| name == index_name),
                            IndexType::EdgeIndex => KeyParser::parse_edge_reverse_key(key)
                                .is_ok_and(|(_, _, _, _, name)| name == index_name),
                        }
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in reverse_keys {
                if let Some(entry) = shard.reverse().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }
        Ok(())
    }
}

impl Default for IndexDataManagerImpl {
    fn default() -> Self {
        Self::new()
    }
}

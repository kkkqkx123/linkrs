use crate::index::helpers::{
    edge_entity_ref, flush_split_generation, merge_split_wal_changes, vertex_entity_ref,
};
use crate::index::key_codec::key_types::{
    SecondaryIndexKey, KEY_TYPE_EDGE_REVERSE, KEY_TYPE_VERTEX_REVERSE,
};
use crate::index::key_codec::{KeyBuilder, KeyParser};
use crate::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard, ManifestCatalog,
    ManifestHandle,
};
use crate::index::shard_runtime::{
    generation_from_maps_with_pool_capacity, GenerationRuntime, IndexBarrierRegistry, IndexMaps,
    IndexRuntime,
};
use crate::index::types::{EdgeIdentity, IndexIdentity, IndexRecord};
use crate::persistence::{read_versioned_payload, write_versioned_payload};
use graphdb_core::stats::StatsManager;
use graphdb_core::types::{
    CommitLsn, Index, IndexGeneration, IndexType, SnapshotTimestamp, Timestamp,
};
use graphdb_core::value::ordered_codec::OrderedCodec;
use graphdb_core::wal::{EntityRef, OutboxIntent};
use graphdb_core::{StorageError, StorageResult, Value};
use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use super::remove_dir_if_empty;
use super::IndexDataManagerImpl;
use super::PendingDelta;
use super::PendingExistingScan;

impl IndexDataManagerImpl {
    /// Number of retired generations awaiting reclamation across all indexes.
    pub fn retired_generation_count(&self) -> usize {
        self.manifest_catalogs
            .read()
            .values()
            .map(|catalog| catalog.retired_reclaimable(|_| true).len())
            .sum()
    }

    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
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
        let catalog_already_loaded = catalogs.contains_key(&identity);
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
            )?;
            e.insert(Arc::new(ManifestCatalog::new(manifest)?));
        }
        drop(catalogs);
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let manifest = catalog.acquire().manifest().clone();
        self.runtimes.write().entry(identity).or_insert_with(|| {
            // When the catalog was restored from disk, its manifest points to real
            // shard checkpoint files. Load the runtime from disk so that shard data
            // is actually available, instead of creating an empty generation that
            // satisfies the `restore_active_generation` check without any data.
            let has_disk_shards = catalog_already_loaded
                && manifest.shards.iter().any(|s| s.checkpoint_file.is_dir());
            if has_disk_shards {
                let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
                let runtime = IndexRuntime::load_with_pool_capacity(&manifest, pool_cap)
                    .unwrap_or_else(|_| IndexRuntime::new(&manifest));
                Arc::new(runtime)
            } else {
                Arc::new(IndexRuntime::new(&manifest))
            }
        });
        self.restore_active_generation(identity)?;
        self.sync_memory_usage();
        Ok(())
    }

    pub(crate) fn restore_active_generation(&self, identity: IndexIdentity) -> StorageResult<()> {
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
        let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
        let runtime = IndexRuntime::load_with_pool_capacity(handle.manifest(), pool_cap)?;
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
        let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
        runtime.install_generation(generation_from_maps_with_pool_capacity(
            &manifest,
            maps,
            None,
            0,
            Vec::new(),
            Vec::new(),
            pool_cap,
        ));
        self.manifest_catalog(manifest.space_id, manifest.index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?
            .publish(manifest)?;
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        runtime.establish_barrier_lsn(barrier_lsn);
        self.record_barrier_lsn(identity, barrier_lsn);
        runtime.wait_for_barrier_lsn(barrier_lsn);
        if let Some(catalog) = self.manifest_catalog(identity.space_id, identity.index_id) {
            self.record_manifest_state(&catalog);
        }
        self.sync_memory_usage();
        Ok(())
    }

    /// Remove generations whose max_ts < safe_ts from all runtimes.
    /// Returns the number of generations retired.
    pub(crate) fn retire_generations(&self, safe_ts: Timestamp) -> usize {
        if safe_ts == 0 {
            return 0;
        }
        let mut retired = 0;
        let identities: Vec<IndexIdentity> = self.runtimes.read().keys().copied().collect();
        for identity in identities {
            let Some(catalog) = self.manifest_catalog(identity.space_id, identity.index_id) else {
                continue;
            };
            let active_gen = catalog.acquire().manifest().generation;
            let Some(runtime) = self.runtime(identity.space_id, identity.index_id).ok() else {
                continue;
            };
            let mut to_remove = Vec::new();
            // Check all non-active generations
            for gen in runtime.generations() {
                if gen.generation < active_gen && safe_ts > gen.max_ts {
                    to_remove.push(gen.generation);
                }
            }
            for gen in to_remove {
                if runtime.remove_generation(gen) {
                    retired += 1;
                }
            }
            if retired > 0 {
                self.record_manifest_state(&catalog);
                if let Err(error) = self.reclaim_retired_generations(identity) {
                    log::warn!(
                        "Failed to reclaim retired generation files for index {} (space {}): {error}",
                        identity.index_id,
                        identity.space_id
                    );
                }
                self.sync_memory_usage();
                self.resync_tombstone_count();
            }
        }
        retired
    }

    /// Acquire a manifest pin for every generation in a runtime chain, fencing
    /// their checkpoint files from reclamation for as long as the returned
    /// handles are alive. Every holder of a generation chain (cursor or
    /// transient snapshot) must pin the chain's manifests, otherwise a
    /// reclamation could delete files that a lazy chunk reload still needs.
    pub(crate) fn pin_chain_manifests(
        &self,
        catalog: &ManifestCatalog,
        chain: &[Arc<GenerationRuntime>],
    ) -> Vec<ManifestHandle> {
        chain
            .iter()
            .filter_map(|gen| catalog.acquire_generation(gen.generation))
            .collect()
    }

    /// Delete the checkpoint files of retired generations that are both free
    /// of reader handles (per the manifest catalog) and no longer installed in
    /// the runtime. Removes the manifest from the catalog only after its files
    /// have been physically deleted, so a failed deletion retries later.
    /// Returns the number of checkpoint directories reclaimed.
    pub(crate) fn reclaim_retired_generations(
        &self,
        identity: IndexIdentity,
    ) -> StorageResult<usize> {
        let Some(catalog) = self.manifest_catalog(identity.space_id, identity.index_id) else {
            return Ok(0);
        };
        let runtime = self.runtime(identity.space_id, identity.index_id)?;
        let active_generation = catalog.acquire().manifest().generation;
        let candidates = catalog.retired_reclaimable(|manifest| {
            manifest.generation < active_generation
                && runtime.generation(manifest.generation).is_none()
        });
        let mut reclaimed = 0;
        for manifest in candidates {
            for shard in &manifest.shards {
                if shard.checkpoint_file.is_dir() {
                    std::fs::remove_dir_all(&shard.checkpoint_file)?;
                    reclaimed += 1;
                }
            }
            // Best-effort removal of the now-empty per-generation directory.
            if let Some(parent) = manifest
                .shards
                .first()
                .and_then(|s| s.checkpoint_file.parent())
            {
                remove_dir_if_empty(parent);
            }
            catalog.remove_retired(manifest.generation);
        }
        Ok(reclaimed)
    }

    pub(crate) fn compact_native_index(
        &self,
        identity: IndexIdentity,
        safe_ts: Timestamp,
    ) -> StorageResult<bool> {
        self.compact_native_index_impl(identity, safe_ts, false)
    }

    /// Internal compact implementation with optional force flag.
    /// When `force` is true, merges all generations regardless of tombstones.
    pub(crate) fn compact_native_index_impl(
        &self,
        identity: IndexIdentity,
        safe_ts: Timestamp,
        force: bool,
    ) -> StorageResult<bool> {
        // fold pending writes into the generation chain before compacting.
        self.publish_pending_delta(identity)?;
        let catalog = self
            .manifest_catalog(identity.space_id, identity.index_id)
            .ok_or_else(|| {
                StorageError::not_found(format!("Index {} has no manifest", identity.index_id))
            })?;
        let runtime = self.runtime(identity.space_id, identity.index_id)?;
        let current = catalog.acquire().manifest().clone();

        // Quick pre-check: skip if no tombstones exist (unless forced)
        if !force {
            let has_tombstones = current.shards.iter().any(|s| {
                runtime
                    .generation(current.generation)
                    .and_then(|g| g.shard(s.shard_id))
                    .is_some_and(|shard| {
                        shard
                            .read_forward()
                            .snapshot()
                            .into_values()
                            .any(|e| e.deleted_ts.is_some())
                            || shard
                                .read_reverse()
                                .snapshot()
                                .into_values()
                                .any(|e| e.deleted_ts.is_some())
                    })
            });
            if !has_tombstones {
                return Ok(false);
            }
        }

        // Step 1: Snapshot full generation chain, merging visible entries
        let maps = {
            let chain = runtime.generation_chain_until(current.generation)?;
            // Pin the generation chain so a concurrent reclamation cannot
            // delete checkpoint files this snapshot may lazily reload.
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);
            let mut maps: HashMap<u32, IndexMaps> = HashMap::new();
            for shard_def in &current.shards {
                let mut forward = BTreeMap::new();
                let mut reverse = BTreeMap::new();
                for gen in &chain {
                    if let Some(shard) = gen.shard(shard_def.shard_id) {
                        let (f, r) = shard.snapshot();
                        for (key, entry) in f {
                            if entry.is_visible_at(safe_ts) {
                                forward.entry(key).or_insert(entry);
                            }
                        }
                        for (key, entry) in r {
                            if entry.is_visible_at(safe_ts) {
                                reverse.entry(key).or_insert(entry);
                            }
                        }
                    }
                }
                maps.insert(shard_def.shard_id, (forward, reverse));
            }
            maps
        };

        // Step 2: Create new manifest with next generation
        let next_gen = IndexGeneration::new(current.generation.get().saturating_add(1));
        let new_shards: Vec<IndexShard> = current
            .shards
            .iter()
            .map(|s| {
                let path = self.generation_checkpoint_path(
                    identity.space_id,
                    identity.index_id,
                    next_gen,
                    s.shard_id,
                );
                IndexShard {
                    shard_id: s.shard_id,
                    lower: s.lower.clone(),
                    upper: s.upper.clone(),
                    checkpoint_file: path,
                    checksum: None,
                }
            })
            .collect();
        let next_manifest =
            IndexManifest::new(identity.space_id, identity.index_id, next_gen, new_shards)?;

        // Step 3: Create generation runtime and flush before publishing
        let current_gen = runtime.generation(current.generation);
        let (fwd_prefix, rev_prefix) = self.compute_prefixes(identity);
        let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
        let next_runtime = generation_from_maps_with_pool_capacity(
            &next_manifest,
            maps,
            current_gen.as_ref(),
            safe_ts,
            fwd_prefix,
            rev_prefix,
            pool_cap,
        );
        let index_type = self.index_types.read().get(&identity).cloned();
        if self.index_root.is_some() && index_type.is_some() {
            flush_split_generation(&next_manifest, &next_runtime)?;
        }

        // Step 4: Publish
        let active_gen = catalog.acquire().manifest().generation;
        if active_gen != current.generation {
            return Err(StorageError::invalid_operation(
                "Index generation changed while compacting; retry",
            ));
        }

        runtime.install_generation(next_runtime);
        catalog.publish(next_manifest)?;
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        self.record_barrier_lsn(identity, CommitLsn::ZERO);

        // Step 5: Retire old generation if safe_ts has advanced past its max_ts
        if let Some(gen) = current_gen {
            if safe_ts > gen.max_ts {
                runtime.remove_generation(current.generation);
            }
        }

        // Reclaim checkpoint files of generations that are both unreferenced
        // by any reader handle and no longer installed in the runtime.
        self.reclaim_retired_generations(identity)?;

        self.sync_memory_usage();

        if let Some(catalog_ref) = self.manifest_catalog(identity.space_id, identity.index_id) {
            self.record_manifest_state(&catalog_ref);
        }

        Ok(true)
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
        let serialized = postcard::to_allocvec(state)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        let mut versioned = Vec::new();
        write_versioned_payload(
            &mut versioned,
            graphdb_core::types::StorageVersion::CURRENT as u32,
            &serialized,
        );
        crate::persistence::write_file_atomic(&path, &versioned)
    }

    pub(crate) fn load_build_state(
        &self,
        index_root: &Path,
    ) -> StorageResult<Option<GenerationBuildState>> {
        let path = self.build_state_path(index_root);
        if !path.exists() {
            return Ok(None);
        }
        let mut file = std::fs::File::open(&path)?;
        let (_version, payload) = read_versioned_payload(&mut file, "generation_build.bin")?;
        let state: GenerationBuildState = postcard::from_bytes(&payload)
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
            GenerationState::Building | GenerationState::CatchingUp
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
        // fold pending writes into the generation chain before splitting.
        self.publish_pending_delta(identity)?;
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
        let next = IndexManifest::new(space_id, index_id, generation, shards)?;
        let mut maps = {
            let chain = runtime.generation_chain_until(current.generation)?;
            // Pin the generation chain so a concurrent reclamation cannot
            // delete the checkpoint files this snapshot may lazily reload.
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);
            let mut maps: HashMap<u32, IndexMaps> = HashMap::new();
            for current_shard in &current.shards {
                let mut forward = BTreeMap::new();
                let mut reverse = BTreeMap::new();
                for gen in &chain {
                    if let Some(shard) = gen.shard(current_shard.shard_id) {
                        let (f, r) = shard.snapshot();
                        for (key, entry) in f {
                            if entry.is_visible_at(snapshot_timestamp.get()) {
                                forward.entry(key).or_insert(entry);
                            }
                        }
                        for (key, entry) in r {
                            if entry.is_visible_at(snapshot_timestamp.get()) {
                                reverse.entry(key).or_insert(entry);
                            }
                        }
                    }
                }
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

        build_state.transition_to_catching_up()?;
        self.save_build_state(index_root, &build_state)?;

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

        build_state.transition_to_publishing(barrier_lsn)?;
        self.save_build_state(index_root, &build_state)?;

        let current_gen = runtime.generation(current.generation);
        let (fwd_prefix, rev_prefix) = self.compute_prefixes(identity);
        let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
        let next_runtime = generation_from_maps_with_pool_capacity(
            &next,
            maps,
            current_gen.as_ref(),
            0,
            fwd_prefix,
            rev_prefix,
            pool_cap,
        );
        flush_split_generation(&next, &next_runtime)?;

        next.store(&index_root.join("manifest.bin"))?;
        runtime.install_generation(next_runtime);
        catalog.publish(next)?;
        runtime.establish_barrier_lsn(barrier_lsn);
        self.record_barrier_lsn(IndexIdentity { space_id, index_id }, barrier_lsn);
        runtime.wait_for_barrier_lsn(barrier_lsn);
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        self.record_manifest_state(&catalog);
        self.reclaim_retired_generations(IndexIdentity { space_id, index_id })?;

        build_state.transition_to_active()?;
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
            if self.index_types.read().get(identity).is_some() {
                runtime.flush_generation(manifest.manifest())?;
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
                    Arc::new(ManifestCatalog::new(manifest)?),
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
                    Arc::new(ManifestCatalog::new(manifest)?),
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
        let identity = IndexIdentity { space_id, index_id };
        // fold pending writes into the chain so their entries can be
        // tombstoned; otherwise a delete would miss accumulated entries.
        self.publish_pending_delta(identity)?;
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            self.wait_for_active_barrier(&runtime);
            let catalog = self.manifest_catalog(space_id, index_id).ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no manifest"))
            })?;
            let handle = catalog.acquire();
            let chain = runtime.generation_chain_until(handle.manifest().generation)?;
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);

            let reverse_prefix =
                KeyBuilder::build_vertex_reverse_key_v2(space_id, vertex_id, index_name)?;
            let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);

            let mut reverse_meta: Vec<(SecondaryIndexKey, Vec<u8>)> = Vec::new();
            for gen in &chain {
                for shard in gen.shards() {
                    if !shard.reverse_may_have_range(&reverse_prefix.0, &reverse_end.0) {
                        continue;
                    }
                    for (key, record) in shard.reverse_range(&reverse_prefix.0, &reverse_end.0) {
                        if !record.is_visible_at(write_ts) {
                            continue;
                        }
                        if let Ok(encoded) = KeyParser::extract_value_from_reverse_key(&key) {
                            reverse_meta.push((key, encoded));
                        }
                    }
                }
            }

            if reverse_meta.is_empty() {
                return Ok(());
            }

            let route = |key: &[u8]| -> StorageResult<u32> {
                handle
                    .manifest()
                    .route_key(key)
                    .map(|s| s.shard_id)
                    .ok_or_else(|| {
                        StorageError::invalid_operation(
                            "Index manifest does not cover the ordered key",
                        )
                    })
            };

            let mut per_shard: HashMap<u32, IndexMaps> = HashMap::new();
            let entity_ref = vertex_entity_ref(vertex_id);

            let encoded_values: Vec<Vec<u8>> =
                reverse_meta.iter().map(|(_, e)| e.clone()).collect();
            for (rev_key, _) in reverse_meta {
                let shard_id = route(&rev_key)?;
                let (_, ref mut rev_map) = per_shard.entry(shard_id).or_default();
                let mut entry = IndexRecord::new(write_ts);
                entry.mark_deleted(write_ts);
                if let Some(ref e) = entity_ref {
                    entry = entry.with_entity_ref(e.clone());
                }
                rev_map.insert(rev_key, entry);
            }

            let mut seen_fwd: HashSet<Vec<u8>> = HashSet::new();
            for encoded in &encoded_values {
                let Ok(value) = OrderedCodec::new().decode(encoded) else {
                    continue;
                };
                let Ok(forward) =
                    KeyBuilder::build_vertex_index_key(space_id, index_name, &value, vertex_id)
                else {
                    continue;
                };
                if !seen_fwd.insert(forward.0.clone()) {
                    continue;
                }
                let fwd_end = KeyBuilder::build_range_end(&forward);

                let mut fwd_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    for shard in gen.shards() {
                        for (key, record) in shard.forward_range(&forward.0, &fwd_end.0) {
                            if record.is_visible_at(write_ts) {
                                fwd_keys.push(key);
                            }
                        }
                    }
                }

                for fwd_key in &fwd_keys {
                    let shard_id = route(fwd_key)?;
                    let (ref mut fwd_map, _) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    if let Some(ref e) = entity_ref {
                        entry = entry.with_entity_ref(e.clone());
                    }
                    fwd_map.insert(fwd_key.clone(), entry);
                }
            }

            per_shard
        };

        if !delta.is_empty() {
            self.accumulate_delta(identity, delta, write_ts)?;
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
        let identity = IndexIdentity { space_id, index_id };
        // fold pending writes into the chain so their entries can be
        // tombstoned; otherwise a delete would miss accumulated entries.
        self.publish_pending_delta(identity)?;
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            self.wait_for_active_barrier(&runtime);
            let catalog = self.manifest_catalog(space_id, index_id).ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no manifest"))
            })?;
            let handle = catalog.acquire();
            let chain = runtime.generation_chain_until(handle.manifest().generation)?;
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);

            let reverse_prefix = KeyBuilder::build_edge_reverse_key(
                space_id, src, dst, edge_type, ranking, index_name,
            )?;
            let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);

            let mut reverse_meta: Vec<(SecondaryIndexKey, Vec<u8>)> = Vec::new();
            for gen in &chain {
                for shard in gen.shards() {
                    if !shard.reverse_may_have_range(&reverse_prefix.0, &reverse_end.0) {
                        continue;
                    }
                    for (key, record) in shard.reverse_range(&reverse_prefix.0, &reverse_end.0) {
                        if !record.is_visible_at(write_ts) {
                            continue;
                        }
                        if let Ok(encoded) = KeyParser::extract_value_from_edge_reverse_key(&key) {
                            reverse_meta.push((key, encoded));
                        }
                    }
                }
            }

            if reverse_meta.is_empty() {
                return Ok(());
            }

            let route = |key: &[u8]| -> StorageResult<u32> {
                handle
                    .manifest()
                    .route_key(key)
                    .map(|s| s.shard_id)
                    .ok_or_else(|| {
                        StorageError::invalid_operation(
                            "Index manifest does not cover the ordered key",
                        )
                    })
            };

            let mut per_shard: HashMap<u32, IndexMaps> = HashMap::new();
            let entity_ref = edge_entity_ref(src, dst, edge_type, ranking);

            let encoded_values: Vec<Vec<u8>> =
                reverse_meta.iter().map(|(_, e)| e.clone()).collect();
            for (rev_key, _) in reverse_meta {
                let shard_id = route(&rev_key)?;
                let (_, ref mut rev_map) = per_shard.entry(shard_id).or_default();
                let mut entry = IndexRecord::new(write_ts);
                entry.mark_deleted(write_ts);
                if let Some(ref e) = entity_ref {
                    entry = entry.with_entity_ref(e.clone());
                }
                rev_map.insert(rev_key, entry);
            }

            let mut seen_fwd: HashSet<Vec<u8>> = HashSet::new();
            for encoded in &encoded_values {
                let Ok(value) = OrderedCodec::new().decode(encoded) else {
                    continue;
                };
                let Ok(forward) = KeyBuilder::build_edge_index_key(
                    space_id, index_name, &value, src, dst, edge_type, ranking,
                ) else {
                    continue;
                };
                if !seen_fwd.insert(forward.0.clone()) {
                    continue;
                }
                let fwd_end = KeyBuilder::build_range_end(&forward);

                let mut fwd_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    for shard in gen.shards() {
                        for (key, record) in shard.forward_range(&forward.0, &fwd_end.0) {
                            if record.is_visible_at(write_ts) {
                                fwd_keys.push(key);
                            }
                        }
                    }
                }

                for fwd_key in &fwd_keys {
                    let shard_id = route(fwd_key)?;
                    let (ref mut fwd_map, _) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    if let Some(ref e) = entity_ref {
                        entry = entry.with_entity_ref(e.clone());
                    }
                    fwd_map.insert(fwd_key.clone(), entry);
                }
            }

            per_shard
        };

        if !delta.is_empty() {
            self.accumulate_delta(identity, delta, write_ts)?;
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
        let identity = IndexIdentity { space_id, index_id };
        // fold pending writes into the chain before clearing the index.
        self.publish_pending_delta(identity)?;
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            self.wait_for_active_barrier(&runtime);
            let catalog = self.manifest_catalog(space_id, index_id).ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no manifest"))
            })?;
            let handle = catalog.acquire();
            let chain = runtime.generation_chain_until(handle.manifest().generation)?;
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);

            let (prefix, end) = match index_type {
                IndexType::TagIndex => {
                    let p = KeyBuilder::build_vertex_index_prefix(space_id, index_name);
                    let e = KeyBuilder::build_range_end(&p);
                    (p, e)
                }
                IndexType::EdgeIndex => {
                    let p = KeyBuilder::build_edge_index_prefix(space_id, index_name);
                    let e = KeyBuilder::build_range_end(&p);
                    (p, e)
                }
            };

            let route = |key: &[u8]| -> StorageResult<u32> {
                handle
                    .manifest()
                    .route_key(key)
                    .map(|s| s.shard_id)
                    .ok_or_else(|| {
                        StorageError::invalid_operation(
                            "Index manifest does not cover the ordered key",
                        )
                    })
            };

            let mut per_shard: HashMap<u32, IndexMaps> = HashMap::new();

            for shard_def in &handle.manifest().shards {
                let mut fwd_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    if let Some(shard) = gen.shard(shard_def.shard_id) {
                        for (key, record) in shard.forward_range(&prefix.0, &end.0) {
                            if record.is_visible_at(write_ts) {
                                fwd_keys.push(key);
                            }
                        }
                    }
                }

                for fwd_key in fwd_keys {
                    let shard_id = route(&fwd_key)?;
                    let (ref mut fwd_map, _) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    fwd_map.insert(fwd_key, entry);
                }

                let rev_match: fn(&[u8], &str) -> bool = match index_type {
                    IndexType::TagIndex => |key, name| {
                        KeyParser::parse_vertex_reverse_key_v2(key).is_ok_and(|(_, n)| n == name)
                    },
                    IndexType::EdgeIndex => |key, name| {
                        KeyParser::parse_edge_reverse_key(key)
                            .is_ok_and(|(_, _, _, _, n)| n == name)
                    },
                };

                let mut rev_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    if let Some(shard) = gen.shard(shard_def.shard_id) {
                        for (key, record) in shard.iter_reverse() {
                            if record.is_visible_at(write_ts) && rev_match(&key, index_name) {
                                rev_keys.push(key);
                            }
                        }
                    }
                }

                for rev_key in rev_keys {
                    let shard_id = route(&rev_key)?;
                    let (_, ref mut rev_map) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    rev_map.insert(rev_key, entry);
                }
            }

            per_shard
        };

        if !delta.is_empty() {
            self.accumulate_delta(identity, delta, write_ts)?;
        }
        Ok(())
    }
}

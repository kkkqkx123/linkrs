use crate::index::helpers::flush_split_generation;
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::manifest::{
    IndexManifest, IndexShard, ManifestCatalog,
    ManifestHandle,
};
use crate::index::shard_runtime::{
    generation_from_maps_with_pool_capacity, GenerationRuntime, IndexMaps,
};
use crate::index::types::{IndexIdentity, IndexRecord};
use graphdb_core::types::{
    CommitLsn, IndexGeneration, Timestamp,
};
use graphdb_core::{StorageError, StorageResult};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::super::remove_dir_if_empty;
use super::super::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    /// Number of retired generations awaiting reclamation across all indexes.
    pub fn retired_generation_count(&self) -> usize {
        self.manifest_catalogs
            .read()
            .values()
            .map(|catalog| catalog.retired_reclaimable(|_| true).len())
            .sum()
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
}

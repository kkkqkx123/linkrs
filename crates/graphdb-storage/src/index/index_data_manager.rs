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
    /// Memory limit in bytes for all indexes. 0 means unlimited.
    pub(crate) memory_limit_bytes: Arc<AtomicU64>,
    /// Cached total memory usage for fast check_memory_limit
    pub(crate) total_memory_usage: Arc<AtomicU64>,
    /// Per-shard buffer pool capacity in bytes.
    pub(crate) pool_capacity: Arc<AtomicU64>,
    /// Enable chunk-level eviction under memory pressure.
    pub(crate) eviction_enabled: Arc<AtomicBool>,
    /// Eviction high-water ratio (stored as ratio * 10000 as integer).
    pub(crate) eviction_high_ratio: Arc<AtomicU64>,
    /// Eviction low-water ratio (stored as ratio * 10000 as integer).
    pub(crate) eviction_low_ratio: Arc<AtomicU64>,
    /// Cached total tombstone count, maintained incrementally on the write path
    /// and resynced by the (rare) GC/retirement/compaction paths. Keeps the
    /// per-statement admission check from scanning every generation.
    pub(crate) cached_tombstone_count: Arc<AtomicU64>,
    /// per-index deltas awaiting publication into a new generation.
    ///
    /// Writes accumulate here (O(1) per statement) instead of publishing a new
    /// generation per statement. The pending delta is folded into a fresh
    /// generation when the entry count reaches `delta_publish_threshold` or
    /// when a read needs a stable snapshot (`publish_pending_delta`).
    pub(crate) pending_deltas: Arc<Mutex<HashMap<IndexIdentity, PendingDelta>>>,
    /// number of pending entries that triggers publication of a new
    /// generation. A value of 0 or 1 disables accumulation, restoring the
    /// per-statement publish behavior (rollback path).
    pub(crate) delta_publish_threshold: Arc<AtomicUsize>,
}

/// Accumulated index deltas awaiting publication into a new generation.
#[derive(Debug, Default, Clone)]
pub(crate) struct PendingDelta {
    /// Per-shard forward/reverse maps with FULL (prefix-included) keys.
    pub(crate) per_shard: HashMap<u32, IndexMaps>,
    /// Number of entries accumulated (sum of forward + reverse keys).
    pub(crate) entries: usize,
    /// Latest write timestamp among the accumulated entries.
    pub(crate) write_ts: Timestamp,
}

/// Mutable accumulators for a pending-delta existing-value scan.
pub(crate) struct PendingExistingScan<'a> {
    pub existing_values: &'a mut Vec<Value>,
    pub existing_encoded: &'a mut HashSet<Vec<u8>>,
    pub existing_columns: &'a mut Vec<(String, Value)>,
    pub covering_populated: &'a mut bool,
}

/// Merge pending-delta reverse entries for `[reverse_prefix, reverse_end)`
/// into the caller's existing-value scan, so the write path observes
/// previously accumulated (but not yet published) entries when computing the
/// diff for a re-written entity.
pub(crate) fn merge_pending_existing_values(
    pending: &PendingDelta,
    reverse_prefix: &[u8],
    reverse_end: &[u8],
    write_ts: Timestamp,
    is_edge: bool,
    scan: &mut PendingExistingScan<'_>,
) {
    use std::ops::Bound;
    for (_, rev_map) in pending.per_shard.values() {
        for (key, record) in rev_map.range((
            Bound::Included(reverse_prefix.to_vec()),
            Bound::Excluded(reverse_end.to_vec()),
        )) {
            if !record.is_visible_at(write_ts) {
                continue;
            }
            let extracted = if is_edge {
                KeyParser::extract_value_from_edge_reverse_suffix(key)
            } else {
                KeyParser::extract_value_from_reverse_suffix(key)
            };
            if let Ok(encoded) = extracted {
                if scan.existing_encoded.insert(encoded.clone()) {
                    if let Ok(value) = OrderedCodec::new().decode(&encoded) {
                        scan.existing_values.push(
                            crate::index::key_codec::key_builder::normalize_int_value(&value),
                        );
                    }
                }
            }
            if !*scan.covering_populated {
                if let Some(cols) = &record.included_columns {
                    scan.existing_columns.clone_from(cols);
                    *scan.covering_populated = true;
                }
            }
        }
    }
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
            memory_limit_bytes: Arc::new(AtomicU64::new(0)),
            total_memory_usage: Arc::new(AtomicU64::new(0)),
            pool_capacity: Arc::new(AtomicU64::new(128 * 1024 * 1024)),
            eviction_enabled: Arc::new(AtomicBool::new(true)),
            eviction_high_ratio: Arc::new(AtomicU64::new(8500)),
            eviction_low_ratio: Arc::new(AtomicU64::new(6500)),
            cached_tombstone_count: Arc::new(AtomicU64::new(0)),
            pending_deltas: Arc::new(Mutex::new(HashMap::new())),
            delta_publish_threshold: Arc::new(AtomicUsize::new(512)),
        }
    }

    /// Cheap O(1) read of the cached tombstone count. See
    /// [`cached_tombstone_count`](Self::cached_tombstone_count).
    pub fn cached_tombstone_count(&self) -> u64 {
        self.cached_tombstone_count.load(Ordering::Relaxed)
    }

    /// Number of retired generations awaiting reclamation across all indexes.
    pub fn retired_generation_count(&self) -> usize {
        self.manifest_catalogs
            .read()
            .values()
            .map(|catalog| catalog.retired_reclaimable(|_| true).len())
            .sum()
    }

    /// Resync the cached tombstone count from a full scan. Called by the rare
    /// GC/retirement/compaction paths where generations are physically removed.
    pub(crate) fn resync_tombstone_count(&self) {
        let count = self.full_tombstone_count();
        self.cached_tombstone_count
            .store(count as u64, Ordering::Relaxed);
    }

    /// Count tombstoned entries across all runtimes, generations and shards.
    fn full_tombstone_count(&self) -> usize {
        let mut count = 0;
        for runtime in self.runtimes.read().values() {
            for generation in runtime.generations() {
                for shard in generation.shards() {
                    for (_, entry) in shard.read_forward().snapshot() {
                        if entry.deleted_ts.is_some() {
                            count += 1;
                        }
                    }
                    for (_, entry) in shard.read_reverse().snapshot() {
                        if entry.deleted_ts.is_some() {
                            count += 1;
                        }
                    }
                }
            }
        }
        // include deltas still awaiting generation publication.
        for entry in self.pending_deltas.lock().values() {
            for (forward, reverse) in entry.per_shard.values() {
                count += forward
                    .values()
                    .chain(reverse.values())
                    .filter(|record| record.deleted_ts.is_some())
                    .count();
            }
        }
        count
    }

    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
    }

    /// Set memory limit in bytes for all indexes. 0 means unlimited.
    pub fn set_memory_limit_bytes(&self, limit: u64) {
        self.memory_limit_bytes.store(limit, Ordering::Relaxed);
    }

    /// Set per-shard buffer pool capacity in bytes.
    pub fn set_pool_capacity(&self, capacity: u64) {
        self.pool_capacity.store(capacity, Ordering::Relaxed);
    }

    /// Set eviction configuration.
    pub fn set_eviction_config(&self, enabled: bool, high_ratio: f64, low_ratio: f64) {
        self.eviction_enabled.store(enabled, Ordering::Relaxed);
        self.eviction_high_ratio
            .store((high_ratio * 10000.0) as u64, Ordering::Relaxed);
        self.eviction_low_ratio
            .store((low_ratio * 10000.0) as u64, Ordering::Relaxed);
    }

    fn eviction_enabled(&self) -> bool {
        self.eviction_enabled.load(Ordering::Relaxed)
    }

    fn eviction_high_ratio(&self) -> f64 {
        self.eviction_high_ratio.load(Ordering::Relaxed) as f64 / 10000.0
    }

    fn eviction_low_ratio(&self) -> f64 {
        self.eviction_low_ratio.load(Ordering::Relaxed) as f64 / 10000.0
    }

    /// Get current memory usage and check against limit. If exceeded, trigger
    /// eviction of cold chunks first, then compaction on the index with the
    /// most generations or retire old ones.
    pub(crate) fn check_memory_limit(&self) -> StorageResult<()> {
        let limit = self.memory_limit_bytes.load(Ordering::Relaxed);
        if limit == 0 {
            return Ok(());
        }
        let usage = self.total_memory_usage.load(Ordering::Relaxed);
        if usage <= limit {
            return Ok(());
        }

        // Step 1: evict cold chunks under memory pressure
        if self.eviction_enabled() {
            let high = self.eviction_high_ratio();
            let low = self.eviction_low_ratio();
            self.evict_cold_chunks_for_pressure(limit, high, low)?;
        }

        // Step 2: compaction if still over limit
        if self.memory_usage_bytes() > limit {
            let runtimes = self.runtimes.read();
            let mut target: Option<(IndexIdentity, Arc<IndexRuntime>)> = None;
            for (identity, runtime) in runtimes.iter() {
                let gen_count = runtime.generations().len();
                if gen_count > 1 {
                    match &target {
                        None => target = Some((*identity, Arc::clone(runtime))),
                        Some((_, max_r)) if gen_count > max_r.generations().len() => {
                            target = Some((*identity, Arc::clone(runtime)));
                        }
                        _ => {}
                    }
                }
            }
            drop(runtimes);

            if let Some((identity, runtime)) = target {
                let safe_ts = runtime.barrier_lsn().get();
                self.compact_native_index(identity, safe_ts)?;
                // If still over limit, force compact to merge all generations
                if self.memory_usage_bytes() > limit {
                    self.compact_native_index_impl(identity, safe_ts, true)?;
                }
            } else {
                // No index with multiple generations; try retiring oldest non-active
                // generation across all indexes to reduce memory pressure
                self.retire_generations(u64::MAX);
            }
        }

        let new_usage = self.memory_usage_bytes();
        self.total_memory_usage.store(new_usage, Ordering::Relaxed);
        Ok(())
    }

    /// Evict cold chunks across all runtimes to bring memory down toward the
    /// low-water ratio of the given limit.
    fn evict_cold_chunks_for_pressure(
        &self,
        limit: u64,
        high_ratio: f64,
        low_ratio: f64,
    ) -> StorageResult<()> {
        // Eviction requires disk persistence; in-memory-only mode cannot recover evicted chunks
        if self.index_root.is_none() {
            return Ok(());
        }
        let high = (limit as f64 * high_ratio) as u64;
        let low = (limit as f64 * low_ratio) as u64;
        let runtimes = self.runtimes.read();
        // Evict from the runtime with the highest memory usage first
        let mut candidates: Vec<(IndexIdentity, u64)> = runtimes
            .iter()
            .map(|(id, rt)| (*id, rt.memory_usage_bytes()))
            .collect();
        drop(runtimes);
        candidates.sort_by_key(|b| std::cmp::Reverse(b.1));

        for (identity, _mem) in candidates {
            if self.memory_usage_bytes() <= low {
                break;
            }
            let runtimes = self.runtimes.read();
            let runtime = match runtimes.get(&identity) {
                Some(rt) => Arc::clone(rt),
                None => continue,
            };
            drop(runtimes);
            runtime.evict_cold_chunks(self.memory_usage_bytes(), high, low)?;
        }
        Ok(())
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

    /// Cheap O(1) snapshot of index memory usage maintained by
    /// publish/retire/split paths via `sync_memory_usage`.
    ///
    /// Safe to call on hot paths (e.g. per-statement write admission checks)
    /// where a full traversal over all runtimes and generations would be
    /// quadratic in the number of generations.
    pub fn cached_memory_usage_bytes(&self) -> u64 {
        self.total_memory_usage.load(Ordering::Relaxed)
    }

    /// Sync cached memory counter with actual usage.
    pub(crate) fn sync_memory_usage(&self) {
        let usage = self.memory_usage_bytes();
        self.total_memory_usage.store(usage, Ordering::Relaxed);
    }

    pub fn active_entry_count(&self) -> usize {
        let mut count = 0;
        for runtime in self.runtimes.read().values() {
            for generation in runtime.generations() {
                for shard in generation.shards() {
                    count += shard
                        .read_forward()
                        .snapshot()
                        .into_values()
                        .filter(|e| e.deleted_ts.is_none())
                        .count();
                    count += shard
                        .read_reverse()
                        .snapshot()
                        .into_values()
                        .filter(|e| e.deleted_ts.is_none())
                        .count();
                }
            }
        }
        count
    }

    pub(crate) fn generation_checkpoint_path(
        &self,
        space_id: u64,
        index_id: u64,
        generation: IndexGeneration,
        shard_id: u32,
    ) -> PathBuf {
        let relative = PathBuf::from(format!(
            "{space_id}/{index_id}/generation-{}/shard-{shard_id}",
            generation.get()
        ));
        match self.index_root.as_ref() {
            Some(root) => root.join(relative),
            None => PathBuf::from("memory-index").join(relative),
        }
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

    fn restore_active_generation(&self, identity: IndexIdentity) -> StorageResult<()> {
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

    /// Compute key prefixes for the given index identity.
    /// Forward prefix: space_id(8) + key_type(1) + name_len(4) + name
    /// Reverse prefix: space_id(8) + key_type(1)
    fn compute_prefixes(&self, identity: IndexIdentity) -> (Vec<u8>, Vec<u8>) {
        let index_type = self.index_types.read().get(&identity).cloned();
        let index_def = self.index_definitions.read().get(&identity).cloned();
        match (index_type, index_def.as_ref()) {
            (Some(IndexType::TagIndex), Some(def)) => {
                let fwd = KeyBuilder::build_vertex_index_prefix(identity.space_id, &def.name).0;
                let mut rev = Vec::with_capacity(9);
                rev.extend_from_slice(&identity.space_id.to_le_bytes());
                rev.push(KEY_TYPE_VERTEX_REVERSE);
                (fwd, rev)
            }
            (Some(IndexType::EdgeIndex), Some(def)) => {
                let fwd = KeyBuilder::build_edge_index_prefix(identity.space_id, &def.name).0;
                let mut rev = Vec::with_capacity(9);
                rev.extend_from_slice(&identity.space_id.to_le_bytes());
                rev.push(KEY_TYPE_EDGE_REVERSE);
                (fwd, rev)
            }
            _ => (Vec::new(), Vec::new()),
        }
    }

    /// Accumulate a delta into the pending buffer, publishing a new generation
    /// once the entry threshold is reached.
    ///
    /// When `delta_publish_threshold <= 1` the delta is published immediately,
    /// preserving the legacy per-statement publish behavior (rollback path).
    pub(crate) fn accumulate_delta(
        &self,
        identity: IndexIdentity,
        delta: HashMap<u32, IndexMaps>,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let threshold = self.delta_publish_threshold.load(Ordering::Relaxed).max(1);
        if threshold <= 1 {
            let tombstones = delta
                .values()
                .flat_map(|(forward, reverse)| forward.values().chain(reverse.values()))
                .filter(|record| record.deleted_ts.is_some())
                .count() as u64;
            if tombstones > 0 {
                self.cached_tombstone_count
                    .fetch_add(tombstones, Ordering::Relaxed);
            }
            return self.publish_delta_generation(identity, delta, write_ts);
        }

        let mut pending = self.pending_deltas.lock();
        let entry = pending.entry(identity).or_default();
        let mut added = 0usize;
        // Merge a (key, record) pair into a pending map, keeping the tombstone
        // counter accurate: a NEW tombstone increments it, and overwriting a
        // pending tombstone with a live entry decrements it (the tombstoned record
        // never reaches a generation, so it must not stay counted).
        let merge_record = |map: &mut BTreeMap<Vec<u8>, IndexRecord>,
                            key: Vec<u8>,
                            record: IndexRecord,
                            counter: &std::sync::atomic::AtomicU64| {
            let record_is_tombstone = record.deleted_ts.is_some();
            match map.insert(key, record) {
                Some(old) => match (old.deleted_ts.is_some(), record_is_tombstone) {
                    (false, true) => {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    (true, false) => {
                        let _ =
                            counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                                Some(count.saturating_sub(1))
                            });
                    }
                    _ => {}
                },
                None => {
                    if record_is_tombstone {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        };
        for (shard_id, (forward, reverse)) in delta {
            let (pending_fwd, pending_rev) = entry.per_shard.entry(shard_id).or_default();
            added += forward.len() + reverse.len();
            for (key, record) in forward {
                merge_record(pending_fwd, key, record, &self.cached_tombstone_count);
            }
            for (key, record) in reverse {
                merge_record(pending_rev, key, record, &self.cached_tombstone_count);
            }
        }
        entry.entries += added;
        entry.write_ts = entry.write_ts.max(write_ts);

        if entry.entries >= threshold {
            let to_publish = pending.remove(&identity).expect("entry exists");
            drop(pending);
            return self.publish_delta_generation(
                identity,
                to_publish.per_shard,
                to_publish.write_ts,
            );
        }
        Ok(())
    }

    /// Publish any pending delta for `identity` as a new generation, making it
    /// visible through the normal generation-chain read path. Reads call this
    /// first so they observe all committed writes.
    pub(crate) fn publish_pending_delta(&self, identity: IndexIdentity) -> StorageResult<()> {
        let to_publish = {
            let mut pending = self.pending_deltas.lock();
            let Some(entry) = pending.remove(&identity) else {
                return Ok(());
            };
            if entry.per_shard.is_empty() {
                return Ok(());
            }
            Some((entry.per_shard, entry.write_ts))
        };
        if let Some((per_shard, write_ts)) = to_publish {
            self.publish_delta_generation(identity, per_shard, write_ts)?;
        }
        Ok(())
    }

    /// Configure the delta-publish threshold (entries per generation).
    pub fn set_delta_publish_threshold(&self, threshold: usize) {
        self.delta_publish_threshold
            .store(threshold.max(1), Ordering::Relaxed);
    }

    /// Number of entries pending publication for `identity`.
    #[cfg(test)]
    pub(crate) fn pending_delta_entries(&self, identity: IndexIdentity) -> usize {
        self.pending_deltas
            .lock()
            .get(&identity)
            .map(|entry| entry.entries)
            .unwrap_or(0)
    }

    /// Publish a delta generation — a new generation that contains only changed
    /// (inserted/updated) entries. The new generation inherits all unchanged
    /// entries from its parent via the generation chain fallback read path.
    ///
    /// Each entry in `delta` is inserted into an otherwise-empty generation.
    /// The read path checks the newest generation first, then falls back to
    /// the parent generation for entries not found in the delta.
    pub(crate) fn publish_delta_generation(
        &self,
        identity: IndexIdentity,
        delta: HashMap<u32, IndexMaps>,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let catalog = self
            .manifest_catalog(identity.space_id, identity.index_id)
            .ok_or_else(|| {
                StorageError::not_found(format!("Index {} has no manifest", identity.index_id))
            })?;
        let runtime = self.runtime(identity.space_id, identity.index_id)?;
        let current = catalog.acquire().manifest().clone();
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

        let current_gen = runtime.generation(current.generation);

        // Compute key prefixes for memory deduplication of the fixed key portion
        let (forward_prefix, reverse_prefix) = self.compute_prefixes(identity);
        let generation = GenerationRuntime::empty_with_maps(
            &next_manifest,
            forward_prefix,
            reverse_prefix,
            delta,
            current_gen.as_ref(),
            write_ts,
        );

        let active_gen = catalog.acquire().manifest().generation;
        if active_gen != current.generation {
            return Err(StorageError::invalid_operation(
                "Index generation changed while publishing delta; retry",
            ));
        }

        // Fold the freshly installed generation's own footprint into the cached
        // memory counter. The publish path runs once per statement per index in
        // auto-commit bulk loads, so a full traversal here would be quadratic in
        // the total number of generations; a full resync is deferred to the rare
        // retirement/eviction/compaction paths which already call sync_memory_usage.
        let generation_bytes = generation.memory_usage_bytes();
        runtime.install_generation(generation);
        catalog.publish(next_manifest)?;
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        self.record_manifest_state(&catalog);
        self.total_memory_usage
            .fetch_add(generation_bytes, Ordering::Relaxed);
        // Check memory limit and trigger compaction if needed
        let _ = self.check_memory_limit();
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
    fn pin_chain_manifests(
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
    fn compact_native_index_impl(
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

/// Remove `path` only when it is an empty directory (e.g. a generation
/// directory left behind after its shard checkpoints were reclaimed).
fn remove_dir_if_empty(path: &Path) {
    if path.is_dir() && std::fs::read_dir(path).is_ok_and(|mut it| it.next().is_none()) {
        let _ = std::fs::remove_dir(path);
    }
}

impl Default for IndexDataManagerImpl {
    fn default() -> Self {
        Self::new()
    }
}

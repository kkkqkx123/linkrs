use crate::index::manifest::ManifestCatalog;
use crate::index::shard_runtime::IndexRuntime;
use crate::index::types::IndexIdentity;
use graphdb_core::types::IndexGeneration;
use graphdb_core::StorageResult;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    /// Cheap O(1) read of the cached tombstone count. See
    /// [`cached_tombstone_count`](Self::cached_tombstone_count).
    pub fn cached_tombstone_count(&self) -> u64 {
        self.cached_tombstone_count.load(Ordering::Relaxed)
    }

    /// Resync the cached tombstone count from a full scan. Called by the rare
    /// GC/retirement/compaction paths where generations are physically removed.
    pub(crate) fn resync_tombstone_count(&self) {
        let count = self.full_tombstone_count();
        self.cached_tombstone_count
            .store(count as u64, Ordering::Relaxed);
    }

    /// Count tombstoned entries across all runtimes, generations and shards.
    pub(crate) fn full_tombstone_count(&self) -> usize {
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

    pub(crate) fn eviction_enabled(&self) -> bool {
        self.eviction_enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn eviction_high_ratio(&self) -> f64 {
        self.eviction_high_ratio.load(Ordering::Relaxed) as f64 / 10000.0
    }

    pub(crate) fn eviction_low_ratio(&self) -> f64 {
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
    pub(crate) fn evict_cold_chunks_for_pressure(
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
}

use crate::index::key_codec::key_types::{KEY_TYPE_EDGE_REVERSE, KEY_TYPE_VERTEX_REVERSE};
use crate::index::key_codec::KeyBuilder;
use crate::index::manifest::{IndexManifest, IndexShard};
use crate::index::shard_runtime::{GenerationRuntime, IndexMaps};
use crate::index::types::IndexIdentity;
use crate::index::types::IndexRecord;
use graphdb_core::types::{IndexGeneration, IndexType, Timestamp};
use graphdb_core::{StorageError, StorageResult};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;

use super::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    /// Compute key prefixes for the given index identity.
    /// Forward prefix: space_id(8) + key_type(1) + name_len(4) + name
    /// Reverse prefix: space_id(8) + key_type(1)
    pub(crate) fn compute_prefixes(&self, identity: IndexIdentity) -> (Vec<u8>, Vec<u8>) {
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
}

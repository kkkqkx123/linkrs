use crate::core::types::Index;
use crate::core::{StorageError, StorageResult};
use crate::cursor::IndexScanPlan;
use crate::index::edge_index_manager::{compute_edge_index_scan_range, EdgeIndexCursor};
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::manifest::{ManifestCatalog, ManifestHandle};
use crate::index::shard_runtime::GenerationRuntime;
use crate::index::types::IndexRecord;
use crate::index::types::StaleChecker;
use crate::index::vertex_index_manager::{
    compute_vertex_index_scan_range, VertexIndexCursor,
};
use crate::index::IndexDataManagerImpl;
use std::collections::HashSet;
use std::sync::Arc;

/// Lazy forward iterator over a generation chain for a single shard.
///
/// Loads entries from one generation at a time (newest first), tracking
/// seen and tombstoned keys to properly handle the chain semantics without
/// pre‑merging the entire data set.  Entries are yielded in gen‑by‑gen order
/// (all visible keys from gen N, then filler keys from gen N‑1, …), so the
/// cursor must not assume global key ordering across generations.
pub(crate) struct ChainForwardIterator {
    chain: Vec<Arc<GenerationRuntime>>,
    shard_id: u32,
    read_ts: u64,
    range_start: Vec<u8>,
    range_end: Vec<u8>,
    gen_idx: usize,
    pos: usize,
    current_entries: Vec<(SecondaryIndexKey, IndexRecord)>,
    seen: HashSet<SecondaryIndexKey>,
    tombstoned: HashSet<SecondaryIndexKey>,
}

impl ChainForwardIterator {
    pub(crate) fn new(
        chain: Vec<Arc<GenerationRuntime>>,
        shard_id: u32,
        read_ts: u64,
        range_start: Vec<u8>,
        range_end: Vec<u8>,
    ) -> Self {
        Self {
            chain,
            shard_id,
            read_ts,
            range_start,
            range_end,
            gen_idx: 0,
            pos: 0,
            current_entries: Vec::new(),
            seen: HashSet::new(),
            tombstoned: HashSet::new(),
        }
    }

    fn load_next_gen(&mut self) {
        while self.gen_idx < self.chain.len() {
            if let Some(shard) = self.chain[self.gen_idx].shard(self.shard_id) {
                if shard.forward_may_have_range(&self.range_start, &self.range_end) {
                    self.current_entries = shard
                        .forward_range(&self.range_start, &self.range_end)
                        .collect();
                }
                self.gen_idx += 1;
                if !self.current_entries.is_empty() {
                    self.pos = 0;
                    return;
                }
            } else {
                self.gen_idx += 1;
            }
        }
        self.current_entries = Vec::new();
    }
}

impl Iterator for ChainForwardIterator {
    type Item = (SecondaryIndexKey, IndexRecord);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.pos >= self.current_entries.len() {
                self.load_next_gen();
                if self.current_entries.is_empty() {
                    return None;
                }
            }

            let (key, entry) = &self.current_entries[self.pos];
            self.pos += 1;

            let key = key.clone();
            let entry = entry.clone();

            if entry.created_ts > self.read_ts {
                continue;
            }
            if entry.deleted_ts.is_some_and(|d| d <= self.read_ts) {
                self.tombstoned.insert(key);
                continue;
            }
            if self.tombstoned.contains(&key) {
                continue;
            }
            if !self.seen.insert(key.clone()) {
                continue;
            }

            return Some((key, entry));
        }
    }
}

impl IndexDataManagerImpl {
    #[cfg(test)]
    pub fn open_edge_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<EdgeIndexCursor> {
        self.open_edge_index_cursor_full(space_id, index, plan, None, None)
    }

    pub fn open_tag_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<StaleChecker>,
        catalog: Option<&ManifestCatalog>,
    ) -> StorageResult<VertexIndexCursor> {
        let identity = crate::index::types::IndexIdentity {
            space_id,
            index_id: plan.index_id,
        };
        // make any pending (unpublished) writes visible to this scan.
        self.publish_pending_delta(identity)?;
        let owned_catalog = if catalog.is_none() {
            self.register_native_index(space_id, index)?;
            self.manifest_catalog(space_id, plan.index_id)
        } else {
            None
        };
        let catalog = catalog
            .or(owned_catalog.as_deref())
            .ok_or_else(|| StorageError::not_found("Index manifest is unavailable"))?;
        let runtime = self.runtime(space_id, plan.index_id)?;
        let handle = catalog.acquire();
        let manifest = handle.manifest();
        let chain = runtime.generation_chain_until(manifest.generation)?;
        let chain_handles = pin_chain_manifests(catalog, &chain);

        let (start, end) = compute_vertex_index_scan_range(space_id, index, plan)?;
        let shard_iterators: Vec<ChainForwardIterator> = manifest
            .scan_ranges_with_shard(&plan.partition, &start, &end)
            .into_iter()
            .filter_map(|(shard_id, lower, upper)| {
                // Fast check: skip if no data exists for this shard across any generation
                let has_data = chain.iter().any(|gen| gen.shard(shard_id).is_some());
                has_data.then(|| {
                    ChainForwardIterator::new(
                        chain.clone(),
                        shard_id,
                        plan.read_timestamp,
                        lower,
                        upper,
                    )
                })
            })
            .collect();

        Ok(VertexIndexCursor::new(
            shard_iterators,
            plan,
            stale_checker,
            chain_handles,
        ))
    }

    pub fn open_edge_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<StaleChecker>,
        catalog: Option<&ManifestCatalog>,
    ) -> StorageResult<EdgeIndexCursor> {
        let identity = crate::index::types::IndexIdentity {
            space_id,
            index_id: plan.index_id,
        };
        // make any pending (unpublished) writes visible to this scan.
        self.publish_pending_delta(identity)?;
        let owned_catalog = if catalog.is_none() {
            self.register_native_index(space_id, index)?;
            self.manifest_catalog(space_id, plan.index_id)
        } else {
            None
        };
        let catalog = catalog
            .or(owned_catalog.as_deref())
            .ok_or_else(|| StorageError::not_found("Index manifest is unavailable"))?;
        let runtime = self.runtime(space_id, plan.index_id)?;
        let handle = catalog.acquire();
        let manifest = handle.manifest();
        let chain = runtime.generation_chain_until(manifest.generation)?;
        let chain_handles = pin_chain_manifests(catalog, &chain);

        let (start, end) = compute_edge_index_scan_range(space_id, index, plan)?;
        let shard_iterators: Vec<ChainForwardIterator> = manifest
            .scan_ranges_with_shard(&plan.partition, &start, &end)
            .into_iter()
            .filter_map(|(shard_id, lower, upper)| {
                let has_data = chain.iter().any(|gen| gen.shard(shard_id).is_some());
                has_data.then(|| {
                    ChainForwardIterator::new(
                        chain.clone(),
                        shard_id,
                        plan.read_timestamp,
                        lower,
                        upper,
                    )
                })
            })
            .collect();

        Ok(EdgeIndexCursor::new(
            shard_iterators,
            plan,
            stale_checker,
            chain_handles,
        ))
    }
}

/// Acquire a manifest pin for every generation in a runtime chain (newest
/// first). Holding these handles fences each chain generation's physical files
/// from reclamation for as long as the cursor is alive, so a cursor can never
/// outlive the checkpoint data it may lazily reload after chunk eviction.
fn pin_chain_manifests(
    catalog: &ManifestCatalog,
    chain: &[Arc<GenerationRuntime>],
) -> Vec<ManifestHandle> {
    chain
        .iter()
        .filter_map(|gen| catalog.acquire_generation(gen.generation))
        .collect()
}

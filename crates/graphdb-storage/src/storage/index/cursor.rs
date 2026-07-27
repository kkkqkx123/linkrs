use crate::core::types::Index;
use crate::core::{StorageError, StorageResult};
use crate::storage::cursor::IndexScanPlan;
use crate::storage::index::edge_index_manager::{compute_edge_index_scan_range, EdgeIndexCursor};
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::manifest::ManifestCatalog;
use crate::storage::index::shard_runtime::GenerationRuntime;
use crate::storage::index::types::IndexRecord;
use crate::storage::index::types::StaleChecker;
use crate::storage::index::vertex_index_manager::{compute_vertex_index_scan_range, VertexIndexCursor};
use crate::storage::index::IndexDataManagerImpl;
use std::collections::BTreeMap;
use std::sync::Arc;

impl IndexDataManagerImpl {
    #[cfg(test)]
    pub fn open_tag_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<VertexIndexCursor> {
        self.open_tag_index_cursor_full(space_id, index, plan, None, None)
    }

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
        let _fence = runtime.read_fence();
        let handle = catalog.acquire();
        let manifest = handle.manifest();
        let chain = runtime.generation_chain_until(manifest.generation)?;

        let (start, end) = compute_vertex_index_scan_range(space_id, index, plan)?;
        let shard_ranges: Vec<(Arc<BTreeMap<SecondaryIndexKey, IndexRecord>>, Vec<u8>, Vec<u8>)> =
            manifest
                .scan_ranges_with_shard(&plan.partition, &start, &end)
                .into_iter()
                .filter_map(|(shard_id, lower, upper)| {
                    merge_gen_chain_forward(&chain, shard_id, plan.read_timestamp)
                        .map(|merged| (merged, lower, upper))
                })
                .collect();

        Ok(VertexIndexCursor::new(
            shard_ranges,
            plan,
            stale_checker,
            Some(handle),
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
        let _fence = runtime.read_fence();
        let handle = catalog.acquire();
        let manifest = handle.manifest();
        let chain = runtime.generation_chain_until(manifest.generation)?;

        let (start, end) = compute_edge_index_scan_range(space_id, index, plan)?;
        let shard_ranges: Vec<(Arc<BTreeMap<SecondaryIndexKey, IndexRecord>>, Vec<u8>, Vec<u8>)> =
            manifest
                .scan_ranges_with_shard(&plan.partition, &start, &end)
                .into_iter()
                .filter_map(|(shard_id, lower, upper)| {
                    merge_gen_chain_forward(&chain, shard_id, plan.read_timestamp)
                        .map(|merged| (merged, lower, upper))
                })
                .collect();

        Ok(EdgeIndexCursor::new(
            shard_ranges,
            plan,
            stale_checker,
            Some(handle),
        ))
    }
}

fn merge_gen_chain_forward(
    chain: &[Arc<GenerationRuntime>],
    shard_id: u32,
    read_ts: u64,
) -> Option<Arc<BTreeMap<SecondaryIndexKey, IndexRecord>>> {
    let mut merged = BTreeMap::new();
    let mut tombstoned = std::collections::HashSet::new();
    for gen in chain {
        let Some(shard) = gen.shard(shard_id) else { continue };
        for (key, entry) in shard.iter_forward() {
            if tombstoned.contains(&key) {
                continue;
            }
            if entry.created_ts > read_ts {
                continue;
            }
            if entry.deleted_ts.is_some_and(|d| d <= read_ts) {
                tombstoned.insert(key);
                continue;
            }
            merged.entry(key).or_insert(entry);
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(Arc::new(merged))
    }
}

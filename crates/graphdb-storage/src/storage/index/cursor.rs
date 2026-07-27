use crate::core::types::Index;
use crate::core::{StorageError, StorageResult};
use crate::storage::cursor::IndexScanPlan;
use crate::storage::index::edge_index_manager::{compute_edge_index_scan_range, EdgeIndexCursor};
use crate::storage::index::manifest::ManifestCatalog;
use crate::storage::index::shard_runtime::ShardRuntime;
use crate::storage::index::types::StaleChecker;
use crate::storage::index::vertex_index_manager::{compute_vertex_index_scan_range, VertexIndexCursor};
use crate::storage::index::IndexDataManagerImpl;
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
        let generation = runtime
            .generation(manifest.generation)
            .ok_or_else(|| StorageError::not_found("Index runtime generation is unavailable"))?;

        let (start, end) = compute_vertex_index_scan_range(space_id, index, plan)?;
        let shard_ranges: Vec<(Arc<ShardRuntime>, Vec<u8>, Vec<u8>)> = manifest
            .scan_ranges_with_shard(&plan.partition, &start, &end)
            .into_iter()
            .filter_map(|(shard_id, lower, upper)| {
                let shard = generation.shard(shard_id)?;
                Some((shard, lower, upper))
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
        let generation = runtime
            .generation(manifest.generation)
            .ok_or_else(|| StorageError::not_found("Index runtime generation is unavailable"))?;

        let (start, end) = compute_edge_index_scan_range(space_id, index, plan)?;
        let shard_ranges: Vec<(Arc<ShardRuntime>, Vec<u8>, Vec<u8>)> = manifest
            .scan_ranges_with_shard(&plan.partition, &start, &end)
            .into_iter()
            .filter_map(|(shard_id, lower, upper)| {
                let shard = generation.shard(shard_id)?;
                Some((shard, lower, upper))
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

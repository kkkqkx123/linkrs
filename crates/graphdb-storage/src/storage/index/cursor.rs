use crate::core::types::Index;
use crate::core::{StorageError, StorageResult};
use crate::storage::cursor::IndexScanPlan;
use crate::storage::index::edge_index_manager::EdgeIndexCursor;
use crate::storage::index::edge_index_manager::EdgeIndexManager;
use crate::storage::index::manifest::ManifestCatalog;
use crate::storage::index::types::StaleChecker;
use crate::storage::index::vertex_index_manager::VertexIndexCursor;
use crate::storage::index::vertex_index_manager::VertexIndexManager;
use crate::storage::index::IndexDataManagerImpl;
use std::collections::BTreeMap;

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
        let generation = runtime
            .generation(handle.manifest().generation)
            .ok_or_else(|| StorageError::not_found("Index runtime generation is unavailable"))?;
        let temporary = VertexIndexManager::new();
        let mut forward = BTreeMap::new();
        for shard in generation.shards() {
            let fwd = shard.forward().read().clone();
            forward.extend(fwd);
        }
        temporary.base().replace_data(forward, BTreeMap::new());
        let mut cursor = temporary.open_tag_index_cursor_full(
            space_id,
            index,
            plan,
            stale_checker,
            Some(catalog),
        )?;
        cursor.set_manifest_handle(handle);
        Ok(cursor)
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
        let generation = runtime
            .generation(handle.manifest().generation)
            .ok_or_else(|| StorageError::not_found("Index runtime generation is unavailable"))?;
        let temporary = EdgeIndexManager::new();
        let mut forward = BTreeMap::new();
        for shard in generation.shards() {
            forward.extend(shard.forward().read().clone());
        }
        temporary.base().replace_data(forward, BTreeMap::new());
        let mut cursor = temporary.open_edge_index_cursor_full(
            space_id,
            index,
            plan,
            stale_checker,
            Some(catalog),
        )?;
        cursor.set_manifest_handle(handle);
        Ok(cursor)
    }
}

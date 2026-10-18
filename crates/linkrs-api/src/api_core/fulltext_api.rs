//! Fulltext Index API – Core Layer
//!
//! Transport-layer independent fulltext index management and search operations.
//! Mirrors `VectorApi` so the two secondary-index backends share one
//! management shape.

use crate::api_core::error::{CoreError, CoreResult};
use linkrs_core::SearchIndexMetadata;
use linkrs_fulltext::manager::FulltextIndexManager;
use linkrs_fulltext::query::FulltextQuery;
use linkrs_fulltext::{EngineType, IndexMetadata, SearchResult};
use linkrs_sync::fulltext_sync::FulltextSyncCoordinator;
use linkrs_sync::{FulltextRebuildOptions, RebuildDocSource, SyncManager};
use std::sync::Arc;

/// Fulltext Index API – Core Layer
pub struct FulltextApi {
    manager: Arc<FulltextIndexManager>,
    coordinator: Arc<FulltextSyncCoordinator>,
    sync_manager: Arc<SyncManager>,
}

impl FulltextApi {
    /// Create a FulltextApi with the full assembly the fulltext subsystem needs.
    pub fn new(
        manager: Arc<FulltextIndexManager>,
        coordinator: Arc<FulltextSyncCoordinator>,
        sync_manager: Arc<SyncManager>,
    ) -> Self {
        Self {
            manager,
            coordinator,
            sync_manager,
        }
    }

    /// Get the fulltext index manager.
    pub fn manager(&self) -> &Arc<FulltextIndexManager> {
        &self.manager
    }

    /// Get the sync coordinator.
    pub fn coordinator(&self) -> &Arc<FulltextSyncCoordinator> {
        &self.coordinator
    }

    /// Create a fulltext index.
    pub async fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        engine_type: Option<EngineType>,
    ) -> CoreResult<String> {
        self.manager
            .create_index(space_id, tag_name, field_name, engine_type)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }

    /// Create a fulltext index with explicit engine configuration.
    pub async fn create_index_with_engine_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        user_index_name: &str,
        engine_type: Option<EngineType>,
        engine_config: Option<serde_json::Value>,
    ) -> CoreResult<String> {
        self.manager
            .create_index_with_engine_config(
                space_id,
                tag_name,
                field_name,
                user_index_name,
                engine_type,
                engine_config,
            )
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }

    /// Drop a fulltext index.
    pub async fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<()> {
        self.manager
            .drop_index(space_id, tag_name, field_name)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }

    /// Get fulltext index info.
    pub fn get_index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<Option<IndexMetadata>> {
        Ok(self.manager.get_metadata(space_id, tag_name, field_name))
    }

    /// List all fulltext indexes.
    pub fn list_indexes(&self) -> Vec<String> {
        self.manager
            .list_indexes()
            .into_iter()
            .map(|m| m.index_id)
            .collect()
    }

    /// List fulltext indexes as the common search-index descriptor.
    pub fn list_index_descriptors(&self) -> Vec<SearchIndexMetadata> {
        self.manager
            .list_indexes()
            .into_iter()
            .map(|m| SearchIndexMetadata {
                space_id: m.space_id,
                tag_name: m.tag_name,
                field_name: m.field_name,
                index_name: Some(m.index_name),
                status: m.status.to_string(),
                doc_count: m.doc_count,
                engine: m.engine_type.to_string(),
                last_updated: Some(m.last_updated),
            })
            .collect()
    }

    /// List fulltext indexes needing operator attention.
    pub fn inconsistent_indexes(&self) -> Vec<IndexMetadata> {
        self.manager.get_inconsistent_indexes()
    }

    /// Search fulltext.
    pub async fn search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> CoreResult<Vec<SearchResult>> {
        self.manager
            .search(space_id, tag_name, field_name, query, limit)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }

    /// Structured fulltext search.
    pub async fn search_structured(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &FulltextQuery,
        limit: usize,
    ) -> CoreResult<Vec<SearchResult>> {
        self.manager
            .search_structured(space_id, tag_name, field_name, query, limit)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }

    /// Get index stats.
    pub async fn get_stats(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<linkrs_fulltext::IndexStats> {
        self.manager
            .get_stats(space_id, tag_name, field_name)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }

    /// Clear a fulltext index.
    pub async fn clear_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> CoreResult<()> {
        self.manager
            .clear_index(space_id, tag_name, field_name, force)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }

    /// Rebuild a fulltext index from primary storage.
    pub async fn rebuild_collection(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        source: &mut dyn RebuildDocSource,
        options: FulltextRebuildOptions,
    ) -> CoreResult<u64> {
        self.sync_manager
            .rebuild_fulltext_index(space_id, tag_name, field_name, source, options)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))
    }
}

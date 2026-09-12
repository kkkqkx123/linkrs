//! Fulltext Index API – Core Layer
//!
//! Provides transport layer independent fulltext index management and search operations.
//! Mirrors the `VectorApi` pattern for consistency across search engines.

use graphdb_fulltext::manager::FulltextIndexManager;
use graphdb_fulltext::{IndexMetadata, SearchResult};
use std::sync::Arc;

/// Fulltext search result with optional highlights
#[derive(Debug, Clone)]
pub struct FulltextSearchResult {
    pub doc_id: String,
    pub score: f32,
    pub highlights: Option<Vec<HighlightInfo>>,
}

/// Highlight information for a matched field
#[derive(Debug, Clone)]
pub struct HighlightInfo {
    pub field: String,
    pub fragments: Vec<String>,
}

/// Fulltext Index API – Core Layer
///
/// Provides a clean abstraction layer for fulltext index management and search,
/// mirroring the `VectorApi` pattern. This enables both embedded and network
/// service layers to access fulltext operations through a unified interface.
pub struct FulltextApi {
    manager: Arc<FulltextIndexManager>,
    #[cfg(feature = "fulltext")]
    sync_manager: Option<Arc<graphdb_sync::SyncManager>>,
}

impl FulltextApi {
    /// Create a new FulltextApi instance
    pub fn new(manager: Arc<FulltextIndexManager>) -> Self {
        Self {
            manager,
            #[cfg(feature = "fulltext")]
            sync_manager: None,
        }
    }

    /// Attach a sync manager after construction (enables online rebuild).
    #[cfg(feature = "fulltext")]
    pub fn with_sync_manager(mut self, sync_manager: Arc<graphdb_sync::SyncManager>) -> Self {
        self.sync_manager = Some(sync_manager);
        self
    }

    /// Get the underlying fulltext index manager
    pub fn manager(&self) -> &Arc<FulltextIndexManager> {
        &self.manager
    }

    /// Create a fulltext index for a specific field
    ///
    /// # Arguments
    /// * `space_id` - The space (namespace) ID
    /// * `tag_name` - The tag (vertex type) name
    /// * `field_name` - The field name to index
    ///
    /// # Returns
    /// The created index metadata
    pub async fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<String, String> {
        self.manager
            .create_index(space_id, tag_name, field_name, None)
            .await
            .map_err(|e| e.to_string())
    }

    /// Drop a fulltext index
    ///
    /// # Arguments
    /// * `space_id` - The space (namespace) ID
    /// * `tag_name` - The tag (vertex type) name
    /// * `field_name` - The field name
    pub async fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), String> {
        self.manager
            .drop_index(space_id, tag_name, field_name)
            .await
            .map_err(|e| e.to_string())
    }

    /// List all fulltext indexes
    pub fn list_indexes(&self) -> Vec<IndexMetadata> {
        self.manager.list_indexes()
    }

    /// Search a fulltext index
    ///
    /// # Arguments
    /// * `space_id` - The space (namespace) ID
    /// * `tag_name` - The tag (vertex type) name
    /// * `field_name` - The field name
    /// * `query` - The search query string
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    /// Vector of search results with doc_id and score
    pub async fn search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, String> {
        self.manager
            .search(space_id, tag_name, field_name, query, limit)
            .await
            .map_err(|e| e.to_string())
    }

    /// Clear a fulltext index: drop all indexed documents without backfill.
    ///
    /// This is an explicit destructive operation, not a rebuild. The caller
    /// must pass `force = true` to confirm the data loss; the call is audit
    /// logged. To repair an inconsistent index without data loss, use the
    /// sync-side online rebuild driver (or drop and recreate the index).
    ///
    /// # Arguments
    /// * `space_id` - The space (namespace) ID
    /// * `tag_name` - The tag (vertex type) name
    /// * `field_name` - The field name
    /// * `force` - Explicit confirmation of the destructive clear
    pub async fn clear_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> Result<(), String> {
        if !force {
            return Err(
                "Refusing to clear fulltext index without explicit confirmation: retry with force = true"
                    .to_string(),
            );
        }
        log::warn!(
            "Clearing fulltext index explicitly confirmed: space {} tag {} field {}",
            space_id,
            tag_name,
            field_name
        );
        self.manager
            .clear_index(space_id, tag_name, field_name, force)
            .await
            .map_err(|e| e.to_string())
    }

    /// List fulltext indexes whose engine is in the Inconsistent state.
    ///
    /// These indexes reject writes and need operator attention: run the
    /// sync-side online rebuild driver (or drop and recreate the index).
    pub fn inconsistent_indexes(&self) -> Vec<IndexMetadata> {
        self.manager.get_inconsistent_indexes()
    }

    /// Rebuild a fulltext index from primary storage without dropping it.
    ///
    /// Symmetric with [`crate::api_core::vector_api::VectorApi::rebuild_collection`]:
    /// the live engine keeps serving reads while `source` backfills a scratch
    /// engine, outbox replay converges it, and publish swaps it in. Requires
    /// a sync manager attached via [`FulltextApi::with_sync_manager`].
    /// Returns the live doc count after publish.
    #[cfg(feature = "fulltext")]
    pub async fn rebuild_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        source: &mut dyn graphdb_sync::RebuildDocSource,
        options: graphdb_sync::FulltextRebuildOptions,
    ) -> Result<u64, String> {
        let Some(manager) = self.sync_manager.as_ref() else {
            return Err("Fulltext rebuild requires a configured SyncManager".to_string());
        };
        manager
            .rebuild_fulltext_index(space_id, tag_name, field_name, source, options)
            .await
            .map_err(|e| e.to_string())
    }

    /// Commit all pending index changes
    pub async fn commit_all(&self) -> Result<(), String> {
        self.manager.commit_all().await.map_err(|e| e.to_string())
    }

    /// Get index statistics
    pub fn get_stats(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<graphdb_fulltext::IndexMetadata> {
        self.manager.list_indexes().into_iter().find(|m| {
            m.space_id == space_id && m.tag_name == tag_name && m.field_name == field_name
        })
    }
}

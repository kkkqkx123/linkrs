//! Fulltext search for an embedded session.
//!
//! Mirrors the vector session methods so embedded users can reach
//! both secondary-index backends through the same `Session` surface.

use super::Session;
use crate::api_core::rebuild_source::StorageRebuildSource;
use crate::api_core::{CoreError, CoreResult, FulltextApi};
use crate::embedded::database::GraphDatabase;
use crate::embedded::session::GraphDatabaseInner;
use linkrs_core::index_events::RebuildProgress;
use linkrs_core::SearchIndexMetadata;
use linkrs_fulltext::query::FulltextQuery;
use linkrs_fulltext::{IndexMetadata, SearchResult};
use linkrs_storage::StorageClient;

#[cfg(feature = "fulltext")]
impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Search a fulltext index with a plain query string.
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - indexed field name
    /// - `query` - fulltext query string
    /// - `limit` - maximum number of results to return
    ///
    /// # Return
    /// - Returns fulltext search results on success
    /// - Return error on failure
    pub async fn search_fulltext(
        &self,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> CoreResult<Vec<SearchResult>> {
        let space_id = (*self.space_id.read())
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.fulltext_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Fulltext coordinator not available".to_string())
        })?;

        let results = coordinator
            .search(space_id, tag_name, field_name, query, limit)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))?;

        Ok(results)
    }

    /// Search a fulltext index with a structured query.
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - indexed field name
    /// - `query` - structured fulltext query
    /// - `limit` - maximum number of results to return
    ///
    /// # Return
    /// - Returns fulltext search results on success
    /// - Return error on failure
    pub async fn search_fulltext_structured(
        &self,
        tag_name: &str,
        field_name: &str,
        query: &FulltextQuery,
        limit: usize,
    ) -> CoreResult<Vec<SearchResult>> {
        let space_id = (*self.space_id.read())
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.fulltext_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Fulltext coordinator not available".to_string())
        })?;

        let results = coordinator
            .search_structured(space_id, tag_name, field_name, query, limit)
            .await
            .map_err(|e| CoreError::FulltextError(e.to_string()))?;

        Ok(results)
    }

    /// Fulltext management API for the configured subsystem.
    ///
    /// Returns `None` when fulltext support is disabled; callers must not
    /// treat that as an empty index list.
    pub fn fulltext_api(&self) -> Option<FulltextApi> {
        fulltext_api_from_inner(&self.db)
    }

    fn require_fulltext_api(&self) -> CoreResult<FulltextApi> {
        self.fulltext_api().ok_or_else(|| {
            CoreError::InvalidParameter("Fulltext subsystem is not available".to_string())
        })
    }

    /// List fulltext index ids.
    pub fn list_fulltext_indexes(&self) -> CoreResult<Vec<String>> {
        Ok(self.require_fulltext_api()?.list_indexes())
    }

    /// List fulltext indexes as common search-index descriptors.
    pub fn list_fulltext_index_descriptors(&self) -> CoreResult<Vec<SearchIndexMetadata>> {
        Ok(self.require_fulltext_api()?.list_index_descriptors())
    }

    /// Fetch metadata for one fulltext index (`None` when missing).
    pub fn get_fulltext_index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<Option<IndexMetadata>> {
        self.require_fulltext_api()?
            .get_index_info(space_id, tag_name, field_name)
    }

    /// Fulltext indexes whose engine is inconsistent.
    pub fn inconsistent_fulltext_indexes(&self) -> CoreResult<Vec<IndexMetadata>> {
        Ok(self.require_fulltext_api()?.inconsistent_indexes())
    }

    /// Live rebuild progress for one index, when a rebuild is running.
    pub fn fulltext_rebuild_status(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<Option<RebuildProgress>> {
        Ok(self
            .require_fulltext_api()?
            .manager()
            .rebuild_progress(space_id, tag_name, field_name))
    }

    /// Rebuild one fulltext index from primary storage.
    ///
    /// Returns the number of applied documents.
    pub async fn rebuild_fulltext_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        batch_size: usize,
    ) -> CoreResult<u64> {
        let api = self.require_fulltext_api()?;
        let storage = self.db.storage.read().clone();
        let space_name = storage
            .get_space_by_id(space_id)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .ok_or_else(|| CoreError::NotFound(format!("Space ID {} does not exist", space_id)))?
            .space_name;
        let mut source = StorageRebuildSource::new(
            &storage,
            space_name,
            tag_name.to_string(),
            field_name.to_string(),
            batch_size,
        );
        api.rebuild_collection(
            space_id,
            tag_name,
            field_name,
            &mut source,
            linkrs_sync::FulltextRebuildOptions::default(),
        )
        .await
    }

    /// Clear one fulltext index explicitly (`force` confirms the data loss).
    pub async fn clear_fulltext_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> CoreResult<()> {
        self.require_fulltext_api()?
            .clear_index(space_id, tag_name, field_name, force)
            .await
    }
}

fn fulltext_api_from_inner<S: StorageClient + Clone + Send + Sync + 'static>(
    db: &GraphDatabaseInner<S>,
) -> Option<FulltextApi> {
    let sync_manager = db.sync_manager.as_ref()?;
    let coordinator = sync_manager.sync_coordinator()?.clone();
    Some(FulltextApi::new(
        coordinator.fulltext_manager().clone(),
        coordinator,
        sync_manager.clone(),
    ))
}

impl<S: StorageClient + Clone + Send + Sync + 'static> GraphDatabase<S> {
    /// Fulltext management API; `None` when fulltext support is disabled.
    pub fn fulltext_api(&self) -> Option<FulltextApi> {
        fulltext_api_from_inner(&self.inner)
    }
}

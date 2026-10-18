//! Fulltext search for an embedded session.
//!
//! Mirrors the vector session methods so embedded users can reach
//! both secondary-index backends through the same `Session` surface.

use super::Session;
use crate::api_core::{CoreError, CoreResult};
use linkrs_fulltext::query::FulltextQuery;
use linkrs_fulltext::SearchResult;
use linkrs_storage::StorageClient;
use std::sync::Arc;

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
}

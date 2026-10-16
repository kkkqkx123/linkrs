//! Vector search and index management for an embedded session.
//!
//! Gated behind `feature = "vector"`: the methods only touch the sync
//! subsystem's `VectorSyncCoordinator` and never reach the statement path.

use super::Session;
use crate::api_core::VectorSearchResult;
use crate::api_core::{CoreError, CoreResult};
use linkrs_storage::StorageClient;
use linkrs_sync::vector_sync::SearchOptions;
use std::sync::Arc;

#[cfg(feature = "vector")]
impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Vector search - search for similar vectors
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    /// - `query_vector` - query vector
    /// - `limit` - maximum number of results to return
    ///
    /// # Return
    /// - Returns vector search results on success
    /// - Return error on failure
    pub async fn simvec(
        &self,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> CoreResult<Vec<VectorSearchResult>> {
        let space_id = (*self.space_id.read())
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        let options = SearchOptions::new(space_id, tag_name, field_name, query_vector, limit);
        let results = coordinator
            .search_with_options(options)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))?;

        Ok(results
            .into_iter()
            .map(|r| VectorSearchResult {
                id: r.id,
                score: r.score,
                vector: r.vector.map(|v| v.to_vec()),
                payload: r.payload.map(|p| p.into_iter().collect()),
            })
            .collect())
    }

    /// Vector search with threshold
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    /// - `query_vector` - query vector
    /// - `limit` - maximum number of results to return
    /// - `threshold` - minimum similarity threshold
    ///
    /// # Return
    /// - Returns vector search results on success
    /// - Return error on failure
    pub async fn simvec_with_threshold(
        &self,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> CoreResult<Vec<VectorSearchResult>> {
        let space_id = (*self.space_id.read())
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        let options = SearchOptions::new(space_id, tag_name, field_name, query_vector, limit)
            .with_threshold(threshold);
        let results = coordinator
            .search_with_options(options)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))?;

        Ok(results
            .into_iter()
            .map(|r| VectorSearchResult {
                id: r.id,
                score: r.score,
                vector: r.vector.map(|v| v.to_vec()),
                payload: r.payload.map(|p| p.into_iter().collect()),
            })
            .collect())
    }

    /// Create a vector index
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    /// - `vector_size` - dimension of the vector
    /// - `distance` - distance metric
    ///
    /// # Return
    /// - Returns collection name on success
    /// - Return error on failure
    pub async fn create_vector_index(
        &self,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: simvec::DistanceMetric,
    ) -> CoreResult<String> {
        let space_id = {
            let guard = self.space_id.read();
            guard
                .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?
        };

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        coordinator
            .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Drop a vector index
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    ///
    /// # Return
    /// - Returns () on success
    /// - Return error on failure
    pub async fn drop_vector_index(&self, tag_name: &str, field_name: &str) -> CoreResult<()> {
        let space_id = {
            let guard = self.space_id.read();
            guard
                .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?
        };

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        coordinator
            .drop_vector_index(space_id, tag_name, field_name)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }
}

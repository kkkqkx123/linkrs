//! Vector search and index management for an embedded session.
//!
//! Gated behind `feature = "vector"`: the methods only touch the sync
//! subsystem's `VectorSyncCoordinator` and never reach the statement path.

use super::Session;
use crate::api_core::rebuild_source::StorageVectorSource;
use crate::api_core::vector_api::ScrollQuery;
use crate::api_core::{CoreError, CoreResult, VectorApi, VectorSearchResult};
use crate::embedded::database::GraphDatabase;
use crate::embedded::session::GraphDatabaseInner;
use linkrs_core::index_events::RebuildProgress;
use linkrs_core::SearchIndexMetadata;
use linkrs_storage::StorageClient;
use linkrs_sync::vector_sync::SearchOptions;
use simvec::{IndexMetadata, Payload, VectorPoint};

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
                doc_id: r.id,
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
                doc_id: r.id,
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

    /// Vector management API for the configured subsystem.
    ///
    /// Returns `None` when vector support is disabled or the database has no
    /// usable backend; callers must not treat that as an empty index list.
    pub fn vector_api(&self) -> Option<VectorApi> {
        vector_api_from_inner(&self.db)
    }

    fn require_vector_api(&self) -> CoreResult<VectorApi> {
        self.vector_api().ok_or_else(|| {
            CoreError::InvalidParameter("Vector subsystem is not available".to_string())
        })
    }

    /// List vector index collection names.
    pub fn list_vector_indexes(&self) -> CoreResult<Vec<String>> {
        Ok(self.require_vector_api()?.list_indexes())
    }

    /// List vector indexes as common search-index descriptors.
    pub fn list_vector_index_descriptors(&self) -> CoreResult<Vec<SearchIndexMetadata>> {
        Ok(self.require_vector_api()?.list_index_descriptors())
    }

    /// Fetch metadata for one vector index (`None` when missing).
    pub fn get_vector_index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<Option<IndexMetadata>> {
        self.require_vector_api()?
            .get_index_info(space_id, tag_name, field_name)
    }

    /// Vector indexes whose engine is inconsistent.
    pub fn inconsistent_vector_indexes(&self) -> CoreResult<Vec<RebuildProgress>> {
        Ok(self.require_vector_api()?.inconsistent_indexes())
    }

    /// Live rebuild progress for one index, when a rebuild is running.
    pub fn vector_rebuild_status(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<Option<RebuildProgress>> {
        Ok(self
            .require_vector_api()?
            .coordinator()
            .rebuild_progress(space_id, tag_name, field_name))
    }

    /// Rebuild one vector index from primary storage.
    ///
    /// Returns the number of applied point operations.
    pub async fn rebuild_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        batch_size: usize,
    ) -> CoreResult<u64> {
        let api = self.require_vector_api()?;
        let storage = self.db.storage.read().clone();
        let space_name = storage
            .get_space_by_id(space_id)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .ok_or_else(|| CoreError::NotFound(format!("Space ID {} does not exist", space_id)))?
            .space_name;
        let mut source = StorageVectorSource::new(
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
            linkrs_sync::VectorRebuildOptions::default(),
        )
        .await
    }

    /// Clear one vector index explicitly (`force` confirms the data loss).
    pub async fn clear_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> CoreResult<()> {
        self.require_vector_api()?
            .clear_collection(space_id, tag_name, field_name, force)
            .await
    }

    /// Fetch one vector point.
    pub async fn get_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_id: &str,
    ) -> CoreResult<Option<VectorPoint>> {
        self.require_vector_api()?
            .get_vector(space_id, tag_name, field_name, point_id)
            .await
    }

    /// Count points in one vector index.
    pub async fn vector_count(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<u64> {
        self.require_vector_api()?
            .count(space_id, tag_name, field_name)
            .await
    }

    /// Page through vector points.
    pub async fn vector_scroll(
        &self,
        query: ScrollQuery<'_>,
    ) -> CoreResult<(Vec<VectorPoint>, Option<String>)> {
        self.require_vector_api()?.scroll(query).await
    }

    /// Replace the payload of the given points.
    pub async fn vector_set_payload(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> CoreResult<()> {
        self.require_vector_api()?
            .set_payload(space_id, tag_name, field_name, point_ids, payload)
            .await
    }

    /// Merge payload fields into the given points.
    pub async fn vector_set_payload_fields(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> CoreResult<()> {
        self.require_vector_api()?
            .set_payload_fields(space_id, tag_name, field_name, point_ids, fields)
            .await
    }

    /// Delete payload keys from the given points.
    pub async fn vector_delete_payload(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> CoreResult<()> {
        self.require_vector_api()?
            .delete_payload(space_id, tag_name, field_name, point_ids, keys)
            .await
    }
}

fn vector_api_from_inner<S: StorageClient + Clone + Send + Sync + 'static>(
    db: &GraphDatabaseInner<S>,
) -> Option<VectorApi> {
    let sync_manager = db.sync_manager.as_ref()?;
    let coordinator = sync_manager.vector_coordinator()?.clone();
    Some(VectorApi::new(
        coordinator.backend().clone(),
        coordinator,
        sync_manager.clone(),
    ))
}

impl<S: StorageClient + Clone + Send + Sync + 'static> GraphDatabase<S> {
    /// Vector management API; `None` when vector support is disabled.
    pub fn vector_api(&self) -> Option<VectorApi> {
        vector_api_from_inner(&self.inner)
    }
}

//! Vector Index API – Core Layer
//!
//! Provides transport layer independent vector index management and search operations.

use crate::api::core::error::{CoreError, CoreResult};
use crate::sync::vector_sync::{SearchOptions, VectorIndexLocation, VectorSyncCoordinator};
use std::sync::Arc;
use vector_client::manager::IndexMetadata;
use vector_client::types::PointId;
use vector_client::{
    CollectionConfig, DistanceMetric, FilterCondition, SearchQuery, VectorManager, VectorPoint,
};

/// Vector search result
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub id: PointId,
    pub score: f32,
    pub vector: Option<Vec<f32>>,
    pub payload: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Vector Index API – Core Layer
pub struct VectorApi {
    vector_manager: Arc<VectorManager>,
    coordinator: Option<Arc<VectorSyncCoordinator>>,
}

impl VectorApi {
    /// Create a new VectorApi instance
    pub fn new(vector_manager: Arc<VectorManager>) -> Self {
        Self {
            vector_manager,
            coordinator: None,
        }
    }

    /// Create a new VectorApi instance with sync coordinator
    pub fn with_coordinator(
        vector_manager: Arc<VectorManager>,
        coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        Self {
            vector_manager,
            coordinator: Some(coordinator),
        }
    }

    /// Get the vector manager
    pub fn vector_manager(&self) -> &Arc<VectorManager> {
        &self.vector_manager
    }

    /// Get the sync coordinator
    pub fn coordinator(&self) -> Option<&Arc<VectorSyncCoordinator>> {
        self.coordinator.as_ref()
    }

    /// Create a vector index
    pub async fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: DistanceMetric,
    ) -> CoreResult<String> {
        if let Some(coordinator) = &self.coordinator {
            coordinator
                .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
                .await
                .map_err(|e| CoreError::VectorError(e.to_string()))
        } else {
            let collection_name =
                VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
            let config = CollectionConfig {
                vector_size,
                distance,
                index_type: Some(vector_client::types::IndexType::HNSW),
                hnsw_config: Some(vector_client::types::HnswConfig {
                    m: 16,
                    ef_construct: 100,
                    full_scan_threshold: None,
                    max_indexing_threads: None,
                    on_disk: None,
                    payload_m: Some(16),
                }),
                ..Default::default()
            };
            self.vector_manager
                .create_index(&collection_name, config)
                .await
                .map_err(|e| CoreError::VectorError(e.to_string()))?;
            let _ = self
                .vector_manager
                .engine()
                .create_payload_index(
                    &collection_name,
                    "group_id",
                    vector_client::types::PayloadSchemaType::Keyword,
                )
                .await;
            Ok(collection_name)
        }
    }

    /// Drop a vector index
    pub async fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<()> {
        if let Some(coordinator) = &self.coordinator {
            coordinator
                .drop_vector_index(space_id, tag_name, field_name)
                .await
                .map_err(|e| CoreError::VectorError(e.to_string()))
        } else {
            let collection_name =
                VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
            self.vector_manager.unregister_index(&collection_name);
            Ok(())
        }
    }

    /// Get vector index info
    pub fn get_index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<Option<IndexMetadata>> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        Ok(self.vector_manager.get_index_metadata(&collection_name))
    }

    /// List all vector indexes
    pub fn list_indexes(&self) -> Vec<String> {
        self.vector_manager
            .list_indexes()
            .into_iter()
            .map(|info| info.name)
            .collect()
    }

    /// Insert a vector point
    pub async fn insert_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point: VectorPoint,
    ) -> CoreResult<()> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.vector_manager
            .upsert(&collection_name, point)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))?;
        Ok(())
    }

    /// Insert vector points in batch
    pub async fn insert_vector_batch(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        points: Vec<VectorPoint>,
    ) -> CoreResult<()> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.vector_manager
            .upsert_batch(&collection_name, points)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))?;
        Ok(())
    }

    /// Delete a vector point
    pub async fn delete_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_id: &str,
    ) -> CoreResult<()> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.vector_manager
            .delete(&collection_name, point_id)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Delete vector points in batch
    pub async fn delete_vector_batch(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: Vec<&str>,
    ) -> CoreResult<()> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.vector_manager
            .delete_batch(&collection_name, point_ids)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Search vectors with options
    pub async fn search_with_options(
        &self,
        options: SearchOptions,
    ) -> CoreResult<Vec<VectorSearchResult>> {
        if let Some(coordinator) = &self.coordinator {
            return coordinator
                .search_with_options(options)
                .await
                .map(|results| {
                    results
                        .into_iter()
                        .map(|r| VectorSearchResult {
                            id: r.id,
                            score: r.score,
                            vector: r.vector.map(|v| v.to_vec()),
                            payload: r.payload.map(|p| p.into_iter().collect()),
                        })
                        .collect()
                })
                .map_err(|e| CoreError::VectorError(e.to_string()));
        }

        let collection_name =
            VectorIndexLocation::new(options.space_id, &options.tag_name, &options.field_name)
                .to_collection_name();

        let mut query = SearchQuery::new(options.query_vector, options.limit);

        if let Some(threshold) = options.threshold {
            query = query.with_score_threshold(threshold);
        }

        // Inject group_id filter
        let group_id = format!("{}_{}", options.tag_name, options.field_name);
        let mut filter = options.filter.unwrap_or_default();
        filter = filter.must(FilterCondition::match_value("group_id", &group_id));
        query = query.with_filter(filter);

        let results = self
            .vector_manager
            .search(&collection_name, query)
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

    /// Get a vector point by ID
    pub async fn get_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_id: &str,
    ) -> CoreResult<Option<VectorPoint>> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.vector_manager
            .get(&collection_name, point_id)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Get vector index count
    pub async fn count(&self, space_id: u64, tag_name: &str, field_name: &str) -> CoreResult<u64> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.vector_manager
            .count(&collection_name)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }
}

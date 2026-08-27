//! Vector Index API – Core Layer
//!
//! Provides transport layer independent vector index management and search operations.

use crate::core::error::{CoreError, CoreResult};
use crate::sync::backend::VectorBackend;
use crate::sync::vector_sync::{SearchOptions, VectorIndexLocation, VectorSyncCoordinator};
use std::sync::Arc;
use vector_search::{
    types::{IndexMetadata, PointId},
    CollectionConfig, DistanceMetric, FilterCondition, SearchQuery, VectorPoint,
};

/// Parameters for paginated vector point scanning.
pub struct ScrollQuery<'a> {
    pub space_id: u64,
    pub tag_name: &'a str,
    pub field_name: &'a str,
    pub limit: usize,
    pub offset: Option<&'a str>,
    pub with_payload: Option<bool>,
    pub with_vector: Option<bool>,
}

/// Metrics every backend accepts at index-creation time; anything else is
/// rejected up front instead of failing deep inside one engine.
fn validate_metric(distance: DistanceMetric) -> CoreResult<()> {
    if matches!(
        distance,
        DistanceMetric::Cosine
            | DistanceMetric::Euclid
            | DistanceMetric::Dot
            | DistanceMetric::Manhattan
    ) {
        Ok(())
    } else {
        Err(CoreError::VectorError(format!(
            "distance metric {distance:?} is not supported; supported metrics: Cosine, Euclid, Dot, Manhattan"
        )))
    }
}

/// Vector search result
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub id: PointId,
    pub score: f32,
    pub vector: Option<Vec<f32>>,
    pub payload: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Write mode for vector point mutations.
///
/// - `Direct`: bypass the transactional outbox and write straight to the
///   backend (`VectorBackend::upsert`). Fast but not transactional: a graph
///   transaction that rolls back will not roll back the vector point.
/// - `Transactional`: stage the mutation through `SyncManager` into the durable
///   outbox (`WAL + SQLite`) so it participates in the graph transaction's
///   commit/abort and benefits from `read-your-writes` consistency.
#[derive(Debug, Clone)]
pub enum VectorWriteMode {
    Direct,
    Transactional {
        txn_id: crate::core::types::TransactionId,
        space_id: u64,
        tag: String,
        field: String,
    },
}

impl Default for VectorWriteMode {
    fn default() -> Self {
        Self::Direct
    }
}

/// Vector Index API – Core Layer
pub struct VectorApi {
    backend: VectorBackend,
    coordinator: Option<Arc<VectorSyncCoordinator>>,
    sync_manager: Option<Arc<crate::sync::SyncManager>>,
}

impl VectorApi {
    /// Create a new VectorApi instance
    pub fn new(backend: VectorBackend) -> Self {
        Self {
            backend,
            coordinator: None,
            sync_manager: None,
        }
    }

    /// Create a new VectorApi instance with sync coordinator
    pub fn with_coordinator(
        backend: VectorBackend,
        coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        Self {
            backend,
            coordinator: Some(coordinator),
            sync_manager: None,
        }
    }

    /// Create a new VectorApi with both coordinator and sync manager (for
    /// transactional vector writes).
    pub fn with_coordinator_and_sync_manager(
        backend: VectorBackend,
        coordinator: Arc<VectorSyncCoordinator>,
        sync_manager: Arc<crate::sync::SyncManager>,
    ) -> Self {
        Self {
            backend,
            coordinator: Some(coordinator),
            sync_manager: Some(sync_manager),
        }
    }

    /// Attach a sync manager after construction (for transactional writes).
    pub fn with_sync_manager(mut self, sync_manager: Arc<crate::sync::SyncManager>) -> Self {
        self.sync_manager = Some(sync_manager);
        self
    }

    /// Get the vector backend
    pub fn backend(&self) -> &VectorBackend {
        &self.backend
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
        self.create_index_with_config(
            space_id,
            tag_name,
            field_name,
            CollectionConfig::new(vector_size, distance),
        )
        .await
    }

    /// Create a vector index with full collection config (quantization/hnsw)
    pub async fn create_index_with_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        mut config: CollectionConfig,
    ) -> CoreResult<String> {
        validate_metric(config.distance)?;
        if let Some(qc) = &config.quantization_config {
            qc.validate(config.vector_size)
                .map_err(|e| CoreError::VectorError(e.to_string()))?;
        }

        if let Some(coordinator) = &self.coordinator {
            // Prefer the coordinator's full-config path when quantization/hnsw is set
            if config.quantization_config.is_some() || config.hnsw_config.is_some() {
                return coordinator
                    .create_index_with_config(space_id, tag_name, field_name, config)
                    .await
                    .map_err(|e| CoreError::VectorError(e.to_string()));
            }
            return coordinator
                .create_vector_index(
                    space_id,
                    tag_name,
                    field_name,
                    config.vector_size,
                    config.distance,
                )
                .await
                .map_err(|e| CoreError::VectorError(e.to_string()));
        } else {
            let collection_name =
                VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
            // When using the bare backend (no coordinator), ensure a sensible
            // default for remote Qdrant while letting the local engine keep
            // exact defaults unless explicitly overridden.
            if !self.backend.is_local() && config.hnsw_config.is_none() {
                config.hnsw_config = Some(vector_search::types::HnswConfig {
                    m: 16,
                    ef_construct: 100,
                    full_scan_threshold: None,
                    max_indexing_threads: None,
                    on_disk: None,
                    payload_m: Some(16),
                    ..Default::default()
                });
                config.index_type = Some(vector_search::types::IndexType::HNSW);
            }
            self.backend
                .create_index(&collection_name, &config)
                .await
                .map_err(|e| CoreError::VectorError(e.to_string()))?;
            let _ = self
                .backend
                .create_payload_index(
                    &collection_name,
                    "group_id",
                    vector_search::types::PayloadSchemaType::Keyword,
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
            self.backend
                .delete_collection(&collection_name)
                .await
                .map_err(|e| CoreError::VectorError(e.to_string()))
        }
    }

    /// Get vector index info (from logical index metadata)
    pub fn get_index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> CoreResult<Option<IndexMetadata>> {
        let Some(coordinator) = &self.coordinator else {
            return Ok(None);
        };
        Ok(coordinator.index_info(space_id, tag_name, field_name))
    }

    /// List all vector indexes
    pub fn list_indexes(&self) -> Vec<String> {
        let Some(coordinator) = &self.coordinator else {
            return Vec::new();
        };
        coordinator
            .list_indexes()
            .into_iter()
            .map(|w| w.collection_name)
            .collect()
    }

    /// Insert a vector point (Direct, non-transactional).
    ///
    /// This is the legacy bypass: the point is written straight to the backend
    /// and does not participate in graph transaction commit/rollback. Use
    /// [`Self::insert_vector_with_mode`] with `Transactional` for transactional
    /// semantics.
    pub async fn insert_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point: VectorPoint,
    ) -> CoreResult<()> {
        self.insert_vector_with_mode(space_id, tag_name, field_name, point, VectorWriteMode::Direct)
            .await
    }

    /// Insert a vector point with explicit write mode.
    pub async fn insert_vector_with_mode(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point: VectorPoint,
        mode: VectorWriteMode,
    ) -> CoreResult<()> {
        match mode {
            VectorWriteMode::Direct => {
                let collection_name =
                    VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
                self.backend
                    .upsert(&collection_name, point)
                    .await
                    .map_err(|e| CoreError::VectorError(e.to_string()))?;
                Ok(())
            }
            VectorWriteMode::Transactional {
                txn_id,
                space_id: txn_space,
                tag,
                field,
            } => {
                let Some(manager) = self.sync_manager.as_ref() else {
                    return Err(CoreError::VectorError(
                        "Transactional vector writes require a configured SyncManager".to_string(),
                    ));
                };
                // Stage as a vertex vector mutation so it flows through the
                // durable outbox (WAL + SQLite) and participates in the graph
                // transaction's commit/abort.
                let vector = point.vector.clone();
                let payload = point.payload.clone().unwrap_or_default();
                let mut properties = Vec::new();
                // Preserve existing payload fields as vertex properties; add the
                // vector field explicitly.
                for (k, v) in payload {
                    if let Ok(val) = serde_json::from_value::<crate::core::Value>(v) {
                        properties.push((k, val));
                    }
                }
                // Use the vector field as the indexed property.
                properties.push((field.clone(), crate::core::Value::vector(vector)));
                let vertex_id = crate::core::Value::string(point.id.to_string());
                manager
                    .on_vertex_change_with_txn(
                        txn_id,
                        txn_space,
                        &tag,
                        &vertex_id,
                        &properties,
                        crate::sync::types::ChangeType::Insert,
                    )
                    .map_err(|e| CoreError::VectorError(e.to_string()))?;
                Ok(())
            }
        }
    }

    /// Insert vector points in batch (Direct, non-transactional).
    pub async fn insert_vector_batch(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        points: Vec<VectorPoint>,
    ) -> CoreResult<()> {
        self.insert_vector_batch_with_mode(space_id, tag_name, field_name, points, VectorWriteMode::Direct)
            .await
    }

    /// Insert vector points in batch with explicit write mode.
    pub async fn insert_vector_batch_with_mode(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        points: Vec<VectorPoint>,
        mode: VectorWriteMode,
    ) -> CoreResult<()> {
        match mode {
            VectorWriteMode::Direct => {
                let collection_name =
                    VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
                self.backend
                    .upsert_batch(&collection_name, points)
                    .await
                    .map_err(|e| CoreError::VectorError(e.to_string()))?;
                Ok(())
            }
            VectorWriteMode::Transactional {
                txn_id,
                space_id: txn_space,
                tag,
                field,
            } => {
                let Some(manager) = self.sync_manager.as_ref() else {
                    return Err(CoreError::VectorError(
                        "Transactional vector writes require a configured SyncManager".to_string(),
                    ));
                };
                for point in points {
                    let vector = point.vector.clone();
                    let payload = point.payload.clone().unwrap_or_default();
                    let mut properties = Vec::new();
                    for (k, v) in payload {
                        if let Ok(val) = serde_json::from_value::<crate::core::Value>(v) {
                            properties.push((k, val));
                        }
                    }
                    properties.push((field.clone(), crate::core::Value::vector(vector)));
                    let vertex_id = crate::core::Value::string(point.id.to_string());
                    manager
                        .on_vertex_change_with_txn(
                            txn_id,
                            txn_space,
                            &tag,
                            &vertex_id,
                            &properties,
                            crate::sync::types::ChangeType::Insert,
                        )
                        .map_err(|e| CoreError::VectorError(e.to_string()))?;
                }
                Ok(())
            }
        }
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
        self.backend
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
        self.backend
            .delete_batch(&collection_name, &point_ids)
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
            .backend
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
        self.backend
            .get_vector(&collection_name, point_id)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Get vector index count
    pub async fn count(&self, space_id: u64, tag_name: &str, field_name: &str) -> CoreResult<u64> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.backend
            .count(&collection_name)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Replace the entire payload for the given points.
    pub async fn set_payload(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: Vec<&str>,
        payload: vector_search::types::Payload,
    ) -> CoreResult<()> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.backend
            .set_payload(&collection_name, point_ids, payload)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Merge the given fields into the payload of the given points.
    /// Only the supplied keys are updated; other existing keys are preserved.
    pub async fn set_payload_fields(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: Vec<&str>,
        fields: vector_search::types::Payload,
    ) -> CoreResult<()> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.backend
            .set_payload_fields(&collection_name, point_ids, fields)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Remove specific keys from the payload of the given points.
    pub async fn delete_payload(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> CoreResult<()> {
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();
        self.backend
            .delete_payload(&collection_name, point_ids, keys)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Paginated scan over points in a collection.
    pub async fn scroll(
        &self,
        query: ScrollQuery<'_>,
    ) -> CoreResult<(Vec<VectorPoint>, Option<String>)> {
        let collection_name = VectorIndexLocation::new(query.space_id, query.tag_name, query.field_name)
            .to_collection_name();
        self.backend
            .scroll(
                &collection_name,
                query.limit,
                query.offset,
                query.with_payload,
                query.with_vector,
            )
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }
}

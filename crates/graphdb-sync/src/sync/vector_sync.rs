//! Vector Synchronization Coordinator
//!
//! Coordinates vector index updates with graph data changes.

use std::collections::HashMap;

#[cfg(feature = "embedding")]
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::core::Value;
use crate::sync::backend::VectorBackend;
use crate::sync::vector_error::{VectorCoordinatorError, VectorCoordinatorResult, VectorError};

#[cfg(feature = "embedding")]
use graphdb_embedding::EmbeddingService;
pub use vector_search::types::{DistanceMetric, PointId, SearchQuery, SearchResult, VectorPoint};
use vector_search::{
    CollectionConfig, FilterCondition, IndexMetadata, PayloadSchemaType, VectorFilter,
};

/// Validate a distance metric at the index-creation entry points.
///
/// Only metrics every backend supports are accepted here so requests fail
/// fast with one consistent error instead of deep inside a specific engine
/// or on the remote server.
fn validate_metric(distance: DistanceMetric) -> VectorCoordinatorResult<()> {
    if matches!(
        distance,
        DistanceMetric::Cosine
            | DistanceMetric::Euclid
            | DistanceMetric::Dot
            | DistanceMetric::Manhattan
    ) {
        Ok(())
    } else {
        Err(VectorCoordinatorError::Vector(VectorError::ConfigError(
            format!(
                "distance metric {distance:?} is not supported; supported metrics: Cosine, Euclid, Dot, Manhattan",
            ),
        )))
    }
}

fn validate_metric_for_backend(
    backend: &VectorBackend,
    distance: DistanceMetric,
) -> VectorCoordinatorResult<()> {
    validate_metric(distance)?;
    let _ = backend;
    Ok(())
}

/// Runtime state of the vector engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorEngineState {
    /// Engine is disabled: user-facing vector operations fail with
    /// [`VectorCoordinatorError::EngineDisabled`]; delivery-plane batches are
    /// skipped and counted. Logical index metadata is still tracked for
    /// schema correctness.
    Disabled,
    /// Engine is active; mutations and searches execute against the backend.
    Active,
}

/// Vector point data for synchronization
#[derive(Debug, Clone)]
pub struct VectorPointData {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: HashMap<String, Value>,
}

/// Vector change type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum VectorChangeType {
    Insert,
    Delete,
}

impl From<crate::sync::types::ChangeType> for VectorChangeType {
    fn from(ct: crate::sync::types::ChangeType) -> Self {
        match ct {
            crate::sync::types::ChangeType::Insert | crate::sync::types::ChangeType::Update => {
                VectorChangeType::Insert
            }
            crate::sync::types::ChangeType::Delete => VectorChangeType::Delete,
        }
    }
}

/// Search options for vector search
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub query_vector: Vec<f32>,
    pub limit: usize,
    pub threshold: Option<f32>,
    pub filter: Option<VectorFilter>,
}

impl SearchOptions {
    pub fn new(
        space_id: u64,
        tag_name: impl Into<String>,
        field_name: impl Into<String>,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
            query_vector,
            limit,
            threshold: None,
            filter: None,
        }
    }

    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = Some(threshold);
        self
    }

    pub fn with_filter(mut self, filter: VectorFilter) -> Self {
        self.filter = Some(filter);
        self
    }
}

/// Vector index location identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VectorIndexLocation {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
}

const VECTOR_INDEX_PREFIX: &str = "space";

impl VectorIndexLocation {
    pub fn new(space_id: u64, tag_name: impl Into<String>, field_name: impl Into<String>) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
        }
    }

    /// Generate collection name from this index location.
    ///
    /// **ARCHITECTURAL NOTE**: This uses space-level collection granularity.
    /// All vector indexes within the same space share a single physical collection,
    /// with logical isolation via the `group_id` field in the payload.
    ///
    /// **Implications**:
    /// - Different (tag, field) combinations in the same space cannot have different
    ///   vector dimensions or distance metrics.
    /// - Deletion by vertex_id removes all vectors for that vertex across all
    ///   (tag, field) combinations in the space.
    /// - This is a deliberate design choice for resource efficiency. If finer
    ///   isolation is needed, change this to use (space_id, tag_name, field_name)
    ///   as the collection name.
    pub fn to_collection_name(&self) -> String {
        format!("{}_{}", VECTOR_INDEX_PREFIX, self.space_id)
    }

    /// Generate group ID for logical isolation within a space-level collection.
    /// This is used as a filter condition in vector searches.
    pub fn group_id(&self) -> String {
        format!("{}_{}", self.tag_name, self.field_name)
    }
}

/// Vector change context
#[derive(Debug, Clone)]
pub struct VectorChangeContext {
    pub location: VectorIndexLocation,
    pub change_type: VectorChangeType,
    pub data: VectorPointData,
}

impl VectorChangeContext {
    pub fn new(
        space_id: u64,
        tag_name: impl Into<String>,
        field_name: impl Into<String>,
        change_type: VectorChangeType,
        data: VectorPointData,
    ) -> Self {
        Self {
            location: VectorIndexLocation::new(space_id, tag_name, field_name),
            change_type,
            data,
        }
    }
}

/// Vector synchronization coordinator
pub struct VectorSyncCoordinator {
    backend: VectorBackend,
    #[cfg(feature = "embedding")]
    embedding_service: Option<Arc<EmbeddingService>>,
    /// Tracks registered logical indexes by key "space_{space_id}_{tag}_{field}" -> metadata
    logical_indexes: DashMap<VectorIndexLocation, IndexMetadata>,
    /// Vector change items skipped because the engine is disabled (delivery
    /// plane). Observable accounting for silent degradation.
    disabled_skips: std::sync::atomic::AtomicU64,
    /// Tokio runtime handle for blocking async operations from sync context.
    /// Using `Handle` instead of `&Runtime` avoids lifetime issues while allowing
    /// the caller (API layer or tests) to control the runtime lifecycle.
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for VectorSyncCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("VectorSyncCoordinator");
        debug
            .field("backend", &self.backend)
            .field("logical_index_count", &self.logical_indexes.len());
        #[cfg(feature = "embedding")]
        debug.field("embedding_service", &self.embedding_service.is_some());
        debug.finish()
    }
}

impl VectorSyncCoordinator {
    pub fn is_disabled_engine(&self) -> bool {
        self.backend.is_disabled()
    }

    /// Returns the runtime state of the underlying vector engine.
    ///
    /// - `Disabled`: engine is not available; user-facing vector operations
    ///   fail with [`VectorCoordinatorError::EngineDisabled`], while
    ///   delivery-plane batches are skipped and accounted (see
    ///   [`Self::disabled_skip_count`]). Index metadata is still tracked
    ///   logically so that queries referencing vector indexes do not produce
    ///   schema errors.
    /// - `Active`: engine is operational; mutations and searches execute normally.
    pub fn engine_state(&self) -> VectorEngineState {
        if self.is_disabled_engine() {
            VectorEngineState::Disabled
        } else {
            VectorEngineState::Active
        }
    }

    /// Total number of vector change items skipped because the engine was
    /// disabled. Non-zero values mean vector data currently diverges from
    /// graph data.
    pub fn disabled_skip_count(&self) -> u64 {
        self.disabled_skips
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Create a new vector sync coordinator with an explicit runtime handle.
    ///
    /// The caller is responsible for ensuring the runtime outlives the coordinator.
    /// In async contexts, use `Handle::current()` or `Runtime::handle()`.
    /// In sync contexts (e.g. tests), create a runtime and pass its handle.
    pub fn new(
        backend: VectorBackend,
        #[cfg(feature = "embedding")] embedding_service: Option<Arc<EmbeddingService>>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            backend,
            #[cfg(feature = "embedding")]
            embedding_service,
            logical_indexes: DashMap::new(),
            disabled_skips: std::sync::atomic::AtomicU64::new(0),
            runtime,
        }
    }

    /// Convenience constructor without embedding service.
    ///
    /// Avoids the `#[cfg]` feature-unification pitfall where callers in other
    /// crates see a different function signature than the one compiled here.
    pub fn new_without_embedding(backend: VectorBackend, runtime: tokio::runtime::Handle) -> Self {
        #[cfg(feature = "embedding")]
        {
            Self::new(backend, None, runtime)
        }
        #[cfg(not(feature = "embedding"))]
        {
            Self::new(backend, runtime)
        }
    }

    /// Get the runtime handle for blocking async operations
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    /// Get the vector backend
    pub fn backend(&self) -> &VectorBackend {
        &self.backend
    }

    /// Get the embedding service
    #[cfg(feature = "embedding")]
    pub fn embedding_service(&self) -> Option<&Arc<EmbeddingService>> {
        self.embedding_service.as_ref()
    }

    /// Create a vector index (logical index in shared collection)
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: DistanceMetric,
    ) -> VectorCoordinatorResult<String> {
        validate_metric_for_backend(&self.backend, distance)?;

        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        if self.is_disabled_engine() {
            let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
            let meta = IndexMetadata::new(
                collection_name.clone(),
                CollectionConfig::new(vector_size, distance),
            );
            self.logical_indexes.insert(logical_key, meta);
            info!(
                "Logical vector index created in disabled mode: space={} tag={} field={} in collection {}",
                space_id, tag_name, field_name, collection_name
            );
            return Ok(collection_name);
        }

        // Index-tier fields are only meaningful for the remote qdrant backend;
        // the local engine controls its tiers through [vector.local.ivf].
        let config = if self.backend.is_local() {
            CollectionConfig::new(vector_size, distance)
        } else {
            let hnsw_config = vector_search::HnswConfig::new(16, 100).with_payload_m(16);
            CollectionConfig::new(vector_size, distance).with_hnsw(hnsw_config)
        };

        // Only create the physical collection if it doesn't exist yet
        if !self.backend.index_exists(&collection_name) {
            self.backend
                .create_index(&collection_name, &config)
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            // Create payload index for group_id filtering (best-effort, log on failure)
            if let Err(e) = self
                .backend
                .create_payload_index(&collection_name, "group_id", PayloadSchemaType::Keyword)
                .await
            {
                tracing::warn!(
                    "Failed to create payload index for group_id in collection '{}': {}",
                    collection_name,
                    e
                );
            }
        } else {
            if let Some(existing_meta) = self.backend.get_index_metadata(&collection_name) {
                if existing_meta.config.vector_size != vector_size
                    || existing_meta.config.distance != distance
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing_meta.config.vector_size,
                        existing_dist: format!("{:?}", existing_meta.config.distance),
                        requested_size: vector_size,
                        requested_dist: format!("{:?}", distance),
                    });
                }
            }
        }

        // Register logical index with the actual config used
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let meta = IndexMetadata::new(collection_name.clone(), config);
        self.logical_indexes.insert(logical_key, meta);

        info!(
            "Logical vector index created: space={} tag={} field={} in collection {}",
            space_id, tag_name, field_name, collection_name
        );
        Ok(collection_name)
    }

    /// Drop a vector index (remove logical index, physical collection remains)
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<()> {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = logical_key.to_collection_name();
        self.logical_indexes.remove(&logical_key);

        // The local engine owns its collection files, so once no logical
        // index references the collection anymore the physical directory can
        // be reclaimed. Remote collections keep their own lifecycle.
        if self.backend.is_local() && self.backend.index_exists(&collection_name) {
            let remaining_siblings = self
                .logical_indexes
                .iter()
                .filter(|entry| entry.value().name == collection_name)
                .count();
            if remaining_siblings == 0 {
                if let Err(error) = self.backend.delete_collection(&collection_name).await {
                    tracing::warn!(
                        "Failed to reclaim vector collection '{}': {}",
                        collection_name,
                        error
                    );
                }
            } else if let Err(error) = self
                .backend
                .delete_by_filter(
                    &collection_name,
                    VectorFilter::new().must(FilterCondition::match_value(
                        "group_id",
                        format!("{tag_name}_{field_name}"),
                    )),
                )
                .await
            {
                tracing::warn!(
                    "Failed to purge dropped vector group '{tag_name}_{field_name}' from collection '{}': {}",
                    collection_name,
                    error
                );
            }
        }

        info!(
            "Logical vector index dropped: space={} tag={} field={}",
            space_id, tag_name, field_name
        );
        Ok(())
    }

    /// Handle batch vector changes
    ///
    /// Delivery-plane semantics: when the engine is disabled the batch is
    /// *skipped and accounted* — logged with a warning, counted in
    /// [`Self::disabled_skip_count`] — instead of failing. Failing here would
    /// stall replication (the sync pipeline only records its LSN after this
    /// returns), while silently pretending success is invisible; the skip is
    /// observable through logs and the counter.
    pub async fn on_vector_change_batch(
        &self,
        contexts: Vec<VectorChangeContext>,
    ) -> VectorCoordinatorResult<()> {
        if self.is_disabled_engine() {
            self.disabled_skips
                .fetch_add(contexts.len() as u64, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                count = contexts.len(),
                total_skipped = self
                    .disabled_skips
                    .load(std::sync::atomic::Ordering::Relaxed),
                "vector engine disabled: skipped delivering vector changes; \
                 vector data diverges from graph data until re-synced"
            );
            return Ok(());
        }

        let mut upsert_by_collection: HashMap<String, Vec<VectorPoint>> = HashMap::new();
        let mut delete_by_collection: HashMap<String, Vec<String>> = HashMap::new();

        for ctx in contexts {
            let collection_name = ctx.location.to_collection_name();
            let point_id = ctx.data.id.to_string();

            match ctx.change_type {
                VectorChangeType::Insert => {
                    let vector = ctx.data.vector;
                    let mut json_payload: HashMap<String, serde_json::Value> = ctx
                        .data
                        .payload
                        .into_iter()
                        .filter_map(|(k, v)| serde_json::to_value(&v).ok().map(|json| (k, json)))
                        .collect();

                    json_payload.insert(
                        "group_id".to_string(),
                        serde_json::to_value(ctx.location.group_id())
                            .unwrap_or(serde_json::Value::Null),
                    );

                    let point = VectorPoint::new(point_id, vector).with_payload(json_payload);

                    upsert_by_collection
                        .entry(collection_name)
                        .or_default()
                        .push(point);
                }
                VectorChangeType::Delete => {
                    delete_by_collection
                        .entry(collection_name)
                        .or_default()
                        .push(point_id);
                }
            }
        }

        for (collection_name, points) in upsert_by_collection {
            let points_count = points.len();
            if points_count == 1 {
                self.backend
                    .upsert(&collection_name, points.into_iter().next().unwrap())
                    .await?;
            } else if !points.is_empty() {
                self.backend.upsert_batch(&collection_name, points).await?;
                debug!(
                    "Batch upserted {} vectors to collection {}",
                    points_count, collection_name
                );
            }
        }

        for (collection_name, point_ids) in delete_by_collection {
            let point_ids_count = point_ids.len();
            if point_ids_count == 1 {
                self.backend.delete(&collection_name, &point_ids[0]).await?;
            } else if !point_ids.is_empty() {
                let refs: Vec<&str> = point_ids.iter().map(|s| s.as_str()).collect();
                self.backend.delete_batch(&collection_name, &refs).await?;
                debug!(
                    "Batch deleted {} vectors from collection {}",
                    point_ids_count, collection_name
                );
            }
        }

        Ok(())
    }

    /// Search for similar vectors
    pub async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let results = self.backend.search(collection, query).await?;
        Ok(results)
    }

    /// Search with options
    pub async fn search_with_options(
        &self,
        options: SearchOptions,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let collection_name =
            VectorIndexLocation::new(options.space_id, &options.tag_name, &options.field_name)
                .to_collection_name();

        let mut query = SearchQuery::new(options.query_vector, options.limit);

        if let Some(threshold) = options.threshold {
            query = query.with_score_threshold(threshold);
        }

        // Inject group_id filter to scope search to the correct (tag, field) group
        let group_id = format!("{}_{}", options.tag_name, options.field_name);
        let mut filter = options.filter.unwrap_or_default();
        filter = filter.must(FilterCondition::match_value("group_id", group_id));
        query = query.with_filter(filter);

        let results = self.search(&collection_name, query).await?;
        Ok(results)
    }

    /// Search with space_id and tag/field names
    pub async fn search_by_location(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        let filter = VectorFilter::new().must(FilterCondition::match_value(
            "group_id",
            format!("{}_{}", tag_name, field_name),
        ));
        let query = SearchQuery::new(query_vector, limit).with_filter(filter);
        self.search(&collection_name, query).await
    }

    /// Search with threshold
    pub async fn search_with_threshold(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        let filter = VectorFilter::new().must(FilterCondition::match_value(
            "group_id",
            format!("{}_{}", tag_name, field_name),
        ));
        let query = SearchQuery::new(query_vector, limit)
            .with_score_threshold(threshold)
            .with_filter(filter);
        self.search(&collection_name, query).await
    }

    /// Search with filter
    pub async fn search_with_filter(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        let group_id = format!("{}_{}", tag_name, field_name);
        let filter = filter.must(FilterCondition::match_value("group_id", group_id));
        let query = SearchQuery::new(query_vector, limit).with_filter(filter);
        self.search(&collection_name, query).await
    }

    /// Embed text to vector
    #[cfg(feature = "embedding")]
    pub async fn embed_text(&self, text: &str) -> VectorCoordinatorResult<Vec<f32>> {
        if let Some(embedding) = &self.embedding_service {
            let vector = embedding
                .embed(text)
                .await
                .map_err(|e| VectorCoordinatorError::EmbeddingError(e.to_string()))?;
            Ok(vector)
        } else {
            Err(VectorCoordinatorError::EmbeddingError(
                "Embedding service not available".to_string(),
            ))
        }
    }

    /// Check if index exists (logical index)
    pub fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.contains_key(&logical_key)
    }

    /// Attach a statement-level logical index name to an existing
    /// `(space_id, tag, field)` index.
    ///
    /// SQL `CREATE VECTOR INDEX <name>` resolves to a physical location; the
    /// name is recorded here so later statements (SEARCH / LOOKUP / DROP) can
    /// resolve `<name>` back to its location during planning.
    /// Best-effort: a missing logical index leaves the map untouched.
    pub fn set_index_name(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        index_name: &str,
    ) {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        if let Some(mut meta) = self.logical_indexes.get_mut(&logical_key) {
            meta.index_name = Some(index_name.to_string());
        }
    }

    /// Get logical index metadata for a tag/field combination
    pub fn index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexMetadata> {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.get(&logical_key).map(|v| v.clone())
    }

    /// List all indexes (logical indexes)
    pub fn list_indexes(&self) -> Vec<crate::sync::vector_sync::IndexMetadataWrapper> {
        self.logical_indexes
            .iter()
            .map(|pair| {
                let location = pair.key();
                crate::sync::vector_sync::IndexMetadataWrapper {
                    collection_name: pair.value().name.clone(),
                    space_id: location.space_id,
                    tag_name: location.tag_name.clone(),
                    field_name: location.field_name.clone(),
                    index_name: pair.value().index_name.clone(),
                }
            })
            .collect()
    }

    /// Register a logical index (for disabled-engine mode or external registration)
    pub fn register_logical_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        collection_name: String,
        config: CollectionConfig,
        user_index_name: Option<String>,
    ) {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let meta = if let Some(idx_name) = user_index_name {
            IndexMetadata::with_index_name(collection_name, config, idx_name)
        } else {
            IndexMetadata::new(collection_name, config)
        };
        self.logical_indexes.insert(logical_key, meta);
    }

    /// Create vector index with config (logical index in shared collection)
    pub async fn create_index_with_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        config: CollectionConfig,
    ) -> VectorCoordinatorResult<String> {
        validate_metric_for_backend(&self.backend, config.distance)?;

        let collection_name =
            VectorIndexLocation::new(space_id, tag_name, field_name).to_collection_name();

        if !self.backend.index_exists(&collection_name) {
            self.backend
                .create_index(&collection_name, &config)
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            // Create payload index for group_id filtering (best-effort, log on failure)
            if let Err(e) = self
                .backend
                .create_payload_index(&collection_name, "group_id", PayloadSchemaType::Keyword)
                .await
            {
                tracing::warn!(
                    "Failed to create payload index for group_id in collection '{}': {}",
                    collection_name,
                    e
                );
            }
        } else {
            if let Some(existing_meta) = self.backend.get_index_metadata(&collection_name) {
                if existing_meta.config.vector_size != config.vector_size
                    || existing_meta.config.distance != config.distance
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing_meta.config.vector_size,
                        existing_dist: format!("{:?}", existing_meta.config.distance),
                        requested_size: config.vector_size,
                        requested_dist: format!("{:?}", config.distance),
                    });
                }
            }
        }

        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let meta = IndexMetadata::new(collection_name.clone(), config);
        self.logical_indexes.insert(logical_key, meta);

        info!(
            "Logical vector index created with config: space={} tag={} field={} in collection {}",
            space_id, tag_name, field_name, collection_name
        );
        Ok(collection_name)
    }

    /// Search with threshold and filter
    pub async fn search_with_threshold_and_filter(
        &self,
        mut options: SearchOptions,
        threshold: f32,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        options.threshold = Some(threshold);
        options.filter = Some(filter);
        self.search_with_options(options).await
    }
}

/// Parsed vector index location from a collection name.
#[derive(Debug, Clone)]
pub struct IndexMetadataWrapper {
    pub collection_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub index_name: Option<String>,
}

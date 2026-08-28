//! Vector Index Manager
//!
//! Manages vector index metadata, lifecycle (create/drop), and search
//! operations.  Extracted from `VectorSyncCoordinator` to separate index
//! management from synchronization concerns.
//!
//! The query layer depends on `VectorIndexManager` for index CRUD and search,
//! while the sync layer (`VectorSyncCoordinator`) wraps it and adds outbox /
//! change-batching / embedding capabilities.

use std::collections::HashMap;

use dashmap::DashMap;
use tracing::info;

use crate::backend::VectorBackend;
use crate::vector_error::{VectorCoordinatorError, VectorCoordinatorResult, VectorError};
pub use vector_search::types::{DistanceMetric, PointId, SearchQuery, SearchResult, VectorPoint};
use vector_search::{
    types::validate_distance_metric, CollectionConfig, FilterCondition, IndexMetadata,
    PayloadSchemaType, VectorFilter,
};

use super::vector_sync::{
    CollectionGranularity, SearchOptions, VectorIndexLocation,
};

/// Validate a distance metric at the index-creation entry points.
fn validate_metric(distance: DistanceMetric) -> VectorCoordinatorResult<()> {
    validate_distance_metric(distance)
        .map_err(|e| VectorCoordinatorError::Vector(VectorError::ConfigError(e)))
}

fn validate_metric_for_backend(
    backend: &VectorBackend,
    distance: DistanceMetric,
) -> VectorCoordinatorResult<()> {
    validate_metric(distance)?;
    let _ = backend;
    Ok(())
}

/// Manages vector index metadata, lifecycle, and search operations.
///
/// This struct owns the vector backend handle and the logical index registry.
/// It provides all index CRUD and search methods without any synchronization
/// (outbox, change batching, embedding) concerns.
pub struct VectorIndexManager {
    backend: VectorBackend,
    /// Tracks registered logical indexes by key `(space_id, tag, field)`.
    logical_indexes: DashMap<VectorIndexLocation, IndexMetadata>,
    /// Collection granularity. Space-level is default for backward
    /// compatibility; Field-level gives physical isolation per (tag,field).
    granularity: parking_lot::RwLock<CollectionGranularity>,
}

impl std::fmt::Debug for VectorIndexManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorIndexManager")
            .field("backend", &self.backend)
            .field("logical_index_count", &self.logical_indexes.len())
            .field("granularity", &*self.granularity.read())
            .finish()
    }
}

impl VectorIndexManager {
    /// Create a new index manager with the given backend.
    pub fn new(backend: VectorBackend) -> Self {
        Self {
            backend,
            logical_indexes: DashMap::new(),
            granularity: parking_lot::RwLock::new(CollectionGranularity::default()),
        }
    }

    /// Get a reference to the underlying vector backend.
    pub fn backend(&self) -> &VectorBackend {
        &self.backend
    }

    pub fn granularity(&self) -> CollectionGranularity {
        *self.granularity.read()
    }

    pub fn set_granularity(&self, granularity: CollectionGranularity) {
        *self.granularity.write() = granularity;
    }

    /// Resolve collection name respecting the configured granularity.
    pub fn collection_name_for(&self, loc: &VectorIndexLocation) -> String {
        loc.to_collection_name_with(self.granularity())
    }

    /// Resolve group_id respecting granularity. `None` means field-level
    /// physical isolation, no group filter needed.
    pub fn group_id_for(&self, loc: &VectorIndexLocation) -> Option<String> {
        loc.group_id_with(self.granularity())
    }

    /// Whether the engine is disabled.
    pub fn is_disabled_engine(&self) -> bool {
        self.backend.is_disabled()
    }

    // ── Index lifecycle ───────────────────────────────────────────────

    /// Create a vector index (logical index in shared collection).
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: DistanceMetric,
    ) -> VectorCoordinatorResult<String> {
        validate_metric_for_backend(&self.backend, distance)?;

        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);

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

        let config = if self.backend.is_local() {
            CollectionConfig::new(vector_size, distance)
        } else {
            let hnsw_config = vector_search::HnswConfig::new(16, 100).with_payload_m(16);
            CollectionConfig::new(vector_size, distance).with_hnsw(hnsw_config)
        };

        if !self.backend.index_exists(&collection_name) {
            self.backend
                .create_index(&collection_name, &config)
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            if self.granularity() == CollectionGranularity::Space {
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
            }
        } else {
            if let Some(existing_meta) = self.backend.get_index_metadata(&collection_name) {
                let existing = &existing_meta.config;
                if existing.vector_size != vector_size
                    || existing.distance != distance
                    || existing.index_type != config.index_type
                    || format!("{:?}", existing.hnsw_config)
                        != format!("{:?}", config.hnsw_config)
                    || existing.quantization_config != config.quantization_config
                    || format!("{:?}", existing.ivf_config)
                        != format!("{:?}", config.ivf_config)
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing.vector_size,
                        existing_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            existing.distance,
                            existing.index_type,
                            existing.hnsw_config,
                            existing.quantization_config
                        ),
                        requested_size: vector_size,
                        requested_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            distance,
                            config.index_type,
                            config.hnsw_config,
                            config.quantization_config
                        ),
                    });
                }
            }
        }

        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let meta = IndexMetadata::new(collection_name.clone(), config);
        self.logical_indexes.insert(logical_key, meta);

        info!(
            "Logical vector index created: space={} tag={} field={} in collection {}",
            space_id, tag_name, field_name, collection_name
        );
        Ok(collection_name)
    }

    /// Create vector index with config (logical index in shared collection).
    pub async fn create_index_with_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        config: CollectionConfig,
    ) -> VectorCoordinatorResult<String> {
        validate_metric_for_backend(&self.backend, config.distance)?;

        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);

        if !self.backend.index_exists(&collection_name) {
            self.backend
                .create_index(&collection_name, &config)
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            if self.granularity() == CollectionGranularity::Space {
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
            }
        } else {
            if let Some(existing_meta) = self.backend.get_index_metadata(&collection_name) {
                let existing = &existing_meta.config;
                if existing.vector_size != config.vector_size
                    || existing.distance != config.distance
                    || existing.index_type != config.index_type
                    || format!("{:?}", existing.hnsw_config)
                        != format!("{:?}", config.hnsw_config)
                    || existing.quantization_config != config.quantization_config
                    || format!("{:?}", existing.ivf_config)
                        != format!("{:?}", config.ivf_config)
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing.vector_size,
                        existing_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            existing.distance,
                            existing.index_type,
                            existing.hnsw_config,
                            existing.quantization_config
                        ),
                        requested_size: config.vector_size,
                        requested_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            config.distance,
                            config.index_type,
                            config.hnsw_config,
                            config.quantization_config
                        ),
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

    /// Drop a vector index (remove logical index, physical collection remains).
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<()> {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&logical_key);
        self.logical_indexes.remove(&logical_key);

        if self.granularity() == CollectionGranularity::Field {
            if self.backend.index_exists(&collection_name) {
                if let Err(error) = self.backend.delete_collection(&collection_name).await {
                    tracing::warn!(
                        "Failed to reclaim vector collection '{}' (field granularity): {}",
                        collection_name,
                        error
                    );
                }
            }
        } else if self.backend.is_local() && self.backend.index_exists(&collection_name) {
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

    /// Check if index exists (logical index).
    pub fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.contains_key(&logical_key)
    }

    /// Attach a statement-level logical index name to an existing index.
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

    /// Get logical index metadata for a tag/field combination.
    pub fn index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexMetadata> {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.get(&logical_key).map(|v| v.clone())
    }

    /// List all logical indexes.
    pub fn list_indexes(&self) -> Vec<super::vector_sync::IndexMetadataWrapper> {
        self.logical_indexes
            .iter()
            .map(|pair| {
                let location = pair.key();
                super::vector_sync::IndexMetadataWrapper {
                    collection_name: pair.value().name.clone(),
                    space_id: location.space_id,
                    tag_name: location.tag_name.clone(),
                    field_name: location.field_name.clone(),
                    index_name: pair.value().index_name.clone(),
                }
            })
            .collect()
    }

    /// Register a logical index (for disabled-engine mode or external registration).
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

    // ── Search ────────────────────────────────────────────────────────

    /// Search for similar vectors.
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

    /// Streaming search.
    pub async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>,
        >,
    > {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let stream = self.backend.search_stream(collection, query).await?;
        Ok(stream)
    }

    /// Streaming scroll.
    pub async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        std::pin::Pin<Box<dyn futures::Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    > {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let stream = self
            .backend
            .scroll_stream(collection, batch_size, with_payload, with_vector)
            .await?;
        Ok(stream)
    }

    /// Search with options (the primary search entry point).
    pub async fn search_with_options(
        &self,
        options: SearchOptions,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let loc =
            VectorIndexLocation::new(options.space_id, &options.tag_name, &options.field_name);
        let collection_name = self.collection_name_for(&loc);

        let mut query = SearchQuery::new(options.query_vector, options.limit);

        if let Some(threshold) = options.threshold {
            query = query.with_score_threshold(threshold);
        }

        let mut filter = options.filter.unwrap_or_default();
        if let Some(gid) = self.group_id_for(&loc) {
            filter = filter.must(FilterCondition::match_value("group_id", gid));
        }
        query = query.with_filter(filter);

        let results = self.search(&collection_name, query).await?;
        Ok(results)
    }

    /// Search with space_id and tag/field names.
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
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let mut query = SearchQuery::new(query_vector, limit);
        if let Some(gid) = self.group_id_for(&loc) {
            let filter = VectorFilter::new().must(FilterCondition::match_value("group_id", gid));
            query = query.with_filter(filter);
        }
        self.search(&collection_name, query).await
    }

    /// Search with threshold.
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
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let mut query = SearchQuery::new(query_vector, limit).with_score_threshold(threshold);
        if let Some(gid) = self.group_id_for(&loc) {
            let filter = VectorFilter::new().must(FilterCondition::match_value("group_id", gid));
            query = query.with_filter(filter);
        }
        self.search(&collection_name, query).await
    }

    /// Search with filter.
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
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let mut enriched = filter;
        if let Some(gid) = self.group_id_for(&loc) {
            enriched = enriched.must(FilterCondition::match_value("group_id", gid));
        }
        let query = SearchQuery::new(query_vector, limit).with_filter(enriched);
        self.search(&collection_name, query).await
    }

    /// Search with threshold and filter.
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

    // ── Sync helpers (used by VectorSyncCoordinator) ──────────────────

    /// Prepare upsert/delete operations for a batch of vector changes,
    /// returning them grouped by collection name.
    pub(crate) fn prepare_change_batch(
        &self,
        contexts: Vec<super::vector_sync::VectorChangeContext>,
    ) -> (
        HashMap<String, Vec<VectorPoint>>,
        HashMap<String, Vec<String>>,
    ) {
        use super::vector_sync::VectorChangeType;

        let mut upsert_by_collection: HashMap<String, Vec<VectorPoint>> = HashMap::new();
        let mut delete_by_collection: HashMap<String, Vec<String>> = HashMap::new();

        for ctx in contexts {
            let collection_name = self.collection_name_for(&ctx.location);
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

                    if let Some(gid) = self.group_id_for(&ctx.location) {
                        json_payload.insert(
                            "group_id".to_string(),
                            serde_json::to_value(gid).unwrap_or(serde_json::Value::Null),
                        );
                    }

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

        (upsert_by_collection, delete_by_collection)
    }
}

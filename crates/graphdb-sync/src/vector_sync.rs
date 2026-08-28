//! Vector Synchronization Coordinator
//!
//! Coordinates vector index updates with graph data changes.  Wraps a
//! [`VectorIndexManager`](crate::VectorIndexManager) for index lifecycle and
//! search, and adds synchronization concerns: change batching, outbox
//! integration, embedding, and disabled-engine accounting.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::backend::VectorBackend;
use crate::vector_error::{VectorCoordinatorError, VectorCoordinatorResult};
use crate::VectorIndexManager;
use graphdb_core::Value;

#[cfg(feature = "embedding")]
use graphdb_embedding::EmbeddingService;
pub use vector_search::types::{DistanceMetric, PointId, SearchQuery, SearchResult, VectorPoint};
use vector_search::{CollectionConfig, IndexMetadata, VectorFilter};

// ── Types (kept here for backward compatibility) ──────────────────────────

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

impl From<crate::types::ChangeType> for VectorChangeType {
    fn from(ct: crate::types::ChangeType) -> Self {
        match ct {
            crate::types::ChangeType::Insert | crate::types::ChangeType::Update => {
                VectorChangeType::Insert
            }
            crate::types::ChangeType::Delete => VectorChangeType::Delete,
        }
    }
}

/// Consistency level for vector search.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SearchConsistency {
    #[default]
    Eventual,
    ReadYourWrites { timeout_ms: u64 },
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
    pub consistency: SearchConsistency,
    pub minimum_lsn: Option<graphdb_core::types::CommitLsn>,
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
            consistency: SearchConsistency::default(),
            minimum_lsn: None,
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

    pub fn with_consistency(mut self, consistency: SearchConsistency) -> Self {
        self.consistency = consistency;
        self
    }

    pub fn with_minimum_lsn(mut self, lsn: graphdb_core::types::CommitLsn) -> Self {
        self.minimum_lsn = Some(lsn);
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

/// Collection granularity mirrors `graphdb_config::VectorCollectionGranularity`
/// but is re-declared here to avoid a hard dependency on `graphdb-config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CollectionGranularity {
    #[default]
    Space,
    Field,
}

/// Naming strategy derived from granularity.
pub trait CollectionNaming: Send + Sync + std::fmt::Debug {
    fn collection_name(&self, loc: &VectorIndexLocation) -> String;
    fn group_id(&self, loc: &VectorIndexLocation) -> Option<String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SpaceGranularityNaming;
impl CollectionNaming for SpaceGranularityNaming {
    fn collection_name(&self, loc: &VectorIndexLocation) -> String {
        format!("{}_{}", VECTOR_INDEX_PREFIX, loc.space_id)
    }
    fn group_id(&self, loc: &VectorIndexLocation) -> Option<String> {
        Some(format!("{}_{}", loc.tag_name, loc.field_name))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FieldGranularityNaming;
impl CollectionNaming for FieldGranularityNaming {
    fn collection_name(&self, loc: &VectorIndexLocation) -> String {
        format!("{}_{}_{}_{}", VECTOR_INDEX_PREFIX, loc.space_id, loc.tag_name, loc.field_name)
    }
    fn group_id(&self, _loc: &VectorIndexLocation) -> Option<String> {
        None
    }
}

impl VectorIndexLocation {
    pub fn new(space_id: u64, tag_name: impl Into<String>, field_name: impl Into<String>) -> Self {
        Self {
            space_id,
            tag_name: tag_name.into(),
            field_name: field_name.into(),
        }
    }

    pub fn to_collection_name(&self) -> String {
        self.to_collection_name_with(CollectionGranularity::Space)
    }

    pub fn to_collection_name_with(&self, granularity: CollectionGranularity) -> String {
        match granularity {
            CollectionGranularity::Space => format!("{}_{}", VECTOR_INDEX_PREFIX, self.space_id),
            CollectionGranularity::Field => format!(
                "{}_{}_{}_{}",
                VECTOR_INDEX_PREFIX, self.space_id, self.tag_name, self.field_name
            ),
        }
    }

    pub fn group_id(&self) -> String {
        self.group_id_with(CollectionGranularity::Space)
            .unwrap_or_default()
    }

    pub fn group_id_with(&self, granularity: CollectionGranularity) -> Option<String> {
        match granularity {
            CollectionGranularity::Space => Some(format!("{}_{}", self.tag_name, self.field_name)),
            CollectionGranularity::Field => None,
        }
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

// ── VectorSyncCoordinator ─────────────────────────────────────────────────

/// Vector synchronization coordinator
pub struct VectorSyncCoordinator {
    index_manager: Arc<VectorIndexManager>,
    #[cfg(feature = "embedding")]
    embedding_service: Option<Arc<EmbeddingService>>,
    /// Vector change items skipped because the engine is disabled (delivery
    /// plane). Observable accounting for silent degradation.
    disabled_skips: std::sync::atomic::AtomicU64,
    /// Tokio runtime handle for blocking async operations from sync context.
    runtime: tokio::runtime::Handle,
    /// Optional outbox handle for `ReadYourWrites` consistency waiting.
    outbox: parking_lot::RwLock<Option<std::sync::Arc<crate::SqliteOutbox>>>,
}

impl std::fmt::Debug for VectorSyncCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("VectorSyncCoordinator");
        debug.field("index_manager", &self.index_manager);
        #[cfg(feature = "embedding")]
        debug.field("embedding_service", &self.embedding_service.is_some());
        debug.finish()
    }
}

impl VectorSyncCoordinator {
    pub fn is_disabled_engine(&self) -> bool {
        self.index_manager.is_disabled_engine()
    }

    /// Returns the runtime state of the underlying vector engine.
    pub fn engine_state(&self) -> crate::VectorEngineState {
        if self.is_disabled_engine() {
            crate::VectorEngineState::Disabled
        } else {
            crate::VectorEngineState::Active
        }
    }

    /// Total number of vector change items skipped because the engine was
    /// disabled.
    pub fn disabled_skip_count(&self) -> u64 {
        self.disabled_skips
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Create a new vector sync coordinator with an explicit runtime handle.
    pub fn new(
        backend: VectorBackend,
        #[cfg(feature = "embedding")] embedding_service: Option<Arc<EmbeddingService>>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            index_manager: Arc::new(VectorIndexManager::new(backend)),
            #[cfg(feature = "embedding")]
            embedding_service,
            disabled_skips: std::sync::atomic::AtomicU64::new(0),
            runtime,
            outbox: parking_lot::RwLock::new(None),
        }
    }

    /// Convenience constructor without embedding service.
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

    /// Get a reference to the underlying index manager.
    pub fn index_manager(&self) -> &Arc<VectorIndexManager> {
        &self.index_manager
    }

    /// Get the runtime handle for blocking async operations.
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    /// Get the vector backend.
    pub fn backend(&self) -> &VectorBackend {
        self.index_manager.backend()
    }

    pub fn set_outbox(&self, outbox: std::sync::Arc<crate::SqliteOutbox>) {
        *self.outbox.write() = Some(outbox);
    }

    pub fn granularity(&self) -> CollectionGranularity {
        self.index_manager.granularity()
    }

    pub fn set_granularity(&self, granularity: CollectionGranularity) {
        self.index_manager.set_granularity(granularity);
    }

    /// Resolve collection name respecting the configured granularity.
    pub fn collection_name_for(&self, loc: &VectorIndexLocation) -> String {
        self.index_manager.collection_name_for(loc)
    }

    /// Resolve group_id respecting granularity.
    pub fn group_id_for(&self, loc: &VectorIndexLocation) -> Option<String> {
        self.index_manager.group_id_for(loc)
    }

    fn vector_index_id(space_id: u64, tag_name: &str) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in format!("vector:{}:{}", space_id, tag_name).as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash & (i64::MAX as u64)
    }

    /// Get the embedding service.
    #[cfg(feature = "embedding")]
    pub fn embedding_service(&self) -> Option<&Arc<EmbeddingService>> {
        self.embedding_service.as_ref()
    }

    // ── Index lifecycle (delegated) ───────────────────────────────────

    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: DistanceMetric,
    ) -> VectorCoordinatorResult<String> {
        self.index_manager
            .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
            .await
    }

    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<()> {
        self.index_manager
            .drop_vector_index(space_id, tag_name, field_name)
            .await
    }

    pub fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        self.index_manager.index_exists(space_id, tag_name, field_name)
    }

    pub fn set_index_name(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        index_name: &str,
    ) {
        self.index_manager
            .set_index_name(space_id, tag_name, field_name, index_name);
    }

    pub fn index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexMetadata> {
        self.index_manager.index_info(space_id, tag_name, field_name)
    }

    pub fn list_indexes(&self) -> Vec<IndexMetadataWrapper> {
        self.index_manager.list_indexes()
    }

    pub fn register_logical_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        collection_name: String,
        config: CollectionConfig,
        user_index_name: Option<String>,
    ) {
        self.index_manager
            .register_logical_index(space_id, tag_name, field_name, collection_name, config, user_index_name);
    }

    pub async fn create_index_with_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        config: CollectionConfig,
    ) -> VectorCoordinatorResult<String> {
        self.index_manager
            .create_index_with_config(space_id, tag_name, field_name, config)
            .await
    }

    // ── Sync: change batch handling ───────────────────────────────────

    pub async fn on_vector_change_batch(
        &self,
        contexts: Vec<crate::VectorChangeContext>,
    ) -> VectorCoordinatorResult<()> {
        if self.is_disabled_engine() {
            self.disabled_skips
                .fetch_add(contexts.len() as u64, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                count = contexts.len(),
                total_skipped = self
                    .disabled_skips
                    .load(std::sync::atomic::Ordering::Relaxed),
                "vector engine disabled: retaining {} vector changes for retry; \
                 vector data diverges until engine recovers",
                contexts.len()
            );
            return Err(VectorCoordinatorError::EngineDisabled);
        }

        if self.backend().is_local() {
            if let Some(local) = self.backend().as_local() {
                let txn_id = {
                    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
                    now.wrapping_mul(0x9e3779b97f4a7c15)
                        .wrapping_add(contexts.len() as u64)
                };
                let mut ops: Vec<vector_search::engine::TxnOp> = Vec::with_capacity(contexts.len());
                for ctx in contexts {
                    let collection = self.collection_name_for(&ctx.location);
                    let point_id = ctx.data.id;
                    match ctx.change_type {
                        VectorChangeType::Insert => {
                            let mut json_payload: HashMap<String, serde_json::Value> = ctx
                                .data
                                .payload
                                .into_iter()
                                .filter_map(|(k, v)| {
                                    serde_json::to_value(&v).ok().map(|json| (k, json))
                                })
                                .collect();
                            if let Some(gid) = self.group_id_for(&ctx.location) {
                                json_payload.insert(
                                    "group_id".to_string(),
                                    serde_json::to_value(gid)
                                        .unwrap_or(serde_json::Value::Null),
                                );
                            }
                            let point = VectorPoint::new(point_id, ctx.data.vector)
                                .with_payload(json_payload);
                            ops.push(vector_search::engine::TxnOp::Upsert { collection, point });
                        }
                        VectorChangeType::Delete => {
                            ops.push(vector_search::engine::TxnOp::Delete {
                                collection,
                                point_id,
                            });
                        }
                    }
                }
                if !ops.is_empty() {
                    local.apply_txn(txn_id, ops).map_err(|e| {
                        VectorCoordinatorError::Vector(crate::vector_error::VectorError::from(e))
                    })?;
                    debug!("Local vector group-commit txn {} applied", txn_id);
                }
                return Ok(());
            }
        }

        let (upsert_by_collection, delete_by_collection) =
            self.index_manager.prepare_change_batch(contexts);

        use std::future::Future;
        use std::pin::Pin;
        let mut all_futs: Vec<Pin<Box<dyn Future<Output = VectorCoordinatorResult<()>> + Send>>> =
            Vec::new();
        for (collection_name, points) in upsert_by_collection {
            let backend = self.backend().clone();
            all_futs.push(Box::pin(async move {
                let points_count = points.len();
                if points_count == 1 {
                    backend
                        .upsert(&collection_name, points.into_iter().next().unwrap())
                        .await?;
                } else if !points.is_empty() {
                    backend.upsert_batch(&collection_name, points).await?;
                    debug!(
                        "Batch upserted {} vectors to collection {}",
                        points_count, collection_name
                    );
                }
                Ok::<(), VectorCoordinatorError>(())
            }));
        }
        for (collection_name, point_ids) in delete_by_collection {
            let backend = self.backend().clone();
            all_futs.push(Box::pin(async move {
                let point_ids_count = point_ids.len();
                if point_ids_count == 1 {
                    backend.delete(&collection_name, &point_ids[0]).await?;
                } else if !point_ids.is_empty() {
                    let refs: Vec<&str> = point_ids.iter().map(|s| s.as_str()).collect();
                    backend.delete_batch(&collection_name, &refs).await?;
                    debug!(
                        "Batch deleted {} vectors from collection {}",
                        point_ids_count, collection_name
                    );
                }
                Ok::<(), VectorCoordinatorError>(())
            }));
        }
        if !all_futs.is_empty() {
            futures::future::try_join_all(all_futs).await?;
        }

        Ok(())
    }

    // ── Search (delegated with RYW consistency) ───────────────────────

    pub async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        self.index_manager.search(collection, query).await
    }

    pub async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>,
        >,
    > {
        self.index_manager.search_stream(collection, query).await
    }

    pub async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        std::pin::Pin<Box<dyn futures::Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    > {
        self.index_manager
            .scroll_stream(collection, batch_size, with_payload, with_vector)
            .await
    }

    /// Search with options (handles RYW consistency before delegating).
    pub async fn search_with_options(
        &self,
        options: SearchOptions,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        if let SearchConsistency::ReadYourWrites { timeout_ms } = &options.consistency {
            let outbox_opt = {
                let guard = self.outbox.read();
                guard.clone()
            };
            if let Some(outbox) = outbox_opt {
                let minimum_lsn = if let Some(lsn) = options.minimum_lsn {
                    lsn
                } else {
                    match outbox.materialized_lsn().await {
                        Ok(lsn) => lsn,
                        Err(e) => {
                            return Err(VectorCoordinatorError::Vector(
                                crate::vector_error::VectorError::Internal(e),
                            ))
                        }
                    }
                };
                if minimum_lsn.get() != 0 {
                    let target =
                        graphdb_core::types::TargetId::new("vector".to_string()).map_err(|e| {
                            VectorCoordinatorError::Vector(
                                crate::vector_error::VectorError::Internal(e),
                            )
                        })?;
                    let index_id = Self::vector_index_id(options.space_id, &options.tag_name);
                    let generation = 1u64;
                    let waited = outbox
                        .wait_for_minimum_lsn(
                            &target,
                            index_id,
                            generation,
                            minimum_lsn,
                            *timeout_ms,
                        )
                        .await
                        .map_err(|e| {
                            VectorCoordinatorError::Vector(
                                crate::vector_error::VectorError::Internal(e),
                            )
                        })?;
                    if !waited {
                        return Err(VectorCoordinatorError::Vector(
                            crate::vector_error::VectorError::Timeout,
                        ));
                    }
                }
            }
        }
        self.index_manager.search_with_options(options).await
    }

    pub async fn search_by_location(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        self.index_manager
            .search_by_location(space_id, tag_name, field_name, query_vector, limit)
            .await
    }

    pub async fn search_with_threshold(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        self.index_manager
            .search_with_threshold(space_id, tag_name, field_name, query_vector, limit, threshold)
            .await
    }

    pub async fn search_with_filter(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        self.index_manager
            .search_with_filter(space_id, tag_name, field_name, query_vector, limit, filter)
            .await
    }

    pub async fn search_with_threshold_and_filter(
        &self,
        options: SearchOptions,
        threshold: f32,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        self.index_manager
            .search_with_threshold_and_filter(options, threshold, filter)
            .await
    }

    // ── Embedding ─────────────────────────────────────────────────────

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
}

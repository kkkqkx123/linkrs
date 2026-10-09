//! Vector Synchronization Coordinator
//!
//! Coordinates vector index updates with graph data changes.  Wraps a
//! [`VectorIndexManager`](crate::VectorIndexManager) for index lifecycle and
//! search, and adds synchronization concerns: change batching, outbox
//! integration, embedding, and disabled-engine accounting.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use linkrs_fulltext::{RebuildPhase, RebuildProgress};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::backend::VectorBackend;
use crate::vector_error::{VectorCoordinatorError, VectorCoordinatorResult};
use crate::VectorIndexManager;
use linkrs_core::Value;

#[cfg(feature = "embedding")]
pub type EmbeddingService =
    llm_embedding::EmbeddingService<llm_embedding::OpenAICompatibleProvider>;

#[cfg(feature = "embedding")]
pub const EMBEDDING_BATCH_SIZE: usize = 32;
#[cfg(feature = "embedding")]
pub const EMBEDDING_MAX_TOKENS_PER_CALL: usize = 8192;
#[cfg(feature = "embedding")]
const EMBEDDING_MAX_RETRIES: u32 = 2;
pub use simvec::types::{DistanceMetric, PointId, SearchQuery, SearchResult, VectorPoint};
use simvec::{CollectionConfig, IndexMetadata, VectorFilter};

// ── Types (kept here for backward compatibility) ──────────────────────────

/// Vector point data for synchronization
#[derive(Debug, Clone)]
pub struct VectorPointData {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: HashMap<Arc<str>, Value>,
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
    ReadYourWrites {
        timeout_ms: u64,
    },
}

/// Search options for vector search
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub query_vector: Vec<f32>,
    pub limit: usize,
    /// Original query text; drives the optional post-recall rerank stage.
    /// `None` for pure vector queries, which skip rerank entirely.
    pub query_text: Option<String>,
    pub threshold: Option<f32>,
    pub filter: Option<VectorFilter>,
    pub consistency: SearchConsistency,
    pub minimum_lsn: Option<linkrs_core::types::CommitLsn>,
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
            query_text: None,
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

    pub fn with_minimum_lsn(mut self, lsn: linkrs_core::types::CommitLsn) -> Self {
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

/// Collection granularity mirrors `linkrs_config::VectorCollectionGranularity`
/// but is re-declared here to avoid a hard dependency on `linkrs-config`.
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
        format!(
            "{}_{}_{}_{}",
            VECTOR_INDEX_PREFIX, loc.space_id, loc.tag_name, loc.field_name
        )
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
    #[cfg(feature = "embedding")]
    query_embedding_service: Option<Arc<EmbeddingService>>,
    /// Optional shared observability handle for embedding calls. Interior
    /// mutability so late binding through the sync manager needs no rebuild.
    #[cfg(any(feature = "embedding", feature = "rerank"))]
    stats_manager: parking_lot::RwLock<Option<Arc<linkrs_metrics::StatsManager>>>,
    /// Optional post-recall rerank service, assembled by the server layer.
    #[cfg(feature = "rerank")]
    rerank_service: Option<Arc<llm_rerank::CohereRerankProvider>>,
    /// Snapshot of the rerank configuration carried with the service.
    #[cfg(feature = "rerank")]
    rerank_config: Option<linkrs_config::VectorRerankConfig>,
    /// Vector change items skipped because the engine is disabled (delivery
    /// plane). Observable accounting for silent degradation.
    disabled_skips: std::sync::atomic::AtomicU64,
    /// Tokio runtime handle for blocking async operations from sync context.
    runtime: tokio::runtime::Handle,
    /// Optional outbox handle for `ReadYourWrites` consistency waiting.
    outbox: parking_lot::RwLock<Option<std::sync::Arc<crate::SqliteOutbox>>>,
    /// Live rebuild progress by index; present only while a rebuild runs.
    rebuild_progress: DashMap<(u64, String, String), RebuildProgress>,
}

impl std::fmt::Debug for VectorSyncCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("VectorSyncCoordinator");
        debug.field("index_manager", &self.index_manager);
        #[cfg(feature = "embedding")]
        debug.field("embedding_service", &self.embedding_service.is_some());
        #[cfg(feature = "embedding")]
        debug.field(
            "query_embedding_service",
            &self.query_embedding_service.is_some(),
        );
        #[cfg(any(feature = "embedding", feature = "rerank"))]
        debug.field("has_stats_manager", &self.stats_manager.read().is_some());
        #[cfg(feature = "rerank")]
        debug.field("rerank_service", &self.rerank_service.is_some());
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
            #[cfg(feature = "embedding")]
            query_embedding_service: None,
            #[cfg(any(feature = "embedding", feature = "rerank"))]
            stats_manager: parking_lot::RwLock::new(None),
            #[cfg(feature = "rerank")]
            rerank_service: None,
            #[cfg(feature = "rerank")]
            rerank_config: None,
            disabled_skips: std::sync::atomic::AtomicU64::new(0),
            runtime,
            outbox: parking_lot::RwLock::new(None),
            rebuild_progress: DashMap::new(),
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

    /// Publish fence for one vector index. Delivery applies hold the read
    /// guard; rebuild holds the write guard across its critical windows.
    /// Delegates to the index manager so explicit purges share the same lock.
    pub fn publish_fence_for(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Arc<tokio::sync::RwLock<()>> {
        self.index_manager
            .publish_fence_for(space_id, tag_name, field_name)
    }

    /// Latest rebuild progress snapshot for one vector index, if any.
    pub fn rebuild_progress(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<RebuildProgress> {
        self.rebuild_progress
            .get(&(space_id, tag_name.to_string(), field_name.to_string()))
            .map(|entry| entry.clone())
    }

    /// Replace the stored rebuild progress snapshot.
    pub fn update_rebuild_progress(&self, progress: RebuildProgress) {
        let key = (
            progress.space_id,
            progress.tag_name.clone(),
            progress.field_name.clone(),
        );
        self.rebuild_progress.insert(key, progress);
    }

    /// Advance the stored rebuild phase, if a snapshot is present.
    pub fn set_rebuild_phase(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        phase: RebuildPhase,
    ) {
        let key = (space_id, tag_name.to_string(), field_name.to_string());
        if let Some(mut entry) = self.rebuild_progress.get_mut(&key) {
            entry.phase = phase;
        }
    }

    /// Drop the stored rebuild progress snapshot.
    pub fn remove_rebuild_progress(&self, space_id: u64, tag_name: &str, field_name: &str) {
        self.rebuild_progress
            .remove(&(space_id, tag_name.to_string(), field_name.to_string()));
    }

    /// Vector indexes needing operator attention: rebuilds stuck in the
    /// `Failed` phase (live data still serves; retry the rebuild). Mirrors
    /// the fulltext `inconsistent` listing so both engines share one
    /// alerting shape.
    pub fn inconsistent_vector_indexes(&self) -> Vec<RebuildProgress> {
        self.rebuild_progress
            .iter()
            .filter(|entry| entry.value().phase == RebuildPhase::Failed)
            .map(|entry| entry.value().clone())
            .collect()
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

    /// Set the query-side embedding service used for read-time text
    /// queries. Falls back to the shared service when unset.
    #[cfg(feature = "embedding")]
    pub fn with_query_embedding_service(mut self, service: Arc<EmbeddingService>) -> Self {
        self.query_embedding_service = Some(service);
        self
    }

    /// Effective service for read-time queries: the query override when
    /// present, otherwise the shared document-side service.
    #[cfg(feature = "embedding")]
    fn query_service(&self) -> Option<&Arc<EmbeddingService>> {
        self.query_embedding_service
            .as_ref()
            .or(self.embedding_service.as_ref())
    }

    /// Late-bind the shared observability handle. Safe to call after the
    /// coordinator is already mounted in the sync manager.
    #[cfg(any(feature = "embedding", feature = "rerank"))]
    pub fn set_stats_manager(&self, stats_manager: Arc<linkrs_metrics::StatsManager>) {
        *self.stats_manager.write() = Some(stats_manager);
    }

    /// Record one embedding call against the bound handle, if any.
    #[cfg(feature = "embedding")]
    fn record_embedding_call(
        &self,
        prompt_tokens: u64,
        total_tokens: u64,
        latency_ms: u64,
        success: bool,
    ) {
        let stats = self.stats_manager.read().clone();
        if let Some(stats) = stats.as_ref() {
            stats.record_vector_embedding(prompt_tokens, total_tokens, latency_ms, success);
        }
    }

    // ── Index lifecycle (delegated) ───────────────────────────────────

    /// Vertex-scoped index creation. The coordinator has no schema access:
    /// callers (query planner/executor, HTTP) must reject edge type names
    /// before reaching here; vector delivery and rebuild never produce
    /// points for edges.
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
        self.index_manager
            .index_exists(space_id, tag_name, field_name)
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
        self.index_manager
            .index_info(space_id, tag_name, field_name)
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
        self.index_manager.register_logical_index(
            space_id,
            tag_name,
            field_name,
            collection_name,
            config,
            user_index_name,
        );
    }

    /// Vertex-scoped index creation with full config. Same contract as
    /// [`Self::create_vector_index`]: `tag_name` is always a vertex tag.
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
        self.deliver_change_batch(None, contexts).await
    }

    /// Deliver a batch into one physical collection (rebuild temp). Shares
    /// the live grouping/`group_id` logic via
    /// `prepare_change_batch_for_collection`; temp writes need no publish
    /// fence (temps have no concurrent readers), bypass the disabled-engine
    /// skip accounting of live delivery, and still fail when the engine is
    /// disabled so rebuilds abort instead of silently diverging.
    pub async fn on_vector_change_batch_to(
        &self,
        collection: &str,
        contexts: Vec<crate::VectorChangeContext>,
    ) -> VectorCoordinatorResult<()> {
        self.deliver_change_batch(Some(collection), contexts).await
    }

    async fn deliver_change_batch(
        &self,
        collection_override: Option<&str>,
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
                let mut ops: Vec<simvec::engine::TxnOp> = Vec::with_capacity(contexts.len());
                for ctx in contexts {
                    let collection = collection_override
                        .map(str::to_string)
                        .unwrap_or_else(|| self.collection_name_for(&ctx.location));
                    let point_id = ctx.data.id;
                    match ctx.change_type {
                        VectorChangeType::Insert => {
                            let mut json_payload: HashMap<String, serde_json::Value> = ctx
                                .data
                                .payload
                                .into_iter()
                                .filter_map(|(k, v)| {
                                    serde_json::to_value(&v)
                                        .ok()
                                        .map(|json| (k.to_string(), json))
                                })
                                .collect();
                            if let Some(gid) = self.group_id_for(&ctx.location) {
                                json_payload.insert(
                                    "group_id".into(),
                                    serde_json::to_value(gid).unwrap_or(serde_json::Value::Null),
                                );
                            }
                            let point = VectorPoint::new(point_id, ctx.data.vector)
                                .with_payload(json_payload);
                            ops.push(simvec::engine::TxnOp::Upsert { collection, point });
                        }
                        VectorChangeType::Delete => {
                            ops.push(simvec::engine::TxnOp::Delete {
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

        let (upsert_by_collection, delete_by_collection) = self
            .index_manager
            .prepare_change_batch_for_collection(collection_override, contexts);

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

    /// Search with options (handles RYW consistency, then delegates to the
    /// index manager).
    ///
    /// This is the single entry point for the post-recall rerank stage: with
    /// an active stage the recall limit is widened to the rerank candidate
    /// window, results are reranked, and the output is truncated back to the
    /// requested `limit`, so callers always observe at most `limit` hits in
    /// rerank order.
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
                        linkrs_core::types::TargetId::new("vector".to_string()).map_err(|e| {
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
        #[cfg(feature = "rerank")]
        {
            let mut options = options;
            let requested_limit = options.limit;
            let window = self.rerank_recall_window(options.query_text.as_deref());
            if window > options.limit {
                options.limit = window;
            }
            let query_text = options.query_text.clone();
            let field_name = options.field_name.clone();
            let results = self.index_manager.search_with_options(options).await?;
            let mut results = self
                .maybe_rerank(query_text.as_deref(), &field_name, results)
                .await;
            results.truncate(requested_limit);
            return Ok(results);
        }
        #[cfg(not(feature = "rerank"))]
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

    /// Resolve query text into a vector for read-time text queries.
    ///
    /// Uses the query-side service when configured, otherwise the shared
    /// document-side service. Retries transport-level failures and splits
    /// oversized batches by estimated token budget.
    #[cfg(feature = "embedding")]
    pub async fn embed_text(&self, text: &str) -> VectorCoordinatorResult<Vec<f32>> {
        if let Some(embedding) = self.query_service() {
            let owned = vec![text.to_string()];
            let started = std::time::Instant::now();
            let outcome = Self::embed_with_retry(embedding, &owned).await;
            let latency_ms = started.elapsed().as_millis() as u64;
            match &outcome {
                Ok(result) => self.record_embedding_call(
                    result.prompt_tokens,
                    result.total_tokens,
                    latency_ms,
                    true,
                ),
                Err(_) => self.record_embedding_call(0, 0, latency_ms, false),
            }
            let result = outcome?;
            result.embeddings.into_iter().next().ok_or_else(|| {
                VectorCoordinatorError::EmbeddingError(
                    "Embedding service returned no vector".to_string(),
                )
            })
        } else {
            Err(VectorCoordinatorError::EmbeddingError(
                "Embedding service not available".to_string(),
            ))
        }
    }

    /// Batch variant of `embed_text` for write-time conversion.
    ///
    /// Preserves input order; callers map each output vector back to the
    /// corresponding text field before staging the vector intent.
    #[cfg(feature = "embedding")]
    pub async fn embed_texts(&self, texts: &[&str]) -> VectorCoordinatorResult<Vec<Vec<f32>>> {
        if let Some(embedding) = &self.embedding_service {
            let owned: Vec<String> = texts.iter().map(|text| text.to_string()).collect();
            let started = std::time::Instant::now();
            let outcome = Self::embed_with_retry(embedding, &owned).await;
            let latency_ms = started.elapsed().as_millis() as u64;
            match &outcome {
                Ok(result) => self.record_embedding_call(
                    result.prompt_tokens,
                    result.total_tokens,
                    latency_ms,
                    true,
                ),
                Err(_) => self.record_embedding_call(0, 0, latency_ms, false),
            }
            Ok(outcome?.embeddings)
        } else {
            Err(VectorCoordinatorError::EmbeddingError(
                "Embedding service not available".to_string(),
            ))
        }
    }

    /// Embed with token-budget splitting and limited retry.
    ///
    /// Generic over the provider so scripted test doubles exercise the same
    /// retry and batching path as the production service. Oversized single
    /// texts fail fast without network traffic. Retry covers transport
    /// failures, timeouts, 429 and 5xx only; config and request errors return
    /// immediately. Usage is reported through debug telemetry so deployments
    /// can observe token spend.
    #[cfg(feature = "embedding")]
    async fn embed_with_retry<P: llm_embedding::EmbeddingProvider>(
        service: &llm_embedding::EmbeddingService<P>,
        texts: &[String],
    ) -> VectorCoordinatorResult<llm_embedding::EmbeddingResult> {
        if texts.is_empty() {
            return Ok(llm_embedding::EmbeddingResult::default());
        }
        let batches = Self::split_for_token_budget(texts)?;
        let mut embeddings = Vec::with_capacity(texts.len());
        let mut prompt_tokens = 0u64;
        let mut total_tokens = 0u64;
        for batch in &batches {
            let mut attempt = 0u32;
            loop {
                match llm_embedding::EmbeddingProvider::embed(service.provider(), batch).await {
                    Ok(result) => {
                        debug!(
                            "embedding batch ok: texts={} prompt_tokens={} total_tokens={}",
                            batch.len(),
                            result.prompt_tokens,
                            result.total_tokens
                        );
                        prompt_tokens += result.prompt_tokens;
                        total_tokens += result.total_tokens;
                        embeddings.extend(result.embeddings);
                        break;
                    }
                    Err(err) => {
                        if !is_retryable_embedding_error(&err) || attempt >= EMBEDDING_MAX_RETRIES {
                            return Err(VectorCoordinatorError::EmbeddingError(err.to_string()));
                        }
                        attempt += 1;
                        debug!("embedding attempt {} failed, retrying: {}", attempt, err);
                        tokio::time::sleep(std::time::Duration::from_millis(
                            100 * 2u64.pow(attempt.saturating_sub(1)),
                        ))
                        .await;
                    }
                }
            }
        }
        Ok(llm_embedding::EmbeddingResult {
            embeddings,
            prompt_tokens,
            total_tokens,
        })
    }

    /// Split texts so each provider call stays within the count batch and
    /// the estimated token budget. Rejects a single over-budget text.
    #[cfg(feature = "embedding")]
    fn split_for_token_budget(texts: &[String]) -> VectorCoordinatorResult<Vec<Vec<String>>> {
        let mut batches: Vec<Vec<String>> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        let mut current_tokens = 0usize;
        for text in texts {
            let estimate = llm_token::estimate_tokens(text);
            if estimate > EMBEDDING_MAX_TOKENS_PER_CALL {
                return Err(VectorCoordinatorError::EmbeddingError(format!(
                    "embedding text exceeds token budget: estimated {} > {}",
                    estimate, EMBEDDING_MAX_TOKENS_PER_CALL
                )));
            }
            if current.len() >= EMBEDDING_BATCH_SIZE
                || current_tokens + estimate > EMBEDDING_MAX_TOKENS_PER_CALL
            {
                batches.push(std::mem::take(&mut current));
                current_tokens = 0;
            }
            current_tokens += estimate;
            current.push(text.clone());
        }
        if !current.is_empty() {
            batches.push(current);
        }
        Ok(batches)
    }

    // ── Rerank ────────────────────────────────────────────────────────

    /// Attach the optional post-recall rerank stage. Absent by default;
    /// assembly warns and disables on invalid config instead of failing.
    #[cfg(feature = "rerank")]
    pub fn with_rerank_service(
        mut self,
        service: Arc<llm_rerank::CohereRerankProvider>,
        config: linkrs_config::VectorRerankConfig,
    ) -> Self {
        self.rerank_service = Some(service);
        self.rerank_config = Some(config);
        self
    }

    /// Recall window enlargement for text queries when rerank is active.
    /// Returns zero when rerank is disabled or the query carries no text,
    /// leaving existing recall limits untouched.
    #[cfg(feature = "rerank")]
    fn rerank_recall_window(&self, query_text: Option<&str>) -> usize {
        match (
            query_text,
            self.rerank_service.as_ref(),
            self.rerank_config.as_ref(),
        ) {
            (Some(_), Some(_), Some(config)) => config.max_candidates.max(1),
            _ => 0,
        }
    }

    /// Reorder recall results with the configured rerank service. Fail-open:
    /// any missing precondition or provider failure returns the input order.
    #[cfg(feature = "rerank")]
    async fn maybe_rerank(
        &self,
        query_text: Option<&str>,
        field_name: &str,
        results: Vec<SearchResult>,
    ) -> Vec<SearchResult> {
        let Some(query) = query_text else {
            return results;
        };
        let Some(service) = self.rerank_service.as_ref() else {
            return results;
        };
        let Some(config) = self.rerank_config.as_ref() else {
            return results;
        };
        if results.len() < 2 {
            return results;
        }
        let max_candidates = config.max_candidates.max(1);
        let mut head = results;
        let tail = if head.len() > max_candidates {
            head.split_off(max_candidates)
        } else {
            Vec::new()
        };
        let text_field = config.text_field.as_deref().unwrap_or(field_name);
        let indexed = Self::rerank_candidates(&head, text_field);
        if indexed.len() < 2 {
            head.extend(tail);
            return head;
        }
        let runtime = llm_rerank::RerankRuntimeConfig {
            max_candidates,
            temperature: 0.0,
            return_reasoning: false,
            score_fusion_strategy: config.fusion,
            timeout_ms: service.config().timeout_secs.max(1).saturating_mul(1000),
        };
        let started = std::time::Instant::now();
        let outcome =
            Self::rerank_with_provider(service.as_ref(), &runtime, query, &indexed, head).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(mut reordered) => {
                self.record_rerank_call(latency_ms, true);
                reordered.extend(tail);
                reordered
            }
            Err(original) => {
                self.record_rerank_call(latency_ms, false);
                let mut restored = original;
                restored.extend(tail);
                restored
            }
        }
    }

    /// Record one issued rerank call against the bound handle, if any.
    /// Skipped reranks never reach here.
    #[cfg(feature = "rerank")]
    fn record_rerank_call(&self, latency_ms: u64, success: bool) {
        let stats = self.stats_manager.read().clone();
        if let Some(stats) = stats.as_ref() {
            stats.record_vector_rerank(latency_ms, success);
        }
    }

    /// Collect text-bearing recall hits as `(recall index, text)` pairs.
    /// The text comes from the payload field under test, defaulting to the
    /// searched field; hits without usable text are excluded.
    #[cfg(feature = "rerank")]
    fn rerank_candidates(results: &[SearchResult], text_field: &str) -> Vec<(usize, String)> {
        results
            .iter()
            .enumerate()
            .filter_map(|(index, result)| {
                let text = result.payload.as_ref()?.get(text_field)?.as_str()?;
                if text.is_empty() {
                    return None;
                }
                Some((index, text.to_string()))
            })
            .collect()
    }

    /// Run one rerank call and map the provider order back onto the recall
    /// hits. Generic over the provider so scripted test doubles exercise the
    /// same mapping path as the production service. Returns the original
    /// hits on any provider or mapping failure.
    #[cfg(feature = "rerank")]
    async fn rerank_with_provider<P: llm_rerank::RerankProvider>(
        provider: &P,
        runtime: &llm_rerank::RerankRuntimeConfig,
        query: &str,
        indexed: &[(usize, String)],
        head: Vec<SearchResult>,
    ) -> Result<Vec<SearchResult>, Vec<SearchResult>> {
        let mut candidates = Vec::with_capacity(indexed.len());
        for (index, text) in indexed {
            if let Some(point) = head.get(*index) {
                candidates.push(llm_rerank::RerankCandidate {
                    id: index.to_string(),
                    content: text.clone(),
                    file_path: point.id.to_string(),
                    initial_score: point.score,
                    entity_type: None,
                    metadata: HashMap::new(),
                });
            } else {
                return Err(head);
            }
        }
        let request = llm_rerank::RerankRequest {
            query: query.to_string(),
            candidates,
            config: runtime.clone(),
        };
        let result = match provider.rerank(&request).await {
            Ok(result) => result,
            Err(_) => return Err(head),
        };
        if result.reranked_candidates.len() != indexed.len() {
            return Err(head);
        }
        let mut order = Vec::with_capacity(indexed.len());
        let mut seen = std::collections::HashSet::with_capacity(indexed.len());
        for item in &result.reranked_candidates {
            match item.id.parse::<usize>() {
                Ok(index) if index < head.len() && seen.insert(index) => order.push(index),
                _ => return Err(head),
            }
        }
        let mut slots: Vec<Option<SearchResult>> = head.into_iter().map(Some).collect();
        let mut ordered = Vec::with_capacity(slots.len());
        for index in order {
            if let Some(slot) = slots.get_mut(index) {
                if let Some(point) = slot.take() {
                    ordered.push(point);
                }
            }
        }
        ordered.extend(slots.into_iter().flatten());
        Ok(ordered)
    }
}

#[cfg(feature = "embedding")]
fn is_retryable_embedding_error(err: &llm_embedding::EmbeddingError) -> bool {
    match err {
        llm_embedding::EmbeddingError::Transport(_) | llm_embedding::EmbeddingError::Timeout => {
            true
        }
        llm_embedding::EmbeddingError::Provider { status, .. } => {
            *status == 429 || (500..=599).contains(status)
        }
        _ => false,
    }
}

#[cfg(all(test, feature = "embedding"))]
mod embedding_tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::VecDeque;

    enum ScriptedStep {
        Succeed,
        Fail(llm_embedding::EmbeddingError),
    }

    struct ScriptedProvider {
        dimension: usize,
        steps: std::sync::Mutex<VecDeque<ScriptedStep>>,
        calls: std::sync::Mutex<Vec<usize>>,
    }

    impl ScriptedProvider {
        fn succeeding(dimension: usize) -> Self {
            Self::with_steps(dimension, Vec::new())
        }

        fn with_steps(dimension: usize, steps: Vec<ScriptedStep>) -> Self {
            Self {
                dimension,
                steps: std::sync::Mutex::new(steps.into()),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded_calls(&self) -> Vec<usize> {
            self.calls
                .lock()
                .expect("test mutex is never poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl llm_embedding::EmbeddingProvider for ScriptedProvider {
        async fn embed(
            &self,
            texts: &[String],
        ) -> llm_embedding::Result<llm_embedding::EmbeddingResult> {
            self.calls
                .lock()
                .expect("test mutex is never poisoned")
                .push(texts.len());
            let step = self
                .steps
                .lock()
                .expect("test mutex is never poisoned")
                .pop_front();
            match step {
                Some(ScriptedStep::Fail(err)) => Err(err),
                _ => Ok(llm_embedding::EmbeddingResult {
                    embeddings: texts
                        .iter()
                        .enumerate()
                        .map(|(index, _)| vec![index as f32; self.dimension])
                        .collect(),
                    prompt_tokens: texts.len() as u64,
                    total_tokens: texts.len() as u64,
                }),
            }
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        fn model_name(&self) -> &str {
            "scripted-test"
        }
    }

    fn scripted_service(
        provider: ScriptedProvider,
    ) -> llm_embedding::EmbeddingService<ScriptedProvider> {
        llm_embedding::EmbeddingService::new(provider, EMBEDDING_BATCH_SIZE)
    }

    #[test]
    fn split_respects_count_batch() {
        let texts: Vec<String> = (0..EMBEDDING_BATCH_SIZE + 1)
            .map(|_| "a".to_string())
            .collect();
        let batches = VectorSyncCoordinator::split_for_token_budget(&texts)
            .expect("short texts stay within budget");
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), EMBEDDING_BATCH_SIZE);
        assert_eq!(batches[1].len(), 1);
    }

    #[test]
    fn split_rejects_single_over_budget_text() {
        let texts = vec!["a".repeat(100_000)];
        let before = texts.len();
        let err = VectorSyncCoordinator::split_for_token_budget(&texts)
            .expect_err("over-budget text must fail");
        assert!(err.to_string().contains("token budget"));
        assert_eq!(before, texts.len());
    }

    #[test]
    fn retryable_classification_matches_policy() {
        assert!(is_retryable_embedding_error(
            &llm_embedding::EmbeddingError::Transport("down".to_string())
        ));
        assert!(is_retryable_embedding_error(
            &llm_embedding::EmbeddingError::Timeout
        ));
        for status in [429u16, 500, 503] {
            assert!(
                is_retryable_embedding_error(&llm_embedding::EmbeddingError::Provider {
                    status,
                    message: "retryable".to_string(),
                    retry_after_ms: None,
                }),
                "status {status} must be retryable"
            );
        }
        for status in [400u16, 401, 404] {
            assert!(
                !is_retryable_embedding_error(&llm_embedding::EmbeddingError::Provider {
                    status,
                    message: "permanent".to_string(),
                    retry_after_ms: None,
                }),
                "status {status} must not be retryable"
            );
        }
        assert!(!is_retryable_embedding_error(
            &llm_embedding::EmbeddingError::Config("bad wiring".to_string())
        ));
    }

    #[tokio::test]
    async fn empty_input_returns_empty_without_calls() {
        let provider = ScriptedProvider::succeeding(4);
        let service = scripted_service(provider);
        let result = VectorSyncCoordinator::embed_with_retry(&service, &[])
            .await
            .expect("empty input succeeds");
        assert!(result.embeddings.is_empty());
        assert_eq!(service.provider().recorded_calls(), Vec::<usize>::new());
    }

    #[tokio::test]
    async fn single_text_shape_matches_read_path() {
        let provider = ScriptedProvider::succeeding(4);
        let service = scripted_service(provider);
        let result = VectorSyncCoordinator::embed_with_retry(&service, &["hello".to_string()])
            .await
            .expect("single embed succeeds");
        assert_eq!(result.embeddings.len(), 1);
        assert_eq!(result.embeddings[0], vec![0.0; 4]);
        assert_eq!(service.provider().recorded_calls(), vec![1]);
    }

    #[tokio::test]
    async fn batch_shape_preserves_write_path_order() {
        let provider = ScriptedProvider::succeeding(2);
        let service = scripted_service(provider);
        let texts: Vec<String> = ["alpha", "beta", "gamma"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let result = VectorSyncCoordinator::embed_with_retry(&service, &texts)
            .await
            .expect("batch embed succeeds");
        assert_eq!(result.embeddings.len(), 3);
        for (index, vector) in result.embeddings.iter().enumerate() {
            assert_eq!(*vector, vec![index as f32; 2]);
        }
    }

    #[tokio::test]
    async fn retry_recovers_after_transport_failure() {
        let provider = ScriptedProvider::with_steps(
            4,
            vec![
                ScriptedStep::Fail(llm_embedding::EmbeddingError::Transport(
                    "connection reset".to_string(),
                )),
                ScriptedStep::Succeed,
            ],
        );
        let service = scripted_service(provider);
        let result = VectorSyncCoordinator::embed_with_retry(&service, &["retry me".to_string()])
            .await
            .expect("retry recovers");
        assert_eq!(result.embeddings.len(), 1);
        assert_eq!(service.provider().recorded_calls(), vec![1, 1]);
    }

    #[tokio::test]
    async fn non_retryable_error_returns_without_retry() {
        let provider = ScriptedProvider::with_steps(
            4,
            vec![ScriptedStep::Fail(llm_embedding::EmbeddingError::Config(
                "bad wiring".to_string(),
            ))],
        );
        let service = scripted_service(provider);
        let err = VectorSyncCoordinator::embed_with_retry(&service, &["x".to_string()])
            .await
            .expect_err("config errors fail fast");
        assert!(err.to_string().contains("bad wiring"));
        assert_eq!(service.provider().recorded_calls(), vec![1]);
    }

    #[tokio::test]
    async fn exhausted_retries_return_error() {
        let provider = ScriptedProvider::with_steps(
            4,
            vec![
                ScriptedStep::Fail(llm_embedding::EmbeddingError::Timeout),
                ScriptedStep::Fail(llm_embedding::EmbeddingError::Timeout),
                ScriptedStep::Fail(llm_embedding::EmbeddingError::Timeout),
                ScriptedStep::Succeed,
            ],
        );
        let service = scripted_service(provider);
        VectorSyncCoordinator::embed_with_retry(&service, &["x".to_string()])
            .await
            .expect_err("retries are capped");
        assert_eq!(service.provider().recorded_calls(), vec![1, 1, 1]);
    }
}

#[cfg(all(test, feature = "rerank"))]
mod rerank_tests {
    use super::*;
    use async_trait::async_trait;

    struct ScriptedRerank {
        reverse: bool,
        fail: bool,
        drop_last: bool,
        calls: std::sync::Mutex<usize>,
    }

    impl ScriptedRerank {
        fn ordered() -> Self {
            Self {
                reverse: false,
                fail: false,
                drop_last: false,
                calls: std::sync::Mutex::new(0),
            }
        }

        fn reversed() -> Self {
            Self {
                reverse: true,
                ..Self::ordered()
            }
        }

        fn failing() -> Self {
            Self {
                fail: true,
                ..Self::ordered()
            }
        }

        fn truncated() -> Self {
            Self {
                drop_last: true,
                ..Self::ordered()
            }
        }

        fn call_count(&self) -> usize {
            *self.calls.lock().expect("test mutex is never poisoned")
        }
    }

    #[async_trait]
    impl llm_rerank::RerankProvider for ScriptedRerank {
        async fn rerank(
            &self,
            request: &llm_rerank::RerankRequest,
        ) -> llm_rerank::Result<llm_rerank::RerankResult> {
            *self.calls.lock().expect("test mutex is never poisoned") += 1;
            if self.fail {
                return Err(llm_rerank::RerankError::Transport("down".to_string()));
            }
            let mut ids: Vec<String> = request.candidates.iter().map(|c| c.id.clone()).collect();
            if self.drop_last {
                ids.pop();
            }
            if self.reverse {
                ids.reverse();
            }
            let total = ids.len();
            let reranked = ids
                .into_iter()
                .enumerate()
                .map(|(position, id)| {
                    let initial = request
                        .candidates
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| c.initial_score)
                        .unwrap_or(0.0);
                    llm_rerank::RerankedCandidate {
                        id,
                        rerank_score: (total - position) as f32,
                        initial_score: initial,
                        final_score: (total - position) as f32,
                        rank_change: 0,
                        reasoning: None,
                    }
                })
                .collect();
            Ok(llm_rerank::RerankResult::new(reranked))
        }

        fn provider_name(&self) -> &str {
            "scripted-test"
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    fn search_hit(id: &str, score: f32, text: Option<&str>) -> SearchResult {
        let mut payload = simvec::types::Payload::new();
        if let Some(text) = text {
            payload.insert(
                "content".to_string(),
                serde_json::Value::String(text.to_string()),
            );
        }
        SearchResult::new(id, score).with_payload(payload)
    }

    fn hit_ids(results: &[SearchResult]) -> Vec<String> {
        results.iter().map(|r| r.id.to_string()).collect()
    }

    #[test]
    fn candidates_skip_hits_without_text() {
        let results = vec![
            search_hit("a", 0.9, Some("alpha")),
            search_hit("b", 0.8, None),
            search_hit("c", 0.7, Some("")),
            search_hit("d", 0.6, Some("delta")),
        ];
        let indexed = VectorSyncCoordinator::rerank_candidates(&results, "content");
        assert_eq!(
            indexed,
            vec![(0, "alpha".to_string()), (3, "delta".to_string())]
        );
    }

    #[tokio::test]
    async fn provider_order_reorders_hits() {
        let provider = ScriptedRerank::reversed();
        let head = vec![
            search_hit("a", 0.9, Some("alpha")),
            search_hit("b", 0.8, Some("beta")),
            search_hit("c", 0.7, Some("gamma")),
        ];
        let indexed = VectorSyncCoordinator::rerank_candidates(&head, "content");
        let reordered = VectorSyncCoordinator::rerank_with_provider(
            &provider,
            &runtime_config(),
            "q",
            &indexed,
            head,
        )
        .await
        .expect("reorder succeeds");
        assert_eq!(
            hit_ids(&reordered),
            vec!["c".to_string(), "b".to_string(), "a".to_string()]
        );
        assert_eq!(provider.call_count(), 1);
    }

    #[tokio::test]
    async fn provider_failure_returns_original_hits() {
        let provider = ScriptedRerank::failing();
        let head = vec![
            search_hit("a", 0.9, Some("alpha")),
            search_hit("b", 0.8, Some("beta")),
        ];
        let indexed = VectorSyncCoordinator::rerank_candidates(&head, "content");
        let original = hit_ids(&head);
        let restored = VectorSyncCoordinator::rerank_with_provider(
            &provider,
            &runtime_config(),
            "q",
            &indexed,
            head,
        )
        .await
        .expect_err("failure keeps original");
        assert_eq!(hit_ids(&restored), original);
    }

    #[tokio::test]
    async fn count_mismatch_returns_original_hits() {
        let provider = ScriptedRerank::truncated();
        let head = vec![
            search_hit("a", 0.9, Some("alpha")),
            search_hit("b", 0.8, Some("beta")),
        ];
        let indexed = VectorSyncCoordinator::rerank_candidates(&head, "content");
        let original = hit_ids(&head);
        let restored = VectorSyncCoordinator::rerank_with_provider(
            &provider,
            &runtime_config(),
            "q",
            &indexed,
            head,
        )
        .await
        .expect_err("mismatch keeps original");
        assert_eq!(hit_ids(&restored), original);
    }

    #[tokio::test]
    async fn textless_hits_sink_in_recall_order() {
        let provider = ScriptedRerank::ordered();
        let head = vec![
            search_hit("a", 0.9, Some("alpha")),
            search_hit("b", 0.8, None),
            search_hit("c", 0.7, Some("gamma")),
            search_hit("d", 0.6, None),
        ];
        let indexed = VectorSyncCoordinator::rerank_candidates(&head, "content");
        let reordered = VectorSyncCoordinator::rerank_with_provider(
            &provider,
            &runtime_config(),
            "q",
            &indexed,
            head,
        )
        .await
        .expect("partial coverage succeeds");
        assert_eq!(
            hit_ids(&reordered),
            vec![
                "a".to_string(),
                "c".to_string(),
                "b".to_string(),
                "d".to_string()
            ]
        );
    }

    fn runtime_config() -> llm_rerank::RerankRuntimeConfig {
        llm_rerank::RerankRuntimeConfig::default()
    }

    fn bare_coordinator() -> (tempfile::TempDir, VectorSyncCoordinator) {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = Arc::new(simvec::LocalVectorEngine::open(dir.path()).expect("local engine"));
        let coordinator = VectorSyncCoordinator::new_without_embedding(
            crate::backend::VectorBackend::from_local_arc(engine),
            tokio::runtime::Handle::current(),
        );
        (dir, coordinator)
    }

    #[tokio::test]
    async fn skips_without_service_text_or_coverage() {
        let (_dir, coordinator) = bare_coordinator();
        assert_eq!(coordinator.rerank_recall_window(Some("q")), 0);
        assert_eq!(coordinator.rerank_recall_window(None), 0);

        let hits = vec![
            search_hit("a", 0.9, Some("alpha")),
            search_hit("b", 0.8, Some("beta")),
        ];
        let kept = coordinator
            .maybe_rerank(None, "content", hits.clone())
            .await;
        assert_eq!(hit_ids(&kept), hit_ids(&hits));

        let single = vec![search_hit("a", 0.9, Some("alpha"))];
        let kept = coordinator
            .maybe_rerank(Some("q"), "content", single.clone())
            .await;
        assert_eq!(hit_ids(&kept), hit_ids(&single));
    }

    #[tokio::test]
    async fn recall_window_opens_with_attached_service() {
        let (_dir, coordinator) = bare_coordinator();
        let provider = llm_rerank::CohereRerankProvider::new(llm_rerank::RerankConfig::new(
            "http://localhost:9/rerank",
            "test",
        ))
        .expect("client builds without I/O");
        let config = linkrs_config::VectorRerankConfig {
            endpoint: llm_rerank::RerankConfig::new("http://localhost:9/rerank", "test"),
            max_candidates: 20,
            fusion: llm_rerank::RerankFusionStrategy::default(),
            text_field: None,
        };
        let coordinator = coordinator.with_rerank_service(std::sync::Arc::new(provider), config);
        assert_eq!(coordinator.rerank_recall_window(Some("q")), 20);
        assert_eq!(coordinator.rerank_recall_window(None), 0);

        let single = vec![search_hit("a", 0.9, Some("alpha"))];
        let kept = coordinator
            .maybe_rerank(Some("q"), "content", single.clone())
            .await;
        assert_eq!(hit_ids(&kept), hit_ids(&single));
    }

    #[tokio::test]
    async fn search_truncates_rerank_widened_recall() {
        let (_dir, coordinator) = bare_coordinator();
        coordinator
            .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
            .await
            .expect("index created");
        let inserts: Vec<VectorChangeContext> = (0..5)
            .map(|i| {
                VectorChangeContext::new(
                    1,
                    "user",
                    "embedding",
                    VectorChangeType::Insert,
                    VectorPointData {
                        id: format!("p{i}"),
                        vector: vec![1.0, i as f32, 0.5, 0.25],
                        payload: HashMap::new(),
                    },
                )
            })
            .collect();
        coordinator
            .on_vector_change_batch(inserts)
            .await
            .expect("delivery");

        let provider = llm_rerank::CohereRerankProvider::new(llm_rerank::RerankConfig::new(
            "http://localhost:9/rerank",
            "test",
        ))
        .expect("client builds without I/O");
        let config = linkrs_config::VectorRerankConfig {
            endpoint: llm_rerank::RerankConfig::new("http://localhost:9/rerank", "test"),
            max_candidates: 20,
            fusion: llm_rerank::RerankFusionStrategy::default(),
            text_field: None,
        };
        let coordinator = coordinator.with_rerank_service(std::sync::Arc::new(provider), config);

        let mut options = SearchOptions::new(1, "user", "embedding", vec![1.0, 0.0, 0.5, 0.25], 3);
        options.query_text = Some("q".to_string());
        let results = coordinator
            .search_with_options(options)
            .await
            .expect("search");
        assert_eq!(
            results.len(),
            3,
            "rerank-widened recall truncates back to the requested limit"
        );
    }
}

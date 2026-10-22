//! Vector backend abstraction.
//!
//! Engines plug in through the object-safe [`VectorEngineHandle`] trait;
//! [`VectorBackend`] is a thin cloneable handle around an
//! `Arc<dyn VectorEngineHandle>` chosen once at construction time. Adding a
//! new backend means implementing the trait — no dispatch-site changes.
//!
//! The local engine is synchronous; the Qdrant handle delegates to the
//! async network client.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use simvec::{
    CollectionConfig, HealthStatus, IndexMetadata, LocalVectorEngine, Payload, PayloadSchemaType,
    SearchQuery, SearchResult, VectorFilter, VectorPoint,
};

#[cfg(feature = "vector-qdrant")]
use vector_client::VectorManager;

use super::vector_error::{VectorCoordinatorError, VectorCoordinatorResult, VectorError};

/// Engine-neutral operations every vector backend must provide.
///
/// The sync layer (coordinator, index manager, rebuild, delivery) only ever
/// talks to this trait; concrete engines implement it behind an
/// `Arc`, so [`VectorBackend`] construction is the single place a backend
/// is selected.
#[async_trait]
pub trait VectorEngineHandle: Send + Sync + fmt::Debug + 'static {
    /// Short engine identifier (`local` / `qdrant`).
    fn engine_name(&self) -> &'static str;

    /// Whether the engine is currently unavailable (e.g. disabled Qdrant).
    fn is_disabled(&self) -> bool;

    /// Engine health status.
    async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus>;

    /// Normalize a requested collection config to the effective config the
    /// backend will actually store, so idempotency checks compare stored
    /// against stored-equivalent instead of false-conflicting.
    fn effective_collection_config(&self, config: &CollectionConfig) -> CollectionConfig;

    /// The underlying local engine, when this handle wraps one.
    fn local_engine(&self) -> Option<Arc<LocalVectorEngine>> {
        None
    }

    /// The underlying Qdrant manager, when this handle wraps one.
    #[cfg(feature = "vector-qdrant")]
    fn qdrant_manager(&self) -> Option<Arc<VectorManager>> {
        None
    }

    // ---- collection management ----

    /// Create a collection. Fails if it already exists.
    async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()>;

    /// Create a payload index for filter acceleration.
    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()>;

    /// Whether a collection exists.
    fn index_exists(&self, name: &str) -> bool;

    /// Atomically promote a temp collection over its live collection,
    /// retaining the previous live directory as `live + backup_suffix`.
    async fn promote_temp_collection(
        &self,
        live: &str,
        temp: &str,
        backup_suffix: &str,
    ) -> VectorCoordinatorResult<()>;

    /// Reverse recovery after a bad publish: restore the newest backup over
    /// the live collection.
    async fn restore_promote_backup(&self, live: &str) -> VectorCoordinatorResult<String>;

    /// Names of all physical collections, including rebuild temps and
    /// publish backups.
    async fn list_collections(&self) -> VectorCoordinatorResult<Vec<String>>;

    /// Collection metadata, or `None` when the collection does not exist.
    fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata>;

    /// Drop a collection.
    async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()>;

    // ---- point access ----

    /// Fetch a single point by id.
    async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>>;

    /// Number of vectors in a collection.
    async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64>;

    // ---- mutations ----

    /// Upsert a point.
    async fn upsert(&self, collection: &str, point: VectorPoint) -> VectorCoordinatorResult<()>;

    /// Upsert a batch of points.
    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()>;

    /// Delete a point by id.
    async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()>;

    /// Delete a batch of points by id.
    async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()>;

    /// Delete every point matching `filter`.
    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()>;

    /// Set/replace payload keys for the given points.
    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> VectorCoordinatorResult<()>;

    /// Merge the given fields into the payload of the given points.
    async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> VectorCoordinatorResult<()>;

    /// Remove specific keys from the payload of the given points.
    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> VectorCoordinatorResult<()>;

    /// Paginated scan over points in a collection.
    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<(Vec<VectorPoint>, Option<String>)>;

    /// Delete a payload index.
    async fn delete_payload_index(
        &self,
        collection: &str,
        field: &str,
    ) -> VectorCoordinatorResult<()>;

    /// List payload indexes.
    async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> VectorCoordinatorResult<Vec<(String, PayloadSchemaType)>>;

    // ---- queries ----

    /// Full search.
    async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>>;

    /// Streaming search: yields results one by one.
    ///
    /// Engines without native streaming buffer a full search and stream the
    /// results; the boxed stream keeps the trait object-safe.
    async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>>,
    >;

    /// Streaming scroll: yields points one by one via paginated scroll.
    async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    >;
}

/// [`VectorEngineHandle`] implementation wrapping the built-in
/// [`LocalVectorEngine`].
#[derive(Debug)]
pub struct LocalEngineHandle(Arc<LocalVectorEngine>);

impl LocalEngineHandle {
    /// Wrap an already-allocated local engine.
    pub fn new(engine: Arc<LocalVectorEngine>) -> Self {
        Self(engine)
    }
}

#[async_trait]
impl VectorEngineHandle for LocalEngineHandle {
    fn engine_name(&self) -> &'static str {
        "local"
    }

    fn is_disabled(&self) -> bool {
        false
    }

    async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus> {
        Ok(HealthStatus::healthy("simvec", env!("CARGO_PKG_VERSION")))
    }

    fn effective_collection_config(&self, config: &CollectionConfig) -> CollectionConfig {
        let engine = &self.0;
        let mut effective = config.clone();
        match effective
            .index_type
            .unwrap_or(simvec::types::IndexType::HNSW)
        {
            simvec::types::IndexType::HNSW => {
                if effective.hnsw_config.is_none() {
                    effective.hnsw_config = engine
                        .default_hnsw_config()
                        .or(Some(simvec::types::HnswConfig::default()));
                }
                effective.ivf_config = None;
            }
            simvec::types::IndexType::IVF => {
                if effective.ivf_config.is_none() {
                    effective.ivf_config = engine.default_ivf_config();
                }
                effective.hnsw_config = None;
            }
            simvec::types::IndexType::FLAT => {
                effective.hnsw_config = None;
                effective.ivf_config = None;
            }
        }
        if effective.quantization_config.is_none() {
            effective.quantization_config = engine.default_quantization_config();
        }
        effective.index_type = Some(
            effective
                .index_type
                .unwrap_or(simvec::types::IndexType::HNSW),
        );
        let drop_quantization = effective
            .quantization_config
            .as_ref()
            .is_some_and(|quantization| !quantization.enabled);
        if drop_quantization {
            effective.quantization_config = None;
        }
        effective
    }

    fn local_engine(&self) -> Option<Arc<LocalVectorEngine>> {
        Some(Arc::clone(&self.0))
    }

    async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .create_collection(name, config)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .create_payload_index(collection, field, schema)
            .map_err(VectorError::from)?;
        Ok(())
    }

    fn index_exists(&self, name: &str) -> bool {
        self.0.collection_exists(name)
    }

    async fn promote_temp_collection(
        &self,
        live: &str,
        temp: &str,
        backup_suffix: &str,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .promote_temp_collection(live, temp, backup_suffix)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn restore_promote_backup(&self, live: &str) -> VectorCoordinatorResult<String> {
        self.0
            .restore_promote_backup(live)
            .map_err(VectorError::from)
            .map_err(VectorCoordinatorError::Vector)
    }

    async fn list_collections(&self) -> VectorCoordinatorResult<Vec<String>> {
        Ok(self.0.list_collections())
    }

    fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        let config = self.0.collection_config(name).ok().flatten()?;
        let vector_count = self.0.count(name).ok()?;
        Some(IndexMetadata {
            name: name.to_string(),
            config,
            created_at: chrono::Utc::now(),
            vector_count,
            index_name: None,
        })
    }

    async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        self.0.delete_collection(name).map_err(VectorError::from)?;
        Ok(())
    }

    async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        Ok(self
            .0
            .get(collection, point_id)
            .map_err(VectorError::from)?)
    }

    async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        Ok(self.0.count(collection).map_err(VectorError::from)?)
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> VectorCoordinatorResult<()> {
        self.0
            .upsert(collection, point)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .upsert_batch(collection, &points)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        self.0
            .delete(collection, point_id)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        let ids: Vec<String> = point_ids.iter().map(|s| s.to_string()).collect();
        self.0
            .delete_batch(collection, &ids)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .delete_by_filter(collection, &filter)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> VectorCoordinatorResult<()> {
        for id in &point_ids {
            self.0
                .set_payload(collection, id, payload.clone())
                .map_err(VectorError::from)?;
        }
        Ok(())
    }

    async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> VectorCoordinatorResult<()> {
        for id in &point_ids {
            self.0
                .set_payload_fields(collection, id, fields.clone())
                .map_err(VectorError::from)?;
        }
        Ok(())
    }

    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> VectorCoordinatorResult<()> {
        let owned_keys: Vec<String> = keys.iter().map(|s| s.to_string()).collect();
        for id in &point_ids {
            self.0
                .delete_payload(collection, id, owned_keys.clone())
                .map_err(VectorError::from)?;
        }
        Ok(())
    }

    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<(Vec<VectorPoint>, Option<String>)> {
        Ok(self
            .0
            .scroll(collection, limit, offset, with_payload, with_vector)
            .map_err(VectorError::from)?)
    }

    async fn delete_payload_index(
        &self,
        collection: &str,
        field: &str,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .delete_payload_index(collection, field)
            .map(|_| ())
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> VectorCoordinatorResult<Vec<(String, PayloadSchemaType)>> {
        Ok(self
            .0
            .list_payload_indexes(collection)
            .map_err(VectorError::from)?)
    }

    async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        Ok(self
            .0
            .search(collection, &query)
            .map_err(VectorError::from)?)
    }

    async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>>,
    > {
        let results = self
            .0
            .search(collection, &query)
            .map_err(VectorError::from)?;
        let stream = futures::stream::iter(results.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }

    async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    > {
        let mut offset: Option<String> = None;
        let mut all_points: Vec<VectorPoint> = Vec::new();
        loop {
            let (points, next) = self
                .0
                .scroll(
                    collection,
                    batch_size,
                    offset.as_deref(),
                    with_payload,
                    with_vector,
                )
                .map_err(VectorError::from)?;
            let is_last = next.is_none();
            all_points.extend(points);
            offset = next;
            if is_last || offset.is_none() {
                break;
            }
        }
        let stream = futures::stream::iter(all_points.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

/// [`VectorEngineHandle`] implementation wrapping the remote Qdrant client.
#[cfg(feature = "vector-qdrant")]
#[derive(Debug)]
pub struct QdrantEngineHandle(Arc<VectorManager>);

#[cfg(feature = "vector-qdrant")]
impl QdrantEngineHandle {
    /// Wrap an already-allocated Qdrant manager.
    pub fn new(manager: Arc<VectorManager>) -> Self {
        Self(manager)
    }
}

#[cfg(feature = "vector-qdrant")]
#[async_trait]
impl VectorEngineHandle for QdrantEngineHandle {
    fn engine_name(&self) -> &'static str {
        "qdrant"
    }

    fn is_disabled(&self) -> bool {
        self.0.engine().name() == "disabled"
    }

    async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus> {
        Ok(self.0.engine().health_check().await?)
    }

    fn effective_collection_config(&self, config: &CollectionConfig) -> CollectionConfig {
        // Remote backends store the request as given.
        config.clone()
    }

    fn qdrant_manager(&self) -> Option<Arc<VectorManager>> {
        Some(Arc::clone(&self.0))
    }

    async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        self.0.create_index(name, config.clone()).await?;
        Ok(())
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .engine()
            .create_payload_index(collection, field, schema)
            .await?;
        Ok(())
    }

    fn index_exists(&self, name: &str) -> bool {
        self.0.index_exists(name)
    }

    async fn promote_temp_collection(
        &self,
        _live: &str,
        _temp: &str,
        _backup_suffix: &str,
    ) -> VectorCoordinatorResult<()> {
        Err(VectorCoordinatorError::Vector(VectorError::ConfigError(
            "qdrant promote uses manager-level logical remap, not physical rename".to_string(),
        )))
    }

    async fn restore_promote_backup(&self, _live: &str) -> VectorCoordinatorResult<String> {
        Err(VectorCoordinatorError::Vector(VectorError::ConfigError(
            "qdrant backup restore is a manager-level logical remap".to_string(),
        )))
    }

    async fn list_collections(&self) -> VectorCoordinatorResult<Vec<String>> {
        self.0
            .engine()
            .list_collections()
            .await
            .map_err(VectorError::from)
            .map_err(VectorCoordinatorError::Vector)
    }

    fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        self.0.get_index_metadata(name)
    }

    async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        self.0.drop_index(name).await?;
        Ok(())
    }

    async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        Ok(self.0.get(collection, point_id).await?)
    }

    async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        Ok(self.0.count(collection).await?)
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> VectorCoordinatorResult<()> {
        self.0.upsert(collection, point).await?;
        Ok(())
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        self.0.upsert_batch(collection, points).await?;
        Ok(())
    }

    async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        self.0.delete(collection, point_id).await?;
        Ok(())
    }

    async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        self.0.delete_batch(collection, point_ids.to_vec()).await?;
        Ok(())
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        self.0.delete_by_filter(collection, filter).await?;
        Ok(())
    }

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .engine()
            .set_payload(collection, point_ids, payload)
            .await?;
        Ok(())
    }

    async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .engine()
            .set_payload_fields(collection, point_ids, fields)
            .await?;
        Ok(())
    }

    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .engine()
            .delete_payload(collection, point_ids, keys)
            .await?;
        Ok(())
    }

    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<(Vec<VectorPoint>, Option<String>)> {
        Ok(self
            .0
            .engine()
            .scroll(collection, limit, offset, with_payload, with_vector)
            .await
            .map_err(VectorError::from)?)
    }

    async fn delete_payload_index(
        &self,
        collection: &str,
        field: &str,
    ) -> VectorCoordinatorResult<()> {
        self.0
            .engine()
            .delete_payload_index(collection, field)
            .await?;
        Ok(())
    }

    async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> VectorCoordinatorResult<Vec<(String, PayloadSchemaType)>> {
        Ok(self
            .0
            .engine()
            .list_payload_indexes(collection)
            .await
            .map_err(VectorError::from)?)
    }

    async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        Ok(self.0.search(collection, query).await?)
    }

    async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>>,
    > {
        // Prefer native streaming when the gRPC engine is in use; fall back
        // to unary search + iter for HTTP or disabled.
        let results = self.0.search(collection, query).await?;
        let stream = futures::stream::iter(results.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }

    async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    > {
        let mut offset: Option<String> = None;
        let mut all_points: Vec<VectorPoint> = Vec::new();
        loop {
            let (points, next) = self
                .0
                .engine()
                .scroll(
                    collection,
                    batch_size,
                    offset.as_deref(),
                    with_payload,
                    with_vector,
                )
                .await
                .map_err(VectorError::from)?;
            let is_last = next.is_none();
            all_points.extend(points);
            offset = next;
            if is_last || offset.is_none() {
                break;
            }
        }
        let stream = futures::stream::iter(all_points.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

/// Delivery policy that varies by backend.
///
/// Unified outbox chain (`WAL → SQLite → claim/ack`) stays backend-agnostic;
/// only batch, concurrency and retry tuning branch here. Created once at
/// startup via `BackendDeliveryPolicy::from_config` and injected into
/// `SyncManager`, so `manager.rs` does not scatter `if is_local` checks.
/// This is the single entry point for delivery tuning; per-target overrides
/// are not supported.
#[derive(Debug, Clone)]
pub struct BackendDeliveryPolicy {
    /// Human readable name (`local` / `qdrant`).
    pub policy_name: String,
    /// Max events claimed per delivery round per target.
    pub batch_size: usize,
    /// Lease duration in milliseconds for `claim_next`.
    pub lease_ms: u64,
    /// Maximum retry attempts before dead-letter.
    pub max_retries: u64,
    /// Concurrent `claim → apply → ack` workers per target.
    pub max_concurrency: usize,
}

impl Default for BackendDeliveryPolicy {
    fn default() -> Self {
        Self::local_default()
    }
}

impl BackendDeliveryPolicy {
    /// Policy tuned for the in-process `LocalVectorEngine`.
    pub fn local_default() -> Self {
        Self {
            policy_name: "local".to_string(),
            batch_size: 128,
            lease_ms: 30_000,
            max_retries: 16,
            max_concurrency: 1,
        }
    }

    /// Policy tuned for the remote Qdrant service.
    pub fn qdrant_default() -> Self {
        Self {
            policy_name: "qdrant".to_string(),
            batch_size: 512,
            lease_ms: 60_000,
            max_retries: 32,
            max_concurrency: 4,
        }
    }

    /// Build a policy from the global vector configuration.
    ///
    /// Single entry point for delivery tuning. Fulltext delivery reuses the
    /// sync-layer failure policy; vector batch tuning branches here only.
    pub fn from_config(config: &linkrs_config::VectorConfig) -> Self {
        match config.engine {
            linkrs_config::VectorEngineKind::Local => Self::local_default(),
            #[cfg(feature = "vector-qdrant")]
            linkrs_config::VectorEngineKind::Qdrant => Self::qdrant_default(),
        }
    }

    /// Map to the consumer config used by `SyncManager::retry_outbox_sync`.
    pub fn to_consumer_config(&self) -> crate::manager::OutboxConsumerConfig {
        crate::manager::OutboxConsumerConfig {
            consumer_id: format!("sync-manager-{}-{}", self.policy_name, uuid::Uuid::new_v4()),
            batch_size: self.batch_size,
            lease_duration_ms: self.lease_ms,
            max_retries: self.max_retries,
            max_concurrency: self.max_concurrency,
        }
    }
}

/// Map the pure-data config surface to the client crate's config.
///
/// `linkrs-config` carries no engine implementation, so the conversion from
/// its TOML-shaped [`linkrs_config::QdrantConfig`] to the client's
/// `VectorClientConfig` lives here, at the layer that depends on both
/// crates. It is a free function rather than a `From` impl because both
/// types are foreign here.
#[cfg(feature = "vector-qdrant")]
pub fn qdrant_client_config(
    config: &linkrs_config::QdrantConfig,
) -> vector_client::VectorClientConfig {
    vector_client::VectorClientConfig {
        enabled: config.enabled,
        engine: vector_client::EngineType::Qdrant,
        connection: vector_client::ConnectionConfig {
            host: config.connection.host.clone(),
            port: config.connection.port,
            use_tls: config.connection.use_tls,
            api_key: config.connection.api_key.clone(),
            connect_timeout_secs: config.connection.connect_timeout_secs,
            http_port: config.connection.http_port,
            transport: match config.connection.transport {
                linkrs_config::QdrantTransport::Grpc => vector_client::QdrantTransport::Grpc,
                linkrs_config::QdrantTransport::Http => vector_client::QdrantTransport::Http,
            },
        },
        timeout: vector_client::TimeoutConfig::new(
            config.timeout.request_timeout_secs,
            config.timeout.search_timeout_secs,
            config.timeout.upsert_timeout_secs,
        ),
        embedding: {
            #[cfg(feature = "embedding")]
            {
                config.embedding.clone()
            }
            #[cfg(not(feature = "embedding"))]
            {
                None
            }
        },
    }
}

/// The active vector engine handle.
///
/// A cloneable wrapper around one `Arc<dyn VectorEngineHandle>` chosen at
/// construction time (see [`VectorBackend::from_config`]); cloning bumps the
/// reference count on the inner engine handle. All operations delegate to
/// the trait, so new engines plug in without touching dispatch sites.
pub struct VectorBackend {
    engine: Arc<dyn VectorEngineHandle>,
    /// Engine-level metrics decorator; `engine` points at it when present.
    metrics: Option<Arc<crate::metrics_handle::MetricsEngineHandle>>,
}

impl Clone for VectorBackend {
    fn clone(&self) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
            metrics: self.metrics.clone(),
        }
    }
}

impl fmt::Debug for VectorBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VectorBackend").field(&self.engine).finish()
    }
}

impl VectorBackend {
    /// Wrap a [`LocalVectorEngine`] in the [`VectorBackend`] handle.
    pub fn local(engine: LocalVectorEngine) -> Self {
        Self::from_local_arc(Arc::new(engine))
    }

    /// Wrap an already-allocated `Arc<LocalVectorEngine>`.
    pub fn from_local_arc(arc: Arc<LocalVectorEngine>) -> Self {
        let metrics = Arc::new(crate::metrics_handle::MetricsEngineHandle::new(Arc::new(
            LocalEngineHandle::new(arc),
        )));
        Self {
            engine: metrics.clone(),
            metrics: Some(metrics),
        }
    }

    /// Wrap a [`VectorManager`] in the [`VectorBackend`] handle.
    #[cfg(feature = "vector-qdrant")]
    pub fn qdrant(manager: Arc<VectorManager>) -> Self {
        let metrics = Arc::new(crate::metrics_handle::MetricsEngineHandle::new(Arc::new(
            QdrantEngineHandle::new(manager),
        )));
        Self {
            engine: metrics.clone(),
            metrics: Some(metrics),
        }
    }

    /// Late-bind the shared observability handle so engine-level operations
    /// (search, create/drop, batch writes) report through the same pipeline
    /// as the fulltext metrics decorator.
    pub fn set_stats_manager(&self, stats_manager: Arc<linkrs_metrics::StatsManager>) {
        if let Some(metrics) = self.metrics.as_ref() {
            metrics.set_stats_manager(stats_manager);
        }
    }

    /// Whether this backend is the built-in local engine.
    pub fn is_local(&self) -> bool {
        self.engine.engine_name() == "local"
    }

    /// Whether the engine is currently unavailable (e.g. disabled Qdrant).
    pub fn is_disabled(&self) -> bool {
        self.engine.is_disabled()
    }

    /// Build the engine selected by `config`.
    ///
    /// Only handles the local engine synchronously. For async Qdrant
    /// construction, use [`Self::from_config_async`].
    pub fn from_config(config: &linkrs_config::VectorConfig) -> VectorCoordinatorResult<Self> {
        match config.engine {
            linkrs_config::VectorEngineKind::Local => {
                let data_dir = config
                    .local
                    .data_dir
                    .as_deref()
                    .unwrap_or_else(|| std::path::Path::new("vector"));
                let engine = LocalVectorEngine::open(data_dir).map_err(|e| {
                    VectorCoordinatorError::Vector(VectorError::Internal(format!(
                        "failed to open local vector engine: {e}"
                    )))
                })?;
                Ok(Self::local(engine))
            }
            #[cfg(feature = "vector-qdrant")]
            linkrs_config::VectorEngineKind::Qdrant => {
                Err(VectorCoordinatorError::Vector(VectorError::ConfigError(
                    "Qdrant engine requires async construction; use from_config_async".to_string(),
                )))
            }
        }
    }

    /// Async variant that also handles the Qdrant backend.
    #[cfg(feature = "vector-qdrant")]
    pub async fn from_config_async(
        config: &linkrs_config::VectorConfig,
    ) -> VectorCoordinatorResult<Self> {
        match config.engine {
            linkrs_config::VectorEngineKind::Local => Self::from_config(config),
            linkrs_config::VectorEngineKind::Qdrant => {
                let client_config = qdrant_client_config(&config.qdrant);
                let manager = Arc::new(
                    VectorManager::new(client_config)
                        .await
                        .map_err(|e| VectorCoordinatorError::Vector(VectorError::from(e)))?,
                );
                Ok(Self::qdrant(manager))
            }
        }
    }

    #[cfg(not(feature = "vector-qdrant"))]
    pub async fn from_config_async(
        config: &linkrs_config::VectorConfig,
    ) -> VectorCoordinatorResult<Self> {
        Self::from_config(config)
    }

    /// Engine health status.
    pub async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus> {
        self.engine.health_check().await
    }

    /// Access the underlying local engine, if present.
    pub fn as_local(&self) -> Option<Arc<LocalVectorEngine>> {
        self.engine.local_engine()
    }

    /// Access the underlying `VectorManager`, if this is a Qdrant backend.
    #[cfg(feature = "vector-qdrant")]
    pub fn as_qdrant_manager(&self) -> Option<Arc<VectorManager>> {
        self.engine.qdrant_manager()
    }

    /// Normalize a requested collection config to the effective config the
    /// backend will actually store.
    pub fn effective_collection_config(&self, config: &CollectionConfig) -> CollectionConfig {
        self.engine.effective_collection_config(config)
    }

    // ---- collection management ----

    /// Create a collection. Fails if it already exists.
    pub async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        self.engine.create_index(name, config).await
    }

    /// Create a payload index for filter acceleration.
    pub async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()> {
        self.engine
            .create_payload_index(collection, field, schema)
            .await
    }

    /// Whether a collection exists.
    pub fn index_exists(&self, name: &str) -> bool {
        self.engine.index_exists(name)
    }

    /// Temp rebuild collection name for one live collection and generation.
    ///
    /// Field granularity temps are `{live}.rebuild-{generation}`; Space
    /// granularity appends the logical owner (`{live}.rebuild-{gen}__{tag}_{field}`)
    /// so sibling slices never share a temp. Temps never reuse the live name
    /// plus the live `group_id`: that scheme cannot be told apart from live
    /// data during startup recovery.
    pub fn temp_collection_name(live: &str, generation: u64, owner_suffix: Option<&str>) -> String {
        match owner_suffix {
            Some(owner) => format!("{live}.rebuild-{generation}__{owner}"),
            None => format!("{live}.rebuild-{generation}"),
        }
    }

    /// Whether a physical collection name is a rebuild temp.
    pub fn is_temp_collection(name: &str) -> bool {
        name.contains(".rebuild-")
    }

    /// Split a temp name into `(live_name, generation)`. Returns `None` for
    /// non-temp names or unparseable generations.
    pub fn parse_temp_collection(name: &str) -> Option<(String, u64)> {
        let (live, rest) = name.split_once(".rebuild-")?;
        let gen_text = rest.split("__").next().unwrap_or(rest);
        Some((live.to_string(), gen_text.parse::<u64>().ok()?))
    }

    /// Backup suffix for the retained pre-publish live collection.
    pub fn promote_backup_suffix(generation: u64) -> String {
        format!(".old-{generation}")
    }

    /// Split a backup name into `(live_name, generation)`.
    pub fn parse_backup_collection(name: &str) -> Option<(String, u64)> {
        let (live, rest) = name.split_once(".old-")?;
        Some((live.to_string(), rest.parse::<u64>().ok()?))
    }

    /// Create an empty temp collection with the given effective config.
    /// Fails when a collection of that name already exists; callers drop
    /// leftovers from the same generation first for idempotent retry.
    pub async fn create_temp_collection(
        &self,
        temp: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        self.create_index(temp, config).await
    }

    /// Drop a temp collection. Succeeds when it is already gone so failed
    /// rebuilds and startup recovery stay idempotent.
    pub async fn drop_temp_collection(&self, temp: &str) -> VectorCoordinatorResult<()> {
        if !self.index_exists(temp) {
            return Ok(());
        }
        self.delete_collection(temp).await
    }

    /// Atomically promote a temp collection over its live collection,
    /// retaining the previous live directory as `live + backup_suffix`
    /// (one backup generation; older ones are pruned).
    ///
    /// Local backend only: the engine renames directories. The remote
    /// backend has no atomic rename, so Qdrant publishes use a
    /// manager-level logical remap instead (see `VectorIndexManager`).
    pub async fn promote_temp_collection(
        &self,
        live: &str,
        temp: &str,
        backup_suffix: &str,
    ) -> VectorCoordinatorResult<()> {
        self.engine
            .promote_temp_collection(live, temp, backup_suffix)
            .await
    }

    /// Reverse recovery after a bad Field-granularity publish: restore the
    /// newest `.old-*` backup over the live collection.
    pub async fn restore_promote_backup(&self, live: &str) -> VectorCoordinatorResult<String> {
        self.engine.restore_promote_backup(live).await
    }

    /// Names of all physical collections, including rebuild temps and
    /// publish backups. Used by startup recovery to find orphaned temps
    /// without adopting them as logical indexes.
    pub async fn list_collections(&self) -> VectorCoordinatorResult<Vec<String>> {
        self.engine.list_collections().await
    }

    /// Temp rebuild collections (`*.rebuild-*`) currently present.
    pub async fn list_temp_collections(&self) -> VectorCoordinatorResult<Vec<String>> {
        Ok(self
            .list_collections()
            .await?
            .into_iter()
            .filter(|name| Self::is_temp_collection(name))
            .collect())
    }

    /// Collection metadata, or `None` when the collection does not exist.
    pub fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        self.engine.get_index_metadata(name)
    }

    /// Drop a collection.
    pub async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        self.engine.delete_collection(name).await
    }

    /// Fetch a single point by id.
    pub async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        self.engine.get_vector(collection, point_id).await
    }

    /// Number of vectors in a collection.
    pub async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        self.engine.count(collection).await
    }

    // ---- mutations ----

    /// Upsert a point.
    pub async fn upsert(
        &self,
        collection: &str,
        point: VectorPoint,
    ) -> VectorCoordinatorResult<()> {
        self.engine.upsert(collection, point).await
    }

    /// Upsert a batch of points.
    pub async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        self.engine.upsert_batch(collection, points).await
    }

    /// Delete a point by id.
    pub async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        self.engine.delete(collection, point_id).await
    }

    /// Delete a batch of points by id.
    pub async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        self.engine.delete_batch(collection, point_ids).await
    }

    /// Delete every point matching `filter`.
    pub async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        self.engine.delete_by_filter(collection, filter).await
    }

    /// Set/replace payload keys for the given points.
    pub async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> VectorCoordinatorResult<()> {
        self.engine
            .set_payload(collection, point_ids, payload)
            .await
    }

    /// Merge the given fields into the payload of the given points.
    pub async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> VectorCoordinatorResult<()> {
        self.engine
            .set_payload_fields(collection, point_ids, fields)
            .await
    }

    /// Remove specific keys from the payload of the given points.
    pub async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> VectorCoordinatorResult<()> {
        self.engine
            .delete_payload(collection, point_ids, keys)
            .await
    }

    /// Paginated scan over points in a collection.
    pub async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<(Vec<VectorPoint>, Option<String>)> {
        self.engine
            .scroll(collection, limit, offset, with_payload, with_vector)
            .await
    }

    /// Delete a payload index.
    pub async fn delete_payload_index(
        &self,
        collection: &str,
        field: &str,
    ) -> VectorCoordinatorResult<()> {
        self.engine.delete_payload_index(collection, field).await
    }

    /// List payload indexes.
    pub async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> VectorCoordinatorResult<Vec<(String, PayloadSchemaType)>> {
        self.engine.list_payload_indexes(collection).await
    }

    // ---- queries ----

    /// Full search.
    pub async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        self.engine.search(collection, query).await
    }

    /// Streaming search: yields results one by one.
    pub async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>>,
    > {
        self.engine.search_stream(collection, query).await
    }

    /// Streaming scroll: yields points one by one via paginated scroll.
    pub async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    > {
        self.engine
            .scroll_stream(collection, batch_size, with_payload, with_vector)
            .await
    }

    /// Whether payload indexes are natively supported by this backend.
    pub fn supports_payload_index(&self) -> bool {
        true
    }

    /// Whether streaming search is available.
    pub fn supports_streaming(&self) -> bool {
        true
    }
}

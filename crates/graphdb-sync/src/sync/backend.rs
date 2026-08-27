//! Vector backend abstraction.
//!
//! The sync layer dispatches to either the built-in local engine or the
//! remote Qdrant client through an enum [`VectorBackend`]. Each variant
//! holds the concrete engine type directly, eliminating dynamic dispatch
//! overhead and `as_any()` downcast.
//!
//! The local engine is synchronous; the Qdrant variant delegates to the
//! async network client.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use vector_search::{
    CollectionConfig, HealthStatus, IndexMetadata, LocalVectorEngine, Payload, PayloadSchemaType,
    SearchQuery, SearchResult, VectorFilter, VectorPoint,
};

#[cfg(feature = "vector-qdrant")]
use vector_client::VectorManager;

use super::vector_error::{VectorCoordinatorError, VectorCoordinatorResult, VectorError};

/// The active vector engine.
///
/// An enum dispatching to one of the supported vector engine backends.
/// Cloning bumps the reference count on the inner engine handle.
pub enum VectorBackend {
    /// Built-in in-process engine (mmap-backed, synchronous I/O).
    Local(Arc<LocalVectorEngine>),
    /// Remote Qdrant service (HTTP or gRPC transport).
    #[cfg(feature = "vector-qdrant")]
    Qdrant(Arc<VectorManager>),
}

impl Clone for VectorBackend {
    fn clone(&self) -> Self {
        match self {
            VectorBackend::Local(engine) => VectorBackend::Local(Arc::clone(engine)),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => VectorBackend::Qdrant(Arc::clone(manager)),
        }
    }
}

impl fmt::Debug for VectorBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VectorBackend::Local(_) => f.debug_tuple("Local").finish(),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(_) => f.debug_tuple("Qdrant").finish(),
        }
    }
}

impl VectorBackend {
    /// Wrap a [`LocalVectorEngine`] in the [`VectorBackend`] handle.
    pub fn local(engine: LocalVectorEngine) -> Self {
        Self::Local(Arc::new(engine))
    }

    /// Wrap an already-allocated `Arc<LocalVectorEngine>`.
    pub fn from_local_arc(arc: Arc<LocalVectorEngine>) -> Self {
        Self::Local(arc)
    }

    /// Wrap a [`VectorManager`] in the [`VectorBackend`] handle.
    #[cfg(feature = "vector-qdrant")]
    pub fn qdrant(manager: Arc<VectorManager>) -> Self {
        Self::Qdrant(manager)
    }

    /// Whether this backend is the built-in local engine.
    pub fn is_local(&self) -> bool {
        matches!(self, VectorBackend::Local(_))
    }

    /// Whether the engine is currently unavailable (e.g. disabled Qdrant).
    pub fn is_disabled(&self) -> bool {
        match self {
            VectorBackend::Local(_) => false,
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.engine().name() == "disabled",
        }
    }

    /// Build the engine selected by `config`.
    ///
    /// Only handles the local engine synchronously. For async Qdrant
    /// construction, use [`Self::from_config_async`].
    pub fn from_config(
        config: &graphdb_config::config::VectorConfig,
    ) -> VectorCoordinatorResult<Self> {
        match config.engine {
            graphdb_config::config::VectorEngineKind::Local => {
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
            graphdb_config::config::VectorEngineKind::Qdrant => {
                Err(VectorCoordinatorError::Vector(VectorError::ConfigError(
                    "Qdrant engine requires async construction; use from_config_async".to_string(),
                )))
            }
            #[cfg(not(feature = "vector-qdrant"))]
            graphdb_config::config::VectorEngineKind::Qdrant => {
                Err(VectorCoordinatorError::Vector(VectorError::ConfigError(
                    "Qdrant engine requested but the `vector-qdrant` feature is not enabled"
                        .to_string(),
                )))
            }
        }
    }

    /// Async variant that also handles the Qdrant backend.
    #[cfg(feature = "vector-qdrant")]
    pub async fn from_config_async(
        config: &graphdb_config::config::VectorConfig,
    ) -> VectorCoordinatorResult<Self> {
        match config.engine {
            graphdb_config::config::VectorEngineKind::Local => Self::from_config(config),
            graphdb_config::config::VectorEngineKind::Qdrant => {
                let client_config = config.qdrant.clone();
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
        config: &graphdb_config::config::VectorConfig,
    ) -> VectorCoordinatorResult<Self> {
        Self::from_config(config)
    }

    /// Engine health status. The local engine is always healthy.
    pub async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus> {
        match self {
            VectorBackend::Local(_) => Ok(HealthStatus::healthy(
                "vector-search",
                env!("CARGO_PKG_VERSION"),
            )),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => Ok(manager.engine().health_check().await?),
        }
    }

    /// Access the underlying local engine, if present.
    pub fn as_local(&self) -> Option<&LocalVectorEngine> {
        match self {
            VectorBackend::Local(engine) => Some(engine.as_ref()),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(_) => None,
        }
    }

    /// Access the underlying `VectorManager`, if this is a Qdrant backend.
    #[cfg(feature = "vector-qdrant")]
    pub fn as_qdrant_manager(&self) -> Option<&VectorManager> {
        match self {
            VectorBackend::Local(_) => None,
            VectorBackend::Qdrant(manager) => Some(manager.as_ref()),
        }
    }

    // ---- collection management ----

    /// Create a collection. Fails if it already exists.
    pub async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .create_collection(name, config)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.create_index(name, config.clone()).await?;
                Ok(())
            }
        }
    }

    /// Create a payload index for filter acceleration.
    pub async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .create_payload_index(collection, field, schema)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager
                    .engine()
                    .create_payload_index(collection, field, schema)
                    .await?;
                Ok(())
            }
        }
    }

    /// Whether a collection exists.
    pub fn index_exists(&self, name: &str) -> bool {
        match self {
            VectorBackend::Local(engine) => engine.collection_exists(name),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.index_exists(name),
        }
    }

    /// Collection metadata, or `None` when the collection does not exist.
    pub fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        match self {
            VectorBackend::Local(engine) => {
                let config = engine.collection_config(name).ok().flatten()?;
                let vector_count = engine.count(name).ok()?;
                Some(IndexMetadata {
                    name: name.to_string(),
                    config,
                    created_at: chrono::Utc::now(),
                    vector_count,
                    index_name: None,
                })
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => manager.get_index_metadata(name),
        }
    }

    /// Drop a collection.
    pub async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine.delete_collection(name).map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.drop_index(name).await?;
                Ok(())
            }
        }
    }

    /// Fetch a single point by id.
    pub async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        match self {
            VectorBackend::Local(engine) => Ok(engine
                .get(collection, point_id)
                .map_err(VectorError::from)?),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => Ok(manager.get(collection, point_id).await?),
        }
    }

    /// Number of vectors in a collection.
    pub async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        match self {
            VectorBackend::Local(engine) => {
                Ok(engine.count(collection).map_err(VectorError::from)?)
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => Ok(manager.count(collection).await?),
        }
    }

    // ---- mutations ----

    /// Upsert a point.
    pub async fn upsert(
        &self,
        collection: &str,
        point: VectorPoint,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .upsert(collection, point)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.upsert(collection, point).await?;
                Ok(())
            }
        }
    }

    /// Upsert a batch of points.
    pub async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .upsert_batch(collection, &points)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.upsert_batch(collection, points).await?;
                Ok(())
            }
        }
    }

    /// Delete a point by id.
    pub async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .delete(collection, point_id)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.delete(collection, point_id).await?;
                Ok(())
            }
        }
    }

    /// Delete a batch of points by id.
    pub async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                let ids: Vec<String> = point_ids.iter().map(|s| s.to_string()).collect();
                engine
                    .delete_batch(collection, &ids)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.delete_batch(collection, point_ids.to_vec()).await?;
                Ok(())
            }
        }
    }

    /// Delete every point matching `filter`.
    pub async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .delete_by_filter(collection, &filter)
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager.delete_by_filter(collection, filter).await?;
                Ok(())
            }
        }
    }

    /// Set/replace payload keys for the given points.
    pub async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                for id in &point_ids {
                    engine
                        .set_payload(collection, id, payload.clone())
                        .map_err(VectorError::from)?;
                }
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager
                    .engine()
                    .set_payload(collection, point_ids, payload)
                    .await?;
                Ok(())
            }
        }
    }

    /// Merge the given fields into the payload of the given points.
    pub async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                for id in &point_ids {
                    engine
                        .set_payload_fields(collection, id, fields.clone())
                        .map_err(VectorError::from)?;
                }
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager
                    .engine()
                    .set_payload_fields(collection, point_ids, fields)
                    .await?;
                Ok(())
            }
        }
    }

    /// Remove specific keys from the payload of the given points.
    pub async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                let owned_keys: Vec<String> = keys.iter().map(|s| s.to_string()).collect();
                for id in &point_ids {
                    engine
                        .delete_payload(collection, id, owned_keys.clone())
                        .map_err(VectorError::from)?;
                }
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager
                    .engine()
                    .delete_payload(collection, point_ids, keys)
                    .await?;
                Ok(())
            }
        }
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
        match self {
            VectorBackend::Local(engine) => Ok(engine
                .scroll(collection, limit, offset, with_payload, with_vector)
                .map_err(VectorError::from)?),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => Ok(manager
                .engine()
                .scroll(collection, limit, offset, with_payload, with_vector)
                .await
                .map_err(VectorError::from)?),
        }
    }

    /// Delete a payload index.
    pub async fn delete_payload_index(
        &self,
        collection: &str,
        field: &str,
    ) -> VectorCoordinatorResult<()> {
        match self {
            VectorBackend::Local(engine) => {
                engine
                    .delete_payload_index(collection, field)
                    .map(|_| ())
                    .map_err(VectorError::from)?;
                Ok(())
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                manager
                    .engine()
                    .delete_payload_index(collection, field)
                    .await?;
                Ok(())
            }
        }
    }

    /// List payload indexes.
    pub async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> VectorCoordinatorResult<Vec<(String, PayloadSchemaType)>> {
        match self {
            VectorBackend::Local(engine) => Ok(engine
                .list_payload_indexes(collection)
                .map_err(VectorError::from)?),
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => Ok(manager
                .engine()
                .list_payload_indexes(collection)
                .await
                .map_err(VectorError::from)?),
        }
    }

    // ---- queries ----

    /// Full search.
    pub async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        match self {
            VectorBackend::Local(engine) => {
                let results = engine
                    .search(collection, &query)
                    .map_err(VectorError::from)?;
                Ok(results)
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                let results = manager.search(collection, query).await?;
                Ok(results)
            }
        }
    }

    /// Streaming search: yields results one by one.
    ///
    /// For the local engine this is implemented by executing a full search
    /// and streaming the buffered results. Remote engines may use true gRPC
    /// streaming when available. The stream is boxed to keep the API
    /// object-safe and feature-unified.
    pub async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>>,
    > {
        match self {
            VectorBackend::Local(engine) => {
                let results = engine
                    .search(collection, &query)
                    .map_err(VectorError::from)?;
                let stream = futures::stream::iter(results.into_iter().map(Ok));
                Ok(Box::pin(stream))
            }
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                // Prefer native streaming when the gRPC engine is in use;
                // fall back to unary search + iter for HTTP or disabled.
                let results = manager.search(collection, query).await?;
                let stream = futures::stream::iter(results.into_iter().map(Ok));
                Ok(Box::pin(stream))
            }
        }
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
        match self {
            VectorBackend::Local(engine) => {
                let mut offset: Option<String> = None;
                let mut all_points: Vec<VectorPoint> = Vec::new();
                loop {
                    let (points, next) = engine
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
            #[cfg(feature = "vector-qdrant")]
            VectorBackend::Qdrant(manager) => {
                let mut offset: Option<String> = None;
                let mut all_points: Vec<VectorPoint> = Vec::new();
                loop {
                    let (points, next) = manager
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
    }

    /// Whether payload indexes are natively supported by this backend.
    ///
    /// The local engine now has real payload indexes (MapIndex / NumericIndex)
    /// backed by `payload_indexes.json`; the remote Qdrant engine always
    /// supports them server-side. This helper allows callers to introspect
    /// capability at runtime instead of via `#[cfg]`.
    pub fn supports_payload_index(&self) -> bool {
        true
    }

    /// Whether streaming search is available.
    ///
    /// Both backends support streaming (local via buffered iter, remote via
    /// gRPC streaming). The method exists so callers can use runtime
    /// detection rather than conditional compilation.
    pub fn supports_streaming(&self) -> bool {
        true
    }
}

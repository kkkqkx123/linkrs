//! Vector backend abstraction.
//!
//! Engines plug in through the object-safe [`VectorEngineHandle`] trait;
//! [`VectorBackend`] is a thin cloneable handle around an
//! `Arc<dyn VectorEngineHandle>` chosen once at construction time. Adding a
//! new backend means implementing the trait — no dispatch-site changes.
//!
//! The local engine is synchronous; the Qdrant handle delegates to the
//! async network client.

use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use simvec::{
    CollectionConfig, HealthStatus, IndexMetadata, LocalVectorEngine, Payload, PayloadSchemaType,
    PointId, SearchQuery, SearchResult, VectorFilter, VectorPoint,
};

#[cfg(feature = "vector-qdrant")]
use vector_client::VectorManager;

#[cfg(feature = "vector-qdrant")]
use super::circuit_breaker::{
    with_circuit_breaker, CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError,
};
#[cfg(feature = "vector-qdrant")]
use super::vector_defaults::CollectionDefaults;
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

    /// Whether the engine runs in-process with no network hop. Delivery
    /// paths use this capability to decide whether a batch may take the
    /// durable-outbox-free fast path; callers must not infer it from the
    /// engine name.
    fn is_in_process(&self) -> bool {
        false
    }

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

    /// Whether this backend can atomically promote a temp collection over
    /// its live collection. Local backends rename directories; remote
    /// backends publish through a logical pointer switch instead.
    fn supports_atomic_promote(&self) -> bool {
        false
    }

    /// Default collection config for a new logical index. Backends decide
    /// their own creation defaults so the index manager never branches on
    /// the engine name for this choice.
    fn default_collection_config(
        &self,
        vector_size: usize,
        distance: simvec::DistanceMetric,
    ) -> CollectionConfig {
        CollectionConfig::new(vector_size, distance)
    }

    /// Whether dropping a logical index should reclaim its space-granularity
    /// slice from the shared collection. Only backends that own physical
    /// slice lifecycle answer true; others keep the shared collection.
    fn owns_space_slice_reclaim(&self) -> bool {
        false
    }

    /// Apply one already-grouped batch of writes and deletes.
    ///
    /// Grouping and payload conversion stay with the index manager; this
    /// is only the execution point so per-engine commit semantics
    /// (grouped commit locally, per-collection writes remotely) do not
    /// leak into the coordinator. The default walks collections one by
    /// one; engines with stronger primitives override it.
    async fn apply_grouped_batch(
        &self,
        upserts: HashMap<String, Vec<VectorPoint>>,
        deletes: HashMap<String, Vec<String>>,
    ) -> VectorCoordinatorResult<()> {
        for (collection, points) in upserts {
            if points.len() == 1 {
                self.upsert(&collection, points.into_iter().next().expect("len checked"))
                    .await?;
            } else if !points.is_empty() {
                self.upsert_batch(&collection, points).await?;
            }
        }
        for (collection, point_ids) in deletes {
            if point_ids.len() == 1 {
                self.delete(&collection, &point_ids[0]).await?;
            } else if !point_ids.is_empty() {
                let refs: Vec<&str> = point_ids.iter().map(|id| id.as_str()).collect();
                self.delete_batch(&collection, &refs).await?;
            }
        }
        Ok(())
    }

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
pub struct LocalEngineHandle {
    engine: Arc<LocalVectorEngine>,
    /// `[vector.limits].default_max_points`; `None` leaves the engine
    /// uncapped.
    max_points: Option<u64>,
}

impl LocalEngineHandle {
    /// Wrap an already-allocated local engine.
    pub fn new(engine: Arc<LocalVectorEngine>) -> Self {
        Self {
            engine,
            max_points: None,
        }
    }

    /// Wrap an already-allocated local engine and apply an admission cap to
    /// collections created without an explicit `max_points`.
    pub fn with_max_points(engine: Arc<LocalVectorEngine>, max_points: u64) -> Self {
        Self {
            engine,
            max_points: (max_points > 0).then_some(max_points),
        }
    }
}

#[async_trait]
impl VectorEngineHandle for LocalEngineHandle {
    fn engine_name(&self) -> &'static str {
        "local"
    }

    fn is_in_process(&self) -> bool {
        true
    }

    async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus> {
        Ok(HealthStatus::healthy("simvec", env!("CARGO_PKG_VERSION")))
    }

    /// Explicitly reject remote-only layout fields instead of silently
    /// ignoring them: a local deployment that sets `shard_number` or a
    /// qdrant storage hint has misunderstood the engine, and a warning at
    /// creation time is the cheapest place to say so.
    fn default_collection_config(
        &self,
        vector_size: usize,
        distance: simvec::DistanceMetric,
    ) -> CollectionConfig {
        let mut config = CollectionConfig::new(vector_size, distance);
        config.max_points = self.max_points;
        config
    }

    fn effective_collection_config(&self, config: &CollectionConfig) -> CollectionConfig {
        if config.shard_number.is_some()
            || config.replication_factor.is_some()
            || config.write_consistency_factor.is_some()
            || config.vectors_on_disk.is_some()
            || config.storage_hints.is_some()
        {
            log::warn!(
                "local vector engine ignores remote-only collection layout fields \
                 (shard_number/replication_factor/write_consistency_factor/vectors_on_disk/\
                 storage_hints); storage layout is governed by [vector.local.storage]"
            );
        }
        let engine = &self.engine;
        let mut effective = config.clone();
        match &mut effective.index {
            simvec::types::IndexConfig::Hnsw(cfg) => {
                if *cfg == simvec::types::HnswConfig::default() {
                    if let Some(default) = engine.default_hnsw_config() {
                        *cfg = default;
                    }
                }
            }
            simvec::types::IndexConfig::Ivf(opt) => {
                if opt.is_none() {
                    if let Some(default) = engine.default_ivf_config() {
                        *opt = Some(default);
                    }
                }
            }
            simvec::types::IndexConfig::Flat => {}
        }
        if effective.quantization_config.is_none() {
            effective.quantization_config = engine.default_quantization_config();
        }
        effective
    }

    fn local_engine(&self) -> Option<Arc<LocalVectorEngine>> {
        Some(Arc::clone(&self.engine))
    }

    async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        self.engine
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
        self.engine
            .create_payload_index(collection, field, schema)
            .map_err(VectorError::from)?;
        Ok(())
    }

    fn index_exists(&self, name: &str) -> bool {
        self.engine.collection_exists(name)
    }

    fn supports_atomic_promote(&self) -> bool {
        true
    }

    fn owns_space_slice_reclaim(&self) -> bool {
        true
    }

    async fn apply_grouped_batch(
        &self,
        upserts: HashMap<String, Vec<VectorPoint>>,
        deletes: HashMap<String, Vec<String>>,
    ) -> VectorCoordinatorResult<()> {
        use simvec::engine::TxnOp;
        let mut ops: Vec<TxnOp> = Vec::new();
        let mut names: Vec<&String> = upserts.keys().chain(deletes.keys()).collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            if let Some(points) = upserts.get(name) {
                for point in points {
                    ops.push(TxnOp::Upsert {
                        collection: name.clone(),
                        point: point.clone(),
                    });
                }
            }
            if let Some(ids) = deletes.get(name) {
                for id in ids {
                    ops.push(TxnOp::Delete {
                        collection: name.clone(),
                        point_id: PointId::from(id.as_str()),
                    });
                }
            }
        }
        if !ops.is_empty() {
            self.engine.apply_txn(ops).map_err(VectorError::from)?;
        }
        Ok(())
    }

    async fn promote_temp_collection(
        &self,
        live: &str,
        temp: &str,
        backup_suffix: &str,
    ) -> VectorCoordinatorResult<()> {
        self.engine
            .promote_temp_collection(live, temp, backup_suffix)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn restore_promote_backup(&self, live: &str) -> VectorCoordinatorResult<String> {
        self.engine
            .restore_promote_backup(live)
            .map_err(VectorError::from)
            .map_err(VectorCoordinatorError::Vector)
    }

    async fn list_collections(&self) -> VectorCoordinatorResult<Vec<String>> {
        Ok(self.engine.list_collections())
    }

    fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        let config = self.engine.collection_config(name).ok().flatten()?;
        let vector_count = self.engine.count(name).ok()?;
        Some(IndexMetadata {
            name: name.to_string(),
            config,
            created_at: chrono::Utc::now(),
            vector_count,
            index_name: None,
        })
    }

    async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        self.engine
            .delete_collection(name)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        let pid = PointId::from(point_id);
        Ok(self
            .engine
            .get(collection, &pid)
            .map_err(VectorError::from)?)
    }

    async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        Ok(self.engine.count(collection).map_err(VectorError::from)?)
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> VectorCoordinatorResult<()> {
        self.engine
            .upsert(collection, point)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        self.engine
            .upsert_batch(collection, &points)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        let pid = PointId::from(point_id);
        self.engine
            .delete(collection, &pid)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        let ids: Vec<PointId> = point_ids.iter().map(|s| PointId::from(*s)).collect();
        self.engine
            .delete_batch(collection, &ids)
            .map_err(VectorError::from)?;
        Ok(())
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        self.engine
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
            let pid = PointId::from(*id);
            self.engine
                .set_payload(collection, &pid, payload.clone())
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
            let pid = PointId::from(*id);
            self.engine
                .set_payload_fields(collection, &pid, fields.clone())
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
            let pid = PointId::from(*id);
            self.engine
                .delete_payload(collection, &pid, owned_keys.clone())
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
        let offset = offset.map(PointId::from);
        let (points, next) = self
            .engine
            .scroll(
                collection,
                limit,
                offset.as_ref(),
                with_payload,
                with_vector,
            )
            .map_err(VectorError::from)?;
        Ok((points, next.map(|id| id.to_string())))
    }

    async fn delete_payload_index(
        &self,
        collection: &str,
        field: &str,
    ) -> VectorCoordinatorResult<()> {
        self.engine
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
            .engine
            .list_payload_indexes(collection)
            .map_err(VectorError::from)?)
    }

    async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        Ok(self
            .engine
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
            .engine
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
        use std::collections::VecDeque;
        let engine = Arc::clone(&self.engine);
        let collection = collection.to_string();
        let page = batch_size.max(1);
        let stream = futures::stream::unfold(
            (None::<String>, VecDeque::<VectorPoint>::new(), false),
            move |(mut offset, mut buffered, mut finished)| {
                let engine = Arc::clone(&engine);
                let collection = collection.clone();
                async move {
                    loop {
                        if let Some(point) = buffered.pop_front() {
                            return Some((Ok(point), (offset, buffered, finished)));
                        }
                        if finished {
                            return None;
                        }
                        let current = offset.clone().map(PointId::from);
                        match engine.scroll(
                            &collection,
                            page,
                            current.as_ref(),
                            with_payload,
                            with_vector,
                        ) {
                            Ok((points, next)) => {
                                finished = next.is_none();
                                offset = next.map(|id| id.to_string());
                                buffered = points.into();
                                if buffered.is_empty() && finished {
                                    return None;
                                }
                            }
                            Err(error) => {
                                let failure: VectorCoordinatorResult<VectorPoint> =
                                    Err(VectorCoordinatorError::Vector(VectorError::from(error)));
                                return Some((failure, (offset, buffered, true)));
                            }
                        }
                    }
                }
            },
        );
        Ok(Box::pin(stream))
    }
}

/// [`VectorEngineHandle`] implementation wrapping the remote Qdrant client.
#[cfg(feature = "vector-qdrant")]
#[derive(Debug)]
pub struct QdrantEngineHandle {
    manager: Arc<VectorManager>,
    /// `[vector.qdrant.collection]` creation defaults.
    defaults: CollectionDefaults,
    /// Protects remote write batches from cascading failures when the
    /// Qdrant service keeps rejecting requests.
    breaker: Arc<CircuitBreaker>,
}

#[cfg(feature = "vector-qdrant")]
impl QdrantEngineHandle {
    /// Wrap an already-allocated Qdrant manager.
    pub fn new(manager: Arc<VectorManager>) -> Self {
        Self::with_defaults(manager, CollectionDefaults::default())
    }

    /// Wrap an already-allocated Qdrant manager with explicit creation
    /// defaults taken from configuration.
    pub fn with_defaults(manager: Arc<VectorManager>, defaults: CollectionDefaults) -> Self {
        Self {
            manager,
            defaults,
            breaker: Arc::new(CircuitBreaker::new(
                "qdrant-write",
                CircuitBreakerConfig::default(),
            )),
        }
    }

    /// Circuit breaker protecting remote write batches. Exposed so callers
    /// can observe state without depending on the concrete manager.
    pub fn circuit_breaker(&self) -> &CircuitBreaker {
        &self.breaker
    }
}

#[cfg(feature = "vector-qdrant")]
#[async_trait]
impl VectorEngineHandle for QdrantEngineHandle {
    fn engine_name(&self) -> &'static str {
        "qdrant"
    }

    async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus> {
        Ok(self.manager.engine().health_check().await?)
    }

    fn effective_collection_config(&self, config: &CollectionConfig) -> CollectionConfig {
        // Remote backends store the request as given.
        config.clone()
    }

    fn qdrant_manager(&self) -> Option<Arc<VectorManager>> {
        Some(Arc::clone(&self.manager))
    }

    async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        self.manager.create_index(name, config.clone()).await?;
        Ok(())
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()> {
        self.manager
            .engine()
            .create_payload_index(collection, field, schema)
            .await?;
        Ok(())
    }

    fn index_exists(&self, name: &str) -> bool {
        self.manager.index_exists(name)
    }

    fn supports_atomic_promote(&self) -> bool {
        false
    }

    fn default_collection_config(
        &self,
        vector_size: usize,
        distance: simvec::DistanceMetric,
    ) -> CollectionConfig {
        let mut config = CollectionConfig::new(vector_size, distance);
        // Qdrant's own defaults are m=16, ef_construct=100 with a payload
        // graph of the same width; keep them when configuration stays silent.
        config.index = simvec::IndexConfig::Hnsw(
            self.defaults
                .hnsw
                .clone()
                .unwrap_or_else(|| simvec::HnswConfig::new(16, 100).with_payload_m(16)),
        );
        self.defaults.apply(&mut config);
        config
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

    async fn apply_grouped_batch(
        &self,
        upserts: HashMap<String, Vec<VectorPoint>>,
        deletes: HashMap<String, Vec<String>>,
    ) -> VectorCoordinatorResult<()> {
        use std::future::Future;
        let breaker = Arc::clone(&self.breaker);
        let result = with_circuit_breaker(&breaker, || async {
            let mut pending: Vec<
                Pin<Box<dyn Future<Output = VectorCoordinatorResult<()>> + Send + '_>>,
            > = Vec::new();
            for (collection, points) in upserts {
                pending.push(Box::pin(async move {
                    if points.len() == 1 {
                        self.upsert(&collection, points.into_iter().next().expect("len checked"))
                            .await?;
                    } else if !points.is_empty() {
                        self.upsert_batch(&collection, points).await?;
                    }
                    Ok(())
                }));
            }
            for (collection, point_ids) in deletes {
                pending.push(Box::pin(async move {
                    if point_ids.len() == 1 {
                        self.delete(&collection, &point_ids[0]).await?;
                    } else if !point_ids.is_empty() {
                        let refs: Vec<&str> = point_ids.iter().map(|id| id.as_str()).collect();
                        self.delete_batch(&collection, &refs).await?;
                    }
                    Ok(())
                }));
            }
            if !pending.is_empty() {
                futures::future::try_join_all(pending).await?;
            }
            Ok::<(), VectorCoordinatorError>(())
        })
        .await;
        match result {
            Ok(()) => Ok(()),
            Err(CircuitBreakerError::CircuitOpen) => Err(VectorCoordinatorError::Vector(
                VectorError::EngineUnavailable("qdrant write circuit breaker is open".to_string()),
            )),
            Err(CircuitBreakerError::OperationFailed(error)) => Err(error),
        }
    }

    async fn list_collections(&self) -> VectorCoordinatorResult<Vec<String>> {
        self.manager
            .engine()
            .list_collections()
            .await
            .map_err(VectorError::from)
            .map_err(VectorCoordinatorError::Vector)
    }

    fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        self.manager.get_index_metadata(name)
    }

    async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        self.manager.drop_index(name).await?;
        Ok(())
    }

    async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        Ok(self.manager.get(collection, point_id).await?)
    }

    async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        Ok(self.manager.count(collection).await?)
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> VectorCoordinatorResult<()> {
        self.manager.upsert(collection, point).await?;
        Ok(())
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        self.manager.upsert_batch(collection, points).await?;
        Ok(())
    }

    async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        self.manager.delete(collection, point_id).await?;
        Ok(())
    }

    async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        self.manager
            .delete_batch(collection, point_ids.to_vec())
            .await?;
        Ok(())
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        self.manager.delete_by_filter(collection, filter).await?;
        Ok(())
    }

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> VectorCoordinatorResult<()> {
        self.manager
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
        self.manager
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
        self.manager
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
            .manager
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
        self.manager
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
            .manager
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
        Ok(self.manager.search(collection, query).await?)
    }

    async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>>,
    > {
        // The remote client exposes no native streaming API yet, so buffer a
        // unary search into a stream. Large result sets are held in memory.
        let results = self.manager.search(collection, query).await?;
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
        use std::collections::VecDeque;
        let manager = Arc::clone(&self.manager);
        let collection = collection.to_string();
        let page = batch_size.max(1);
        let stream = futures::stream::unfold(
            (None::<String>, VecDeque::<VectorPoint>::new(), false),
            move |(mut offset, mut buffered, mut finished)| {
                let manager = Arc::clone(&manager);
                let collection = collection.clone();
                async move {
                    loop {
                        if let Some(point) = buffered.pop_front() {
                            return Some((Ok(point), (offset, buffered, finished)));
                        }
                        if finished {
                            return None;
                        }
                        let current = offset.clone();
                        match manager
                            .engine()
                            .scroll(
                                &collection,
                                page,
                                current.as_deref(),
                                with_payload,
                                with_vector,
                            )
                            .await
                        {
                            Ok((points, next)) => {
                                finished = next.is_none();
                                offset = next;
                                buffered = points.into();
                                if buffered.is_empty() && finished {
                                    return None;
                                }
                            }
                            Err(error) => {
                                let failure: VectorCoordinatorResult<VectorPoint> =
                                    Err(VectorCoordinatorError::from(error));
                                return Some((failure, (offset, buffered, true)));
                            }
                        }
                    }
                }
            },
        );
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

    /// Wrap an already-allocated `Arc<LocalVectorEngine>` and cap collections
    /// created without an explicit `max_points`.
    pub fn from_local_arc_with_limit(arc: Arc<LocalVectorEngine>, max_points: u64) -> Self {
        let metrics = Arc::new(crate::metrics_handle::MetricsEngineHandle::new(Arc::new(
            LocalEngineHandle::with_max_points(arc, max_points),
        )));
        Self {
            engine: metrics.clone(),
            metrics: Some(metrics),
        }
    }

    /// Wrap a [`VectorManager`] in the [`VectorBackend`] handle.
    ///
    /// Rejects a manager built from a disabled client configuration: an
    /// unavailable vector subsystem is represented by the absence of a
    /// backend, never by a handle whose every call fails.
    #[cfg(feature = "vector-qdrant")]
    pub fn qdrant(manager: Arc<VectorManager>) -> VectorCoordinatorResult<Self> {
        Self::qdrant_with_defaults(manager, CollectionDefaults::default())
    }

    /// Wrap a [`VectorManager`] with configuration-driven creation defaults.
    #[cfg(feature = "vector-qdrant")]
    pub fn qdrant_with_defaults(
        manager: Arc<VectorManager>,
        defaults: CollectionDefaults,
    ) -> VectorCoordinatorResult<Self> {
        if !manager.is_engine_available() {
            return Err(VectorCoordinatorError::Vector(VectorError::ConfigError(
                "Qdrant vector engine is disabled by configuration".to_string(),
            )));
        }
        let metrics = Arc::new(crate::metrics_handle::MetricsEngineHandle::new(Arc::new(
            QdrantEngineHandle::with_defaults(manager, defaults),
        )));
        Ok(Self {
            engine: metrics.clone(),
            metrics: Some(metrics),
        })
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
        self.engine.is_in_process()
    }

    /// Short engine identifier (`local` / `qdrant`), for reporting which
    /// backend serves an index.
    pub fn engine_name(&self) -> &'static str {
        self.engine.engine_name()
    }

    /// Assemble the vector backend described by `config`.
    ///
    /// This is the single place that turns a `[vector]` configuration into an
    /// engine: every entry point (server startup, the `GraphService` factory,
    /// the embedded database) calls it, so a new knob or engine cannot be
    /// wired into one path and missed in another.
    ///
    /// `Ok(None)` means the subsystem is deliberately off
    /// ([`linkrs_config::VectorSelection::Off`]); a contradictory selection —
    /// `engine = "qdrant"` with `[vector.qdrant].enabled = false` — is an
    /// error, because the operator asked for an engine and then switched off
    /// its only parameter block.
    ///
    /// The local engine needs a concrete directory: `[vector.local].data_dir`
    /// must already be resolved by the caller, since where the default lives
    /// is a deployment concern (`<storage_path>/vector` for the server, a
    /// sibling of the database file when embedded).
    pub async fn from_config(
        config: &linkrs_config::VectorConfig,
    ) -> VectorCoordinatorResult<Option<Self>> {
        use linkrs_config::VectorSelection;

        match config
            .selection()
            .map_err(|reason| VectorCoordinatorError::Vector(VectorError::ConfigError(reason)))?
        {
            VectorSelection::Off => Ok(None),
            VectorSelection::Local => {
                let data_dir = config.local.data_dir.clone().ok_or_else(|| {
                    VectorCoordinatorError::Vector(VectorError::ConfigError(
                        "the local vector engine needs a resolved [vector.local].data_dir"
                            .to_string(),
                    ))
                })?;
                let engine = crate::vector_defaults::assemble_local_engine(config, data_dir)
                    .map_err(|e| VectorCoordinatorError::Vector(VectorError::Internal(e)))?;
                Ok(Some(Self::from_local_arc_with_limit(
                    engine,
                    config.limits.default_max_points,
                )))
            }
            #[cfg(feature = "vector-qdrant")]
            VectorSelection::Qdrant => {
                let client_config = qdrant_client_config(&config.qdrant);
                let manager = Arc::new(
                    VectorManager::new(client_config)
                        .await
                        .map_err(|e| VectorCoordinatorError::Vector(VectorError::from(e)))?,
                );
                Ok(Some(Self::qdrant_with_defaults(
                    manager,
                    CollectionDefaults::from_qdrant(
                        &config.qdrant.collection,
                        config.limits.default_max_points,
                    ),
                )?))
            }
        }
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

    /// Whether this backend can atomically promote a temp collection.
    /// Callers choose physical promotion or logical remap through this
    /// instead of branching on the engine name.
    pub fn supports_atomic_promote(&self) -> bool {
        self.engine.supports_atomic_promote()
    }

    /// Default collection config for a new logical index. The index manager
    /// normalizes the result through `effective_collection_config` before
    /// registering it.
    pub fn default_collection_config(
        &self,
        vector_size: usize,
        distance: simvec::DistanceMetric,
    ) -> CollectionConfig {
        self.engine.default_collection_config(vector_size, distance)
    }

    /// Whether dropping a logical index reclaims its space-granularity slice
    /// from the shared collection. Capability query, not an engine-name
    /// branch at the call site.
    pub fn owns_space_slice_reclaim(&self) -> bool {
        self.engine.owns_space_slice_reclaim()
    }

    /// Atomically promote a temp collection over its live collection,
    /// retaining the previous live directory as `live + backup_suffix`
    /// (one backup generation; older ones are pruned).
    ///
    /// Only call when `supports_atomic_promote` holds. Backends without
    /// atomic promotion publish through a manager-level logical remap
    /// instead (see `VectorIndexManager`).
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

    /// Apply one already-grouped batch of writes and deletes through the
    /// engine-specific commit path.
    pub async fn apply_grouped_batch(
        &self,
        upserts: HashMap<String, Vec<VectorPoint>>,
        deletes: HashMap<String, Vec<String>>,
    ) -> VectorCoordinatorResult<()> {
        self.engine.apply_grouped_batch(upserts, deletes).await
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

}

#[cfg(all(test, feature = "vector-qdrant"))]
mod tests {
    use super::*;
    use crate::circuit_breaker::CircuitState;
    use crate::vector_error::VectorErrorKind;
    use vector_client::VectorClientConfig;

    async fn offline_handle() -> QdrantEngineHandle {
        let manager = Arc::new(
            VectorManager::new(VectorClientConfig::disabled())
                .await
                .expect("disabled manager builds without I/O"),
        );
        QdrantEngineHandle::new(manager)
    }

    #[tokio::test]
    async fn disabled_manager_is_rejected_by_backend_construction() {
        let manager = Arc::new(
            VectorManager::new(VectorClientConfig::disabled())
                .await
                .expect("disabled manager builds without I/O"),
        );
        let error = VectorBackend::qdrant(manager)
            .err()
            .expect("a disabled manager must not become a backend");
        assert!(matches!(
            error,
            VectorCoordinatorError::Vector(VectorError::ConfigError(_))
        ));
    }

    #[tokio::test]
    async fn tripped_breaker_rejects_write_batch_as_retryable() {
        let handle = offline_handle().await;
        handle.circuit_breaker().trip();
        assert_eq!(handle.circuit_breaker().state(), CircuitState::Open);

        let result = handle
            .apply_grouped_batch(HashMap::new(), HashMap::new())
            .await;
        let error = result.expect_err("open breaker must reject the batch");
        match &error {
            VectorCoordinatorError::Vector(VectorError::EngineUnavailable(message)) => {
                assert!(message.contains("circuit breaker is open"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(matches!(error.kind(), VectorErrorKind::Retryable));
    }
}

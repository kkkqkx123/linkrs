//! Engine-level metrics decorator for vector backends.
//!
//! Mirrors the fulltext `MetricsSearchEngine` pattern: a decorator implementing
//! [`VectorEngineHandle`] records latency and success/failure into the shared
//! [`StatsManager`] so every backend reports through one pipeline. The shared
//! handle is late-bound (`set_stats_manager`), matching how the coordinator
//! binds embedding/rerank stats after construction.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use parking_lot::RwLock;
use simvec::{
    CollectionConfig, HealthStatus, IndexMetadata, LocalVectorEngine, Payload, PayloadSchemaType,
    SearchQuery, SearchResult, VectorFilter, VectorPoint,
};

use crate::backend::VectorEngineHandle;
use crate::vector_error::VectorCoordinatorResult;

/// Decorator recording engine-level operations into [`StatsManager`].
///
/// Engine handles see collection names, not logical index locations, so
/// per-collection metrics land in the `unknown.{collection}` buckets and
/// never pollute real `space_{id}` keys.
pub struct MetricsEngineHandle {
    inner: Arc<dyn VectorEngineHandle>,
    stats_manager: RwLock<Option<Arc<linkrs_metrics::StatsManager>>>,
}

impl std::fmt::Debug for MetricsEngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsEngineHandle")
            .field("inner", &self.inner)
            .finish()
    }
}

impl MetricsEngineHandle {
    /// Wrap an engine handle with metrics recording.
    pub fn new(inner: Arc<dyn VectorEngineHandle>) -> Self {
        Self {
            inner,
            stats_manager: RwLock::new(None),
        }
    }

    /// Late-bind the shared observability handle.
    pub fn set_stats_manager(&self, stats_manager: Arc<linkrs_metrics::StatsManager>) {
        *self.stats_manager.write() = Some(stats_manager);
    }

    fn stats(&self) -> Option<Arc<linkrs_metrics::StatsManager>> {
        self.stats_manager.read().clone()
    }
}

#[async_trait]
impl VectorEngineHandle for MetricsEngineHandle {
    fn engine_name(&self) -> &'static str {
        self.inner.engine_name()
    }

    fn is_disabled(&self) -> bool {
        self.inner.is_disabled()
    }

    async fn health_check(&self) -> VectorCoordinatorResult<HealthStatus> {
        self.inner.health_check().await
    }

    fn effective_collection_config(&self, config: &CollectionConfig) -> CollectionConfig {
        self.inner.effective_collection_config(config)
    }

    fn local_engine(&self) -> Option<Arc<LocalVectorEngine>> {
        self.inner.local_engine()
    }

    #[cfg(feature = "vector-qdrant")]
    fn qdrant_manager(&self) -> Option<Arc<vector_client::VectorManager>> {
        self.inner.qdrant_manager()
    }

    async fn create_index(
        &self,
        name: &str,
        config: &CollectionConfig,
    ) -> VectorCoordinatorResult<()> {
        let start = Instant::now();
        let result = self.inner.create_index(name, config).await;
        if let Some(stats) = self.stats() {
            stats.record_index_operation_unknown_space(
                name,
                start.elapsed().as_millis() as u64,
                result.is_ok(),
            );
        }
        result
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> VectorCoordinatorResult<()> {
        self.inner
            .create_payload_index(collection, field, schema)
            .await
    }

    fn index_exists(&self, name: &str) -> bool {
        self.inner.index_exists(name)
    }

    async fn promote_temp_collection(
        &self,
        live: &str,
        temp: &str,
        backup_suffix: &str,
    ) -> VectorCoordinatorResult<()> {
        self.inner
            .promote_temp_collection(live, temp, backup_suffix)
            .await
    }

    async fn restore_promote_backup(&self, live: &str) -> VectorCoordinatorResult<String> {
        self.inner.restore_promote_backup(live).await
    }

    async fn list_collections(&self) -> VectorCoordinatorResult<Vec<String>> {
        self.inner.list_collections().await
    }

    fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        self.inner.get_index_metadata(name)
    }

    async fn delete_collection(&self, name: &str) -> VectorCoordinatorResult<()> {
        let start = Instant::now();
        let result = self.inner.delete_collection(name).await;
        if let Some(stats) = self.stats() {
            stats.record_index_operation_unknown_space(
                name,
                start.elapsed().as_millis() as u64,
                result.is_ok(),
            );
        }
        result
    }

    async fn get_vector(
        &self,
        collection: &str,
        point_id: &str,
    ) -> VectorCoordinatorResult<Option<VectorPoint>> {
        self.inner.get_vector(collection, point_id).await
    }

    async fn count(&self, collection: &str) -> VectorCoordinatorResult<u64> {
        self.inner.count(collection).await
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> VectorCoordinatorResult<()> {
        self.inner.upsert(collection, point).await
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> VectorCoordinatorResult<()> {
        let start = Instant::now();
        let result = self.inner.upsert_batch(collection, points).await;
        if let Some(stats) = self.stats() {
            stats.record_index_operation_unknown_space(
                collection,
                start.elapsed().as_millis() as u64,
                result.is_ok(),
            );
        }
        result
    }

    async fn delete(&self, collection: &str, point_id: &str) -> VectorCoordinatorResult<()> {
        self.inner.delete(collection, point_id).await
    }

    async fn delete_batch(
        &self,
        collection: &str,
        point_ids: &[&str],
    ) -> VectorCoordinatorResult<()> {
        let start = Instant::now();
        let result = self.inner.delete_batch(collection, point_ids).await;
        if let Some(stats) = self.stats() {
            stats.record_delete_operation_unknown_space(
                collection,
                start.elapsed().as_millis() as u64,
                result.is_ok(),
            );
        }
        result
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<()> {
        self.inner.delete_by_filter(collection, filter).await
    }

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> VectorCoordinatorResult<()> {
        self.inner.set_payload(collection, point_ids, payload).await
    }

    async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> VectorCoordinatorResult<()> {
        self.inner
            .set_payload_fields(collection, point_ids, fields)
            .await
    }

    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> VectorCoordinatorResult<()> {
        self.inner.delete_payload(collection, point_ids, keys).await
    }

    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<(Vec<VectorPoint>, Option<String>)> {
        self.inner
            .scroll(collection, limit, offset, with_payload, with_vector)
            .await
    }

    async fn delete_payload_index(
        &self,
        collection: &str,
        field: &str,
    ) -> VectorCoordinatorResult<()> {
        self.inner.delete_payload_index(collection, field).await
    }

    async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> VectorCoordinatorResult<Vec<(String, PayloadSchemaType)>> {
        self.inner.list_payload_indexes(collection).await
    }

    async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        let start = Instant::now();
        let result = self.inner.search(collection, query).await;
        if let Some(stats) = self.stats() {
            let latency_ms = start.elapsed().as_millis() as u64;
            stats.record_search_unknown_space(collection, latency_ms, result.is_ok());
            if let Ok(results) = &result {
                stats.record_search_result_count(0, results.len() as u64);
            }
        }
        result
    }

    async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn futures::Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>>,
    > {
        self.inner.search_stream(collection, query).await
    }

    async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        Pin<Box<dyn futures::Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    > {
        self.inner
            .scroll_stream(collection, batch_size, with_payload, with_vector)
            .await
    }
}

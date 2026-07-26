mod index;

pub use index::IndexMetadata;

use std::sync::Arc;

use dashmap::DashMap;
use tracing::{debug, info, warn};

use crate::config::{EngineType, VectorClientConfig};
#[cfg(all(feature = "qdrant-http", not(feature = "qdrant-grpc")))]
use crate::engine::QdrantEngine;
#[cfg(feature = "qdrant-grpc")]
use crate::engine::QdrantGrpcEngine;
use crate::engine::VectorEngine;
use crate::error::{Result, VectorClientError};
use crate::types::{CollectionConfig, SearchQuery, SearchResult, VectorFilter, VectorPoint};

pub struct VectorManager {
    engine: Arc<dyn VectorEngine>,
    indexes: DashMap<String, IndexMetadata>,
}

impl std::fmt::Debug for VectorManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorManager")
            .field("engine", &self.engine.name())
            .field("index_count", &self.indexes.len())
            .finish()
    }
}

impl VectorManager {
    pub async fn new(config: VectorClientConfig) -> Result<Self> {
        let enabled = config.enabled;

        let engine: Arc<dyn VectorEngine> = if enabled {
            let engine = create_engine(config).await?;
            engine
        } else {
            info!("Vector search is disabled, using no-op engine");
            Arc::new(DisabledEngine) as Arc<dyn VectorEngine>
        };

        if enabled {
            match engine.health_check().await {
                Ok(health) => {
                    if health.is_healthy {
                        info!(
                            "Vector engine health check passed: {} {}",
                            health.engine_name, health.engine_version
                        );
                    } else {
                        warn!("Vector engine health check failed: {:?}", health.message);
                    }
                }
                Err(e) => {
                    warn!("Vector engine health check failed: {}", e);
                }
            }
        }

        Ok(Self {
            engine,
            indexes: DashMap::new(),
        })
    }

    pub fn engine(&self) -> &Arc<dyn VectorEngine> {
        &self.engine
    }

    pub async fn create_index(&self, name: &str, config: CollectionConfig) -> Result<()> {
        if self.indexes.contains_key(name) {
            return Err(VectorClientError::IndexAlreadyExists(name.to_string()));
        }

        debug!("Creating vector collection: {}", name);
        self.engine.create_collection(name, config.clone()).await?;

        let metadata = IndexMetadata::new(name.to_string(), config);
        self.indexes.insert(name.to_string(), metadata);

        info!("Vector index created: {}", name);
        Ok(())
    }

    pub async fn drop_index(&self, name: &str) -> Result<()> {
        if let Some((_, metadata)) = self.indexes.remove(name) {
            debug!("Dropping vector collection: {}", metadata.name);
            self.engine.delete_collection(name).await?;
            info!("Vector index dropped: {}", name);
        }
        Ok(())
    }

    pub fn unregister_index(&self, name: &str) {
        if self.indexes.remove(name).is_some() {
            debug!("Unregistered logical index: {}", name);
        }
    }

    pub fn register_index(&self, name: &str, metadata: IndexMetadata) {
        self.indexes.insert(name.to_string(), metadata);
    }

    pub fn index_exists(&self, name: &str) -> bool {
        self.indexes.contains_key(name)
    }

    pub fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        self.indexes.get(name).map(|m| m.clone())
    }

    pub fn list_indexes(&self) -> Vec<IndexMetadata> {
        self.indexes.iter().map(|m| m.value().clone()).collect()
    }

    pub async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<()> {
        self.engine.upsert(collection, point).await?;
        Ok(())
    }

    pub async fn upsert_batch(&self, collection: &str, points: Vec<VectorPoint>) -> Result<()> {
        self.engine.upsert_batch(collection, points).await?;
        Ok(())
    }

    pub async fn delete(&self, collection: &str, point_id: &str) -> Result<()> {
        self.engine.delete(collection, point_id).await?;
        Ok(())
    }

    pub async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<()> {
        self.engine.delete_batch(collection, point_ids).await?;
        Ok(())
    }

    pub async fn delete_by_filter(&self, collection: &str, filter: VectorFilter) -> Result<()> {
        self.engine.delete_by_filter(collection, filter).await?;
        Ok(())
    }

    pub async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
        self.engine.search(collection, query).await
    }

    pub async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>> {
        self.engine.get(collection, point_id).await
    }

    pub async fn count(&self, collection: &str) -> Result<u64> {
        self.engine.count(collection).await
    }
}

async fn create_engine(config: VectorClientConfig) -> Result<Arc<dyn VectorEngine>> {
    match config.engine {
        EngineType::Qdrant => {
            #[cfg(feature = "qdrant-grpc")]
            {
                info!("Initializing Qdrant gRPC engine");
                let engine = QdrantGrpcEngine::new(config)
                    .await
                    .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))?;
                Ok(Arc::new(engine) as Arc<dyn VectorEngine>)
            }

            #[cfg(all(feature = "qdrant-http", not(feature = "qdrant-grpc")))]
            {
                info!("Initializing Qdrant HTTP engine");
                let engine = QdrantEngine::new(config)
                    .await
                    .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))?;
                Ok(Arc::new(engine) as Arc<dyn VectorEngine>)
            }

            #[cfg(not(any(feature = "qdrant-http", feature = "qdrant-grpc")))]
            {
                let _ = config;
                Err(VectorClientError::EngineNotAvailable(
                    "Qdrant engine feature not enabled".to_string(),
                ))
            }
        }
    }
}

mod disabled {
    use async_trait::async_trait;

    use crate::engine::VectorEngine;
    use crate::error::{Result, VectorClientError};
    use crate::types::*;

    pub struct DisabledEngine;

    impl std::fmt::Debug for DisabledEngine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("DisabledEngine").finish()
        }
    }

    #[async_trait]
    impl VectorEngine for DisabledEngine {
        fn name(&self) -> &str {
            "disabled"
        }
        fn version(&self) -> &str {
            "0.0"
        }

        async fn health_check(&self) -> Result<HealthStatus> {
            Ok(HealthStatus::unhealthy(
                "disabled",
                "0.0",
                "Engine disabled",
            ))
        }

        async fn create_collection(&self, _name: &str, _config: CollectionConfig) -> Result<()> {
            self.err().await
        }
        async fn delete_collection(&self, _name: &str) -> Result<()> {
            self.err().await
        }
        async fn collection_exists(&self, _name: &str) -> Result<bool> {
            self.err().await
        }
        async fn collection_info(&self, _name: &str) -> Result<CollectionInfo> {
            self.err().await
        }
        async fn upsert(&self, _collection: &str, _point: VectorPoint) -> Result<UpsertResult> {
            self.err().await
        }
        async fn upsert_batch(
            &self,
            _collection: &str,
            _points: Vec<VectorPoint>,
        ) -> Result<UpsertResult> {
            self.err().await
        }
        async fn delete(&self, _collection: &str, _point_id: &str) -> Result<DeleteResult> {
            self.err().await
        }
        async fn delete_batch(
            &self,
            _collection: &str,
            _point_ids: Vec<&str>,
        ) -> Result<DeleteResult> {
            self.err().await
        }
        async fn delete_by_filter(
            &self,
            _collection: &str,
            _filter: VectorFilter,
        ) -> Result<DeleteResult> {
            self.err().await
        }
        async fn search(
            &self,
            _collection: &str,
            _query: SearchQuery,
        ) -> Result<Vec<SearchResult>> {
            self.err().await
        }
        async fn search_batch(
            &self,
            _collection: &str,
            _queries: Vec<SearchQuery>,
        ) -> Result<Vec<Vec<SearchResult>>> {
            self.err().await
        }
        async fn get(&self, _collection: &str, _point_id: &str) -> Result<Option<VectorPoint>> {
            self.err().await
        }
        async fn get_batch(
            &self,
            _collection: &str,
            _point_ids: Vec<&str>,
        ) -> Result<Vec<Option<VectorPoint>>> {
            self.err().await
        }
        async fn count(&self, _collection: &str) -> Result<u64> {
            self.err().await
        }
        async fn set_payload(
            &self,
            _collection: &str,
            _point_ids: Vec<&str>,
            _payload: Payload,
        ) -> Result<()> {
            self.err().await
        }
        async fn delete_payload(
            &self,
            _collection: &str,
            _point_ids: Vec<&str>,
            _keys: Vec<&str>,
        ) -> Result<()> {
            self.err().await
        }
        async fn scroll(
            &self,
            _collection: &str,
            _limit: usize,
            _offset: Option<&str>,
            _with_payload: Option<bool>,
            _with_vector: Option<bool>,
        ) -> Result<(Vec<VectorPoint>, Option<String>)> {
            self.err().await
        }
        async fn create_payload_index(
            &self,
            _collection: &str,
            _field: &str,
            _schema: PayloadSchemaType,
        ) -> Result<()> {
            self.err().await
        }
        async fn delete_payload_index(&self, _collection: &str, _field: &str) -> Result<()> {
            self.err().await
        }
        async fn list_payload_indexes(
            &self,
            _collection: &str,
        ) -> Result<Vec<(String, PayloadSchemaType)>> {
            self.err().await
        }
    }

    impl DisabledEngine {
        async fn err<T>(&self) -> Result<T> {
            Err(VectorClientError::EngineNotAvailable(
                "vector engine disabled".to_string(),
            ))
        }
    }
}

use disabled::DisabledEngine;

#[cfg(test)]
mod tests {
    use super::disabled::DisabledEngine;
    use crate::engine::VectorEngine;
    use crate::types::*;

    #[tokio::test]
    async fn test_disabled_engine_health_check() {
        let engine = DisabledEngine;
        let h = engine.health_check().await.unwrap();
        assert!(!h.is_healthy);
        assert_eq!(h.engine_name, "disabled");
    }

    #[tokio::test]
    async fn test_disabled_engine_create_collection() {
        let engine = DisabledEngine;
        let result = engine
            .create_collection("test", CollectionConfig::default())
            .await;
        assert!(matches!(
            result,
            Err(crate::error::VectorClientError::EngineNotAvailable(_))
        ));
    }

    #[tokio::test]
    async fn test_disabled_engine_delete_collection() {
        let engine = DisabledEngine;
        assert!(engine.delete_collection("test").await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_upsert() {
        let engine = DisabledEngine;
        assert!(engine
            .upsert("c", VectorPoint::new(1u64, vec![1.0]))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_upsert_batch() {
        let engine = DisabledEngine;
        assert!(engine.upsert_batch("c", vec![]).await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_delete() {
        let engine = DisabledEngine;
        assert!(engine.delete("c", "1").await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_delete_batch() {
        let engine = DisabledEngine;
        assert!(engine.delete_batch("c", vec!["1"]).await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_delete_by_filter() {
        let engine = DisabledEngine;
        assert!(engine
            .delete_by_filter("c", VectorFilter::new())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_search() {
        let engine = DisabledEngine;
        assert!(engine
            .search("c", SearchQuery::new(vec![1.0], 10))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_get() {
        let engine = DisabledEngine;
        assert!(engine.get("c", "1").await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_count() {
        let engine = DisabledEngine;
        assert!(engine.count("c").await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_collection_exists() {
        let engine = DisabledEngine;
        assert!(engine.collection_exists("c").await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_collection_info() {
        let engine = DisabledEngine;
        assert!(engine.collection_info("c").await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_scroll() {
        let engine = DisabledEngine;
        assert!(engine.scroll("c", 10, None, None, None).await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_set_payload() {
        let engine = DisabledEngine;
        let mut payload = std::collections::HashMap::new();
        payload.insert("k".into(), serde_json::json!("v"));
        assert!(engine.set_payload("c", vec!["1"], payload).await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_delete_payload() {
        let engine = DisabledEngine;
        assert!(engine
            .delete_payload("c", vec!["1"], vec!["k"])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_create_payload_index() {
        let engine = DisabledEngine;
        assert!(engine
            .create_payload_index("c", "f", PayloadSchemaType::Keyword)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_delete_payload_index() {
        let engine = DisabledEngine;
        assert!(engine.delete_payload_index("c", "f").await.is_err());
    }

    #[tokio::test]
    async fn test_disabled_engine_list_payload_indexes() {
        let engine = DisabledEngine;
        assert!(engine.list_payload_indexes("c").await.is_err());
    }
}

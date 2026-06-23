use std::sync::Arc;

#[cfg(any(feature = "qdrant-http", feature = "qdrant-grpc"))]
use crate::config::EngineType;
use crate::config::VectorClientConfig;
use crate::embedding::{EmbeddingConfig, EmbeddingService};
use crate::engine::{DisabledEngine, VectorEngine};
use crate::error::{Result, VectorClientError};
use crate::types::*;

use super::core::{CollectionApi, CollectionApiDyn, PointApi, PointApiDyn, SearchApi, SearchApiDyn};

#[derive(Debug)]
pub struct VectorClient {
    engine: Arc<dyn VectorEngine>,
}

impl VectorClient {
    pub async fn new(config: VectorClientConfig) -> Result<Self> {
        if !config.enabled {
            return Ok(Self {
                engine: Arc::new(DisabledEngine),
            });
        }

        #[cfg(any(feature = "qdrant-http", feature = "qdrant-grpc"))]
        {
            let engine: Arc<dyn VectorEngine> = match config.engine {
                EngineType::Qdrant => {
                    #[cfg(feature = "qdrant-grpc")]
                    {
                        let e = crate::engine::QdrantGrpcEngine::new(config.clone()).await?;
                        Arc::new(e)
                    }
                    #[cfg(all(not(feature = "qdrant-grpc"), feature = "qdrant-http"))]
                    {
                        let e = crate::engine::QdrantEngine::new(config.clone()).await?;
                        Arc::new(e)
                    }
                }
            };

            Ok(Self { engine })
        }

        #[cfg(not(any(feature = "qdrant-http", feature = "qdrant-grpc")))]
        {
            let _ = config;
            Err(crate::error::VectorClientError::EngineNotAvailable(
                "no qdrant engine feature enabled".to_string(),
            ))
        }
    }

    pub fn engine(&self) -> &dyn VectorEngine {
        self.engine.as_ref()
    }

    pub async fn health_check(&self) -> Result<HealthStatus> {
        self.engine.health_check().await
    }

    pub fn collection(&self) -> CollectionApiDyn<'_> {
        CollectionApi::new(self.engine.as_ref())
    }

    pub fn points(&self, collection: impl Into<String>) -> PointApiDyn<'_> {
        PointApi::new(self.engine.as_ref(), collection)
    }

    pub fn search(&self, collection: impl Into<String>) -> SearchApiDyn<'_> {
        SearchApi::new(self.engine.as_ref(), collection)
    }

    pub async fn search_with_text(
        &self,
        collection: impl Into<String>,
        text: &str,
        embedding_config: EmbeddingConfig,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let embedding_service = EmbeddingService::from_config(embedding_config)
            .map_err(|e| VectorClientError::InternalError(e.to_string()))?;
        let vector = embedding_service
            .embed(text)
            .await
            .map_err(|e| VectorClientError::InternalError(e.to_string()))?;

        let query = SearchQuery::new(vector, limit);
        self.search(collection).search(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_disabled_engine_returns_error() {
        let engine = DisabledEngine;
        let result = engine
            .create_collection("test", CollectionConfig::default())
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            VectorClientError::EngineNotAvailable(_) => {}
            _ => panic!("expected EngineNotAvailable"),
        }
    }

    #[tokio::test]
    async fn test_disabled_engine_health_check() {
        let engine = DisabledEngine;
        let health = engine.health_check().await.unwrap();
        assert!(!health.is_healthy);
        assert_eq!(health.engine_name, "disabled");
    }

    #[tokio::test]
    async fn test_disabled_engine_all_ops_error() {
        let engine = DisabledEngine;
        assert!(engine
            .upsert("c", VectorPoint::new(1u64, vec![1.0]))
            .await
            .is_err());
        assert!(engine.delete("c", "1").await.is_err());
        assert!(engine
            .search("c", SearchQuery::new(vec![1.0], 10))
            .await
            .is_err());
        assert!(engine.get("c", "1").await.is_err());
        assert!(engine.count("c").await.is_err());
        assert!(engine.collection_exists("c").await.is_err());
        assert!(engine.collection_info("c").await.is_err());
    }

    #[tokio::test]
    async fn test_vector_client_with_disabled_engine() {
        let config = VectorClientConfig::disabled();
        let client = VectorClient::new(config).await.unwrap();
        let debug_str = format!("{:?}", client);
        assert!(debug_str.contains("VectorClient"));
        assert!(client.health_check().await.unwrap().is_healthy == false);
    }
}

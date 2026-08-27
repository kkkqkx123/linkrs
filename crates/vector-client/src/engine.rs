use async_trait::async_trait;

use crate::error::{Result, VectorClientError};
use crate::types::*;

pub mod common;

#[cfg(feature = "qdrant-http")]
pub mod http;

#[cfg(feature = "qdrant-http")]
pub use http::QdrantEngine;

#[cfg(feature = "qdrant-grpc")]
pub mod grpc;

#[cfg(feature = "qdrant-grpc")]
pub use grpc::QdrantGrpcEngine;

#[derive(Debug)]
pub struct DisabledEngine;

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
    async fn list_collections(&self) -> Result<Vec<String>> {
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
    async fn delete_batch(&self, _collection: &str, _point_ids: Vec<&str>) -> Result<DeleteResult> {
        self.err().await
    }
    async fn delete_by_filter(
        &self,
        _collection: &str,
        _filter: VectorFilter,
    ) -> Result<DeleteResult> {
        self.err().await
    }
    async fn search(&self, _collection: &str, _query: SearchQuery) -> Result<Vec<SearchResult>> {
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

#[async_trait]
pub trait VectorEngine: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    fn version(&self) -> &str;

    async fn health_check(&self) -> Result<HealthStatus>;

    async fn create_collection(&self, name: &str, config: CollectionConfig) -> Result<()>;
    async fn delete_collection(&self, name: &str) -> Result<()>;
    async fn collection_exists(&self, name: &str) -> Result<bool>;
    /// Names of all collections known to the server.
    async fn list_collections(&self) -> Result<Vec<String>> {
        Err(VectorClientError::NotSupported(
            "list_collections".to_string(),
        ))
    }
    async fn collection_info(&self, name: &str) -> Result<CollectionInfo>;

    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult>;
    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<UpsertResult>;

    async fn delete(&self, collection: &str, point_id: &str) -> Result<DeleteResult>;
    async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<DeleteResult>;

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> Result<DeleteResult> {
        let _ = (collection, filter);
        Err(VectorClientError::NotSupported(
            "delete_by_filter".to_string(),
        ))
    }

    async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>>;

    async fn search_batch(
        &self,
        collection: &str,
        queries: Vec<SearchQuery>,
    ) -> Result<Vec<Vec<SearchResult>>> {
        let mut results = Vec::with_capacity(queries.len());
        for query in queries {
            results.push(self.search(collection, query).await?);
        }
        Ok(results)
    }

    async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>>;
    async fn get_batch(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
    ) -> Result<Vec<Option<VectorPoint>>>;
    async fn count(&self, collection: &str) -> Result<u64>;

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> Result<()> {
        let _ = (collection, point_ids, payload);
        Err(VectorClientError::NotSupported("set_payload".to_string()))
    }

    /// Merge the given fields into the payload of the given points. Only the
    /// supplied keys are updated; other existing keys are preserved. Engines
    /// that do not support per-key merge fall back to `set_payload`.
    async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> Result<()> {
        let _ = (collection, point_ids, fields);
        Err(VectorClientError::NotSupported(
            "set_payload_fields".to_string(),
        ))
    }

    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> Result<()> {
        let _ = (collection, point_ids, keys);
        Err(VectorClientError::NotSupported(
            "delete_payload".to_string(),
        ))
    }

    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> Result<(Vec<VectorPoint>, Option<String>)> {
        let _ = (collection, limit, offset, with_payload, with_vector);
        Err(VectorClientError::NotSupported("scroll".to_string()))
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> Result<()> {
        let _ = (collection, field, schema);
        Err(VectorClientError::NotSupported(
            "create_payload_index".to_string(),
        ))
    }

    async fn delete_payload_index(&self, collection: &str, field: &str) -> Result<()> {
        let _ = (collection, field);
        Err(VectorClientError::NotSupported(
            "delete_payload_index".to_string(),
        ))
    }

    async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, PayloadSchemaType)>> {
        let _ = collection;
        Err(VectorClientError::NotSupported(
            "list_payload_indexes".to_string(),
        ))
    }
}

/// Build the engine selected by `config`.
///
/// The wire transport is taken from [`crate::config::ConnectionConfig::transport`];
/// requesting a transport whose feature is not compiled in is a loud error
/// rather than a silent fallback.
pub async fn create_engine(
    config: crate::config::VectorClientConfig,
) -> Result<std::sync::Arc<dyn VectorEngine>> {
    use crate::config::{EngineType, QdrantTransport};

    match config.engine {
        EngineType::Qdrant => {
            #[cfg(feature = "qdrant-grpc")]
            if config.connection.transport == QdrantTransport::Grpc {
                let engine = QdrantGrpcEngine::new(config.clone()).await?;
                return Ok(std::sync::Arc::new(engine));
            }

            #[cfg(feature = "qdrant-http")]
            if config.connection.transport == QdrantTransport::Http {
                let engine = QdrantEngine::new(config.clone()).await?;
                return Ok(std::sync::Arc::new(engine));
            }

            Err(VectorClientError::EngineNotAvailable(format!(
                "{:?} transport is not available in this build",
                config.connection.transport
            )))
        }
    }
}

#[cfg(test)]
mod disabled_tests {
    use super::DisabledEngine;
    use crate::engine::VectorEngine;
    use crate::types::*;

    #[tokio::test]
    async fn disabled_engine_health_check_is_unhealthy() {
        let engine = DisabledEngine;
        let health = engine.health_check().await.unwrap();
        assert!(!health.is_healthy);
        assert_eq!(health.engine_name, "disabled");
    }

    #[tokio::test]
    async fn disabled_engine_rejects_collections_and_points() {
        let engine = DisabledEngine;
        assert!(engine
            .create_collection("c", CollectionConfig::default())
            .await
            .is_err());
        assert!(engine.delete_collection("c").await.is_err());
        assert!(engine
            .upsert("c", VectorPoint::new(1u64, vec![1.0]))
            .await
            .is_err());
        assert!(engine.upsert_batch("c", vec![]).await.is_err());
        assert!(engine.delete("c", "1").await.is_err());
        assert!(engine.delete_batch("c", vec!["1"]).await.is_err());
        assert!(engine
            .delete_by_filter("c", VectorFilter::new())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn disabled_engine_rejects_reads_and_admin_ops() {
        let engine = DisabledEngine;
        assert!(engine
            .search("c", SearchQuery::new(vec![1.0], 10))
            .await
            .is_err());
        assert!(engine.get("c", "1").await.is_err());
        assert!(engine.count("c").await.is_err());
        assert!(engine.collection_exists("c").await.is_err());
        assert!(engine.collection_info("c").await.is_err());
        assert!(engine.scroll("c", 10, None, None, None).await.is_err());

        let payload = std::collections::HashMap::from([("k".to_string(), serde_json::json!("v"))]);
        assert!(engine.set_payload("c", vec!["1"], payload).await.is_err());
        assert!(engine
            .delete_payload("c", vec!["1"], vec!["k"])
            .await
            .is_err());
        assert!(engine
            .create_payload_index("c", "f", PayloadSchemaType::Keyword)
            .await
            .is_err());
        assert!(engine.delete_payload_index("c", "f").await.is_err());
        assert!(engine.list_payload_indexes("c").await.is_err());
    }
}

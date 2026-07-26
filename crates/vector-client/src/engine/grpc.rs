#[cfg(feature = "qdrant-grpc")]
#[allow(clippy::large_enum_variant)]
pub mod proto {
    tonic::include_proto!("qdrant");
}

pub mod convert;
pub mod filter;

#[cfg(feature = "qdrant-grpc")]
pub mod interceptor;

#[cfg(feature = "qdrant-grpc")]
pub mod streaming;

use async_trait::async_trait;
use std::time::Duration;
use tonic::transport::Channel;
use tracing::{debug, info, warn};

use super::VectorEngine;
use crate::config::VectorClientConfig;
use crate::error::{Result, VectorClientError};
use crate::types::*;

use convert::{
    collection_config_to_create, collection_info_from_proto, point_id_to_proto,
    point_struct_to_proto, retrieved_point_from_proto, scored_point_from_proto,
    search_query_to_proto, upsert_result_from_proto,
};
use filter::filter_to_proto;
use interceptor::GrpcInterceptor;

const QDRANT_GRPC_VERSION: &str = "1.12.x (gRPC)";

pub struct QdrantGrpcEngine {
    channel: Channel,
    config: VectorClientConfig,
}

impl std::fmt::Debug for QdrantGrpcEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QdrantGrpcEngine")
            .field("config", &self.config)
            .finish()
    }
}

impl QdrantGrpcEngine {
    pub async fn new(config: VectorClientConfig) -> Result<Self> {
        let grpc_port = config.connection.port;
        let host = &config.connection.host;
        let scheme = if config.connection.use_tls {
            "https"
        } else {
            "http"
        };
        let addr = format!("{}://{}:{}", scheme, host, grpc_port);

        info!("Connecting to Qdrant gRPC API at {}", addr);

        let timeout = config.timeout.request_duration();

        let endpoint = Channel::from_shared(addr)
            .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))?
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(config.connection.connect_timeout_secs));

        let channel = endpoint
            .connect()
            .await
            .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))?;

        let interceptor = GrpcInterceptor::new(config.connection.api_key.clone(), true, true);

        let channel = interceptor.apply_to_channel(channel);

        let engine = Self { channel, config };

        match engine.health_check().await {
            Ok(health) => {
                if health.is_healthy {
                    info!("Successfully connected to Qdrant gRPC API");
                } else {
                    warn!("Qdrant health check warning: {:?}", health.message);
                }
            }
            Err(e) => {
                warn!("Initial health check failed, continuing: {}", e);
            }
        }

        Ok(engine)
    }

    pub fn streaming(&self) -> streaming::StreamingEngine {
        streaming::StreamingEngine::new(self.channel.clone())
    }

    fn points(&self) -> proto::points_client::PointsClient<Channel> {
        proto::points_client::PointsClient::new(self.channel.clone())
    }

    fn collections(&self) -> proto::collections_client::CollectionsClient<Channel> {
        proto::collections_client::CollectionsClient::new(self.channel.clone())
    }
}

#[async_trait]
impl VectorEngine for QdrantGrpcEngine {
    fn name(&self) -> &str {
        "qdrant-grpc"
    }

    fn version(&self) -> &str {
        QDRANT_GRPC_VERSION
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        match self
            .collections()
            .list(proto::ListCollectionsRequest {})
            .await
        {
            Ok(_) => Ok(HealthStatus::healthy(self.name(), self.version())),
            Err(e) => Ok(HealthStatus::unhealthy(
                self.name(),
                self.version(),
                e.to_string(),
            )),
        }
    }

    async fn create_collection(&self, name: &str, cfg: CollectionConfig) -> Result<()> {
        debug!("Creating collection '{}' via gRPC", name);

        let request = collection_config_to_create(name, &cfg);
        self.collections().create(request).await?;

        info!("Collection '{}' created successfully via gRPC", name);
        Ok(())
    }

    async fn delete_collection(&self, name: &str) -> Result<()> {
        debug!("Deleting collection '{}' via gRPC", name);

        let request = proto::DeleteCollection {
            collection_name: name.to_string(),
            timeout: None,
        };

        self.collections().delete(request).await?;

        info!("Collection '{}' deleted successfully via gRPC", name);
        Ok(())
    }

    async fn collection_exists(&self, name: &str) -> Result<bool> {
        let request = proto::CollectionExistsRequest {
            collection_name: name.to_string(),
        };

        let response = self.collections().collection_exists(request).await?;

        Ok(response
            .into_inner()
            .result
            .map(|r| r.exists)
            .unwrap_or(false))
    }

    async fn collection_info(&self, name: &str) -> Result<CollectionInfo> {
        let request = proto::GetCollectionInfoRequest {
            collection_name: name.to_string(),
        };

        let response = self.collections().get(request).await?;

        let info = response
            .into_inner()
            .result
            .ok_or_else(|| VectorClientError::CollectionNotFound(name.to_string()))?;

        collection_info_from_proto(info, name)
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult> {
        debug!("Upserting point '{}' via gRPC", point.id);

        let request = proto::UpsertPoints {
            collection_name: collection.to_string(),
            wait: Some(true),
            ordering: None,
            shard_key_selector: None,
            points: vec![point_struct_to_proto(&point)],
        };

        let response = self.points().upsert(request).await?;
        let result = response
            .into_inner()
            .result
            .ok_or_else(|| VectorClientError::InternalError("missing update result".into()))?;

        Ok(upsert_result_from_proto(result))
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<UpsertResult> {
        debug!("Upserting {} points via gRPC", points.len());

        let proto_points: Vec<proto::PointStruct> =
            points.iter().map(point_struct_to_proto).collect();

        let request = proto::UpsertPoints {
            collection_name: collection.to_string(),
            wait: Some(true),
            ordering: None,
            shard_key_selector: None,
            points: proto_points,
        };

        let response = self.points().upsert(request).await?;
        let result = response
            .into_inner()
            .result
            .ok_or_else(|| VectorClientError::InternalError("missing update result".into()))?;

        Ok(upsert_result_from_proto(result))
    }

    async fn delete(&self, collection: &str, point_id: &str) -> Result<DeleteResult> {
        debug!("Deleting point '{}' via gRPC", point_id);

        let selector = proto::PointsSelector {
            points_selector_one_of: Some(proto::points_selector::PointsSelectorOneOf::Points(
                proto::PointsIdsList {
                    ids: vec![point_id_to_proto(point_id)],
                },
            )),
        };

        let request = proto::DeletePoints {
            collection_name: collection.to_string(),
            wait: Some(true),
            points: Some(selector),
            ordering: None,
            shard_key_selector: None,
        };

        let response = self.points().delete(request).await?;
        let result = response
            .into_inner()
            .result
            .ok_or_else(|| VectorClientError::InternalError("missing delete result".into()))?;

        Ok(DeleteResult {
            operation_id: result.operation_id,
            deleted_count: if result.status() == proto::UpdateStatus::Completed {
                1
            } else {
                0
            },
        })
    }

    async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<DeleteResult> {
        debug!("Deleting {} points via gRPC", point_ids.len());

        let ids: Vec<proto::PointId> = point_ids.iter().map(|id| point_id_to_proto(id)).collect();

        let selector = proto::PointsSelector {
            points_selector_one_of: Some(proto::points_selector::PointsSelectorOneOf::Points(
                proto::PointsIdsList { ids },
            )),
        };

        let request = proto::DeletePoints {
            collection_name: collection.to_string(),
            wait: Some(true),
            points: Some(selector),
            ordering: None,
            shard_key_selector: None,
        };

        let response = self.points().delete(request).await?;
        let result = response
            .into_inner()
            .result
            .ok_or_else(|| VectorClientError::InternalError("missing delete result".into()))?;

        Ok(DeleteResult {
            operation_id: result.operation_id,
            deleted_count: point_ids.len() as u64,
        })
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> Result<DeleteResult> {
        debug!("Deleting points by filter via gRPC");

        let proto_filter = filter_to_proto(&filter)?
            .ok_or_else(|| VectorClientError::FilterError("Empty filter".to_string()))?;

        let selector = proto::PointsSelector {
            points_selector_one_of: Some(proto::points_selector::PointsSelectorOneOf::Filter(
                proto_filter,
            )),
        };

        let request = proto::DeletePoints {
            collection_name: collection.to_string(),
            wait: Some(true),
            points: Some(selector),
            ordering: None,
            shard_key_selector: None,
        };

        let response = self.points().delete(request).await?;
        let result = response
            .into_inner()
            .result
            .ok_or_else(|| VectorClientError::InternalError("missing delete result".into()))?;

        Ok(DeleteResult {
            operation_id: result.operation_id,
            deleted_count: if result.status() == proto::UpdateStatus::Completed {
                1
            } else {
                0
            },
        })
    }

    async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
        debug!("Searching in collection '{}' via gRPC", collection);

        let request = search_query_to_proto(collection, &query);

        let response = self.points().search(request).await?;
        let scored_points = response.into_inner().result;

        let results: Vec<SearchResult> = scored_points
            .into_iter()
            .map(scored_point_from_proto)
            .collect();

        Ok(results)
    }

    async fn search_batch(
        &self,
        collection: &str,
        queries: Vec<SearchQuery>,
    ) -> Result<Vec<Vec<SearchResult>>> {
        debug!("Batch searching {} queries via gRPC", queries.len());

        let search_points: Vec<proto::SearchPoints> = queries
            .iter()
            .map(|q| search_query_to_proto(collection, q))
            .collect();

        let request = proto::SearchBatchPoints {
            collection_name: collection.to_string(),
            search_points,
            read_consistency: None,
            timeout: None,
        };

        let response = self.points().search_batch(request).await?;
        let batch_results = response.into_inner().result;

        let results: Vec<Vec<SearchResult>> = batch_results
            .into_iter()
            .map(|batch| {
                batch
                    .result
                    .into_iter()
                    .map(scored_point_from_proto)
                    .collect()
            })
            .collect();

        Ok(results)
    }

    async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>> {
        debug!("Getting point '{}' via gRPC", point_id);

        let request = proto::GetPoints {
            collection_name: collection.to_string(),
            ids: vec![point_id_to_proto(point_id)],
            with_payload: Some(proto::WithPayloadSelector {
                selector_options: Some(proto::with_payload_selector::SelectorOptions::Enable(true)),
            }),
            with_vectors: Some(proto::WithVectorsSelector {
                selector_options: Some(proto::with_vectors_selector::SelectorOptions::Enable(true)),
            }),
            read_consistency: None,
            shard_key_selector: None,
            timeout: None,
        };

        let response = self.points().get(request).await?;
        let retrieved = response.into_inner().result;

        match retrieved.into_iter().next() {
            Some(point) => Ok(Some(retrieved_point_from_proto(point))),
            None => Ok(None),
        }
    }

    async fn get_batch(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
    ) -> Result<Vec<Option<VectorPoint>>> {
        debug!("Getting {} points via gRPC", point_ids.len());

        let ids: Vec<proto::PointId> = point_ids.iter().map(|id| point_id_to_proto(id)).collect();

        let request = proto::GetPoints {
            collection_name: collection.to_string(),
            ids,
            with_payload: Some(proto::WithPayloadSelector {
                selector_options: Some(proto::with_payload_selector::SelectorOptions::Enable(true)),
            }),
            with_vectors: Some(proto::WithVectorsSelector {
                selector_options: Some(proto::with_vectors_selector::SelectorOptions::Enable(true)),
            }),
            read_consistency: None,
            shard_key_selector: None,
            timeout: None,
        };

        let response = self.points().get(request).await?;
        let retrieved_map: std::collections::HashMap<String, proto::RetrievedPoint> = response
            .into_inner()
            .result
            .into_iter()
            .filter_map(|p| {
                let id_str = p.id.as_ref().map(point_id_to_string);
                id_str.map(|id| (id, p))
            })
            .collect();

        Ok(point_ids
            .iter()
            .map(|id| {
                retrieved_map
                    .get(*id)
                    .cloned()
                    .map(retrieved_point_from_proto)
            })
            .collect())
    }

    async fn count(&self, collection: &str) -> Result<u64> {
        let request = proto::CountPoints {
            collection_name: collection.to_string(),
            filter: None,
            exact: None,
            read_consistency: None,
            shard_key_selector: None,
            timeout: None,
        };

        let response = self.points().count(request).await?;
        let count_result = response.into_inner().result;

        Ok(count_result.map(|r| r.count).unwrap_or(0))
    }

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> Result<()> {
        debug!("Setting payload for {} points via gRPC", point_ids.len());

        let ids: Vec<proto::PointId> = point_ids.iter().map(|id| point_id_to_proto(id)).collect();
        let proto_payload = convert::payload_to_proto_map(&payload);

        let points_selector = proto::PointsSelector {
            points_selector_one_of: Some(proto::points_selector::PointsSelectorOneOf::Points(
                proto::PointsIdsList { ids },
            )),
        };

        let request = proto::SetPayloadPoints {
            collection_name: collection.to_string(),
            wait: Some(true),
            payload: proto_payload,
            points_selector: Some(points_selector),
            ordering: None,
            shard_key_selector: None,
            key: None,
        };

        self.points().set_payload(request).await?;
        Ok(())
    }

    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> Result<()> {
        debug!(
            "Deleting payload keys for {} points via gRPC",
            point_ids.len()
        );

        let ids: Vec<proto::PointId> = point_ids.iter().map(|id| point_id_to_proto(id)).collect();
        let keys_owned: Vec<String> = keys.iter().map(|k| k.to_string()).collect();

        let points_selector = proto::PointsSelector {
            points_selector_one_of: Some(proto::points_selector::PointsSelectorOneOf::Points(
                proto::PointsIdsList { ids },
            )),
        };

        let request = proto::DeletePayloadPoints {
            collection_name: collection.to_string(),
            wait: Some(true),
            keys: keys_owned,
            points_selector: Some(points_selector),
            ordering: None,
            shard_key_selector: None,
        };

        self.points().delete_payload(request).await?;
        Ok(())
    }

    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> Result<(Vec<VectorPoint>, Option<String>)> {
        debug!("Scrolling collection '{}' via gRPC", collection);

        let offset_id = offset.map(point_id_to_proto);

        let request = proto::ScrollPoints {
            collection_name: collection.to_string(),
            filter: None,
            offset: offset_id,
            limit: Some(limit as u32),
            with_payload: Some(proto::WithPayloadSelector {
                selector_options: Some(proto::with_payload_selector::SelectorOptions::Enable(
                    with_payload.unwrap_or(true),
                )),
            }),
            with_vectors: Some(proto::WithVectorsSelector {
                selector_options: Some(proto::with_vectors_selector::SelectorOptions::Enable(
                    with_vector.unwrap_or(false),
                )),
            }),
            read_consistency: None,
            shard_key_selector: None,
            order_by: None,
            timeout: None,
        };

        let response = self.points().scroll(request).await?;
        let scroll_result = response.into_inner();

        let points: Vec<VectorPoint> = scroll_result
            .result
            .into_iter()
            .map(retrieved_point_from_proto)
            .collect();

        let next_page = scroll_result
            .next_page_offset
            .map(|id| point_id_to_string(&id));

        Ok((points, next_page))
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> Result<()> {
        debug!("Creating payload index for field '{}' via gRPC", field);

        let field_type = convert::payload_schema_type_to_field_type(schema);

        let request = proto::CreateFieldIndexCollection {
            collection_name: collection.to_string(),
            wait: Some(true),
            field_name: field.to_string(),
            field_type: Some(field_type as i32),
            field_index_params: None,
            ordering: None,
        };

        self.points().create_field_index(request).await?;

        info!(
            "Payload index created for field '{}' in collection '{}'",
            field, collection
        );
        Ok(())
    }

    async fn delete_payload_index(&self, collection: &str, field: &str) -> Result<()> {
        debug!("Deleting payload index for field '{}' via gRPC", field);

        let request = proto::DeleteFieldIndexCollection {
            collection_name: collection.to_string(),
            wait: Some(true),
            field_name: field.to_string(),
            ordering: None,
        };

        self.points().delete_field_index(request).await?;

        info!(
            "Payload index deleted for field '{}' in collection '{}'",
            field, collection
        );
        Ok(())
    }

    async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, PayloadSchemaType)>> {
        let request = proto::GetCollectionInfoRequest {
            collection_name: collection.to_string(),
        };

        let response = self.collections().get(request).await?;
        let info = response
            .into_inner()
            .result
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let mut indexes = Vec::new();
        for (field, schema_info) in info.payload_schema {
            let schema_type = convert::payload_schema_type_from_proto(schema_info.data_type);
            indexes.push((field, schema_type));
        }

        Ok(indexes)
    }
}

fn point_id_to_string(id: &proto::PointId) -> String {
    match &id.point_id_options {
        Some(proto::point_id::PointIdOptions::Num(n)) => n.to_string(),
        Some(proto::point_id::PointIdOptions::Uuid(u)) => u.clone(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_id_to_string_num() {
        let id = proto::PointId {
            point_id_options: Some(proto::point_id::PointIdOptions::Num(42)),
        };
        assert_eq!(point_id_to_string(&id), "42");
    }

    #[test]
    fn test_point_id_to_string_uuid() {
        let id = proto::PointId {
            point_id_options: Some(proto::point_id::PointIdOptions::Uuid("abc-def".into())),
        };
        assert_eq!(point_id_to_string(&id), "abc-def");
    }

    #[test]
    fn test_point_id_to_string_none() {
        let id = proto::PointId {
            point_id_options: None,
        };
        assert_eq!(point_id_to_string(&id), "");
    }
}

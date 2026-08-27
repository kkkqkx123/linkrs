use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::VectorEngine;
use crate::config::VectorClientConfig;
use crate::error::{Result, VectorClientError};
use crate::types::*;

use super::common::convert::extract_search_params;
use super::common::distance_utils::{inbound_result_score, outbound_score_threshold};
use super::common::utils::point_id_to_json;

mod config;
pub mod filter;
mod utils;

use config::{
    build_create_collection_body, build_create_payload_index_body, build_delete_by_filter_body,
    build_delete_by_ids_body, build_delete_payload_body, build_get_body, build_scroll_body,
    build_search_batch_body, build_search_body, build_set_payload_body, build_upsert_body,
    distance_from_qdrant,
};
use filter::convert_filter;
use utils::{parse_payload, QdrantSearchResult, QdrantUpsertResult, VectorValue};

const QDRANT_VERSION: &str = "1.12.x (HTTP REST)";

/// Qdrant exposes two ports on the same host: the gRPC port lives in
/// [`ConnectionConfig::port`], while the REST API keeps its conventional
/// 6333 default here.
const DEFAULT_HTTP_PORT: u16 = 6333;

fn parse_collection_status(status: Option<&str>) -> CollectionStatus {
    match status.map(|value| value.to_ascii_lowercase()).as_deref() {
        Some("green") => CollectionStatus::Green,
        Some("yellow") => CollectionStatus::Yellow,
        Some("red") => CollectionStatus::Red,
        Some("grey") | Some("gray") => CollectionStatus::Grey,
        _ => CollectionStatus::Grey,
    }
}

fn parse_upsert_status(status: Option<&str>) -> UpsertStatus {
    match status.map(|value| value.to_ascii_lowercase()).as_deref() {
        Some("acknowledged") => UpsertStatus::Acknowledged,
        Some("completed") => UpsertStatus::Completed,
        _ => UpsertStatus::Completed,
    }
}

pub struct QdrantEngine {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    config: VectorClientConfig,
    collections: RwLock<HashMap<String, CollectionConfig>>,
}

impl std::fmt::Debug for QdrantEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QdrantEngine")
            .field("base_url", &self.base_url)
            .field("config", &self.config)
            .finish()
    }
}

impl QdrantEngine {
    pub async fn new(config: VectorClientConfig) -> Result<Self> {
        let http_port = config.connection.http_port.unwrap_or(DEFAULT_HTTP_PORT);
        let scheme = if config.connection.use_tls {
            "https"
        } else {
            "http"
        };
        let base_url = format!("{}://{}:{}", scheme, config.connection.host, http_port);

        info!("Connecting to Qdrant HTTP API at {}", base_url);

        let timeout_secs = config.timeout.request_timeout_secs;
        let client_builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .connect_timeout(std::time::Duration::from_secs(
                config.connection.connect_timeout_secs,
            ));

        let client = client_builder
            .build()
            .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))?;

        let engine = Self {
            client,
            base_url,
            api_key: config.connection.api_key.clone(),
            config,
            collections: RwLock::new(HashMap::new()),
        };

        match engine.health_check().await {
            Ok(health) => {
                if health.is_healthy {
                    info!("Successfully connected to Qdrant HTTP API");
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

    async fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.request(method, &url);
        if let Some(ref key) = self.api_key {
            req = req.header("api-key", key);
        }
        req.send()
            .await
            .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))
    }

    async fn request_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.request(method, &url).json(&body);
        if let Some(ref key) = self.api_key {
            req = req.header("api-key", key);
        }
        req.send()
            .await
            .map_err(|e| VectorClientError::ConnectionFailed(e.to_string()))
    }

    async fn check_response(&self, response: reqwest::Response) -> Result<()> {
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let msg = body
            .get("status")
            .and_then(|s| s.get("error"))
            .and_then(|e| e.as_str())
            .unwrap_or("unknown error")
            .to_string();
        let code = status.as_u16();
        Err(match code {
            404 => {
                if msg.contains("Collection") {
                    VectorClientError::CollectionNotFound(msg)
                } else {
                    VectorClientError::PointNotFound(msg, String::new())
                }
            }
            409 => VectorClientError::CollectionAlreadyExists(msg),
            _ => VectorClientError::QdrantHttpError {
                status: code,
                message: msg,
            },
        })
    }

    async fn parse_result<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        let body: Value = response.json().await.map_err(|e| {
            VectorClientError::InternalError(format!("Failed to parse response: {}", e))
        })?;

        if !status.is_success() {
            let msg = body
                .get("status")
                .and_then(|s| s.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error")
                .to_string();
            let code = status.as_u16();
            return Err(match code {
                404 => {
                    if msg.contains("Collection") {
                        VectorClientError::CollectionNotFound(msg)
                    } else {
                        VectorClientError::PointNotFound(msg, String::new())
                    }
                }
                409 => VectorClientError::CollectionAlreadyExists(msg),
                _ => VectorClientError::QdrantHttpError {
                    status: code,
                    message: msg,
                },
            });
        }

        let result = body.get("result").ok_or_else(|| {
            VectorClientError::InternalError("Response missing 'result' field".to_string())
        })?;
        serde_json::from_value(result.clone()).map_err(|e| {
            VectorClientError::InternalError(format!(
                "Failed to deserialize response result: {}",
                e
            ))
        })
    }

    /// Fetch the collection configuration from the server.
    ///
    /// Reads back size, distance, HNSW and quantization settings so callers
    /// never fall back to invented defaults.
    async fn fetch_remote_collection_config(
        &self,
        collection: &str,
    ) -> Result<Option<CollectionConfig>> {
        #[derive(serde::Deserialize)]
        struct RawVectors {
            #[serde(default)]
            size: Option<u64>,
            #[serde(default)]
            distance: Option<String>,
            #[serde(default)]
            hnsw_config: Option<RawHnsw>,
            #[serde(default)]
            quantization_config: Option<Value>,
        }

        #[derive(serde::Deserialize)]
        struct RawHnsw {
            #[serde(default)]
            m: Option<u64>,
            #[serde(default)]
            ef_construct: Option<u64>,
            #[serde(default)]
            full_scan_threshold: Option<u64>,
            #[serde(default)]
            max_indexing_threads: Option<u64>,
            #[serde(default)]
            on_disk: Option<bool>,
            #[serde(default)]
            payload_m: Option<u64>,
        }

        #[derive(serde::Deserialize)]
        struct RawParams {
            #[serde(default)]
            vectors: Option<RawVectors>,
        }

        #[derive(serde::Deserialize)]
        struct RawConfig {
            #[serde(default)]
            params: Option<RawParams>,
        }

        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/collections/{}", collection),
            )
            .await?;
        let raw: Value = self.parse_result(response).await?;

        let config: Option<RawConfig> = serde_json::from_value(raw).map_err(|e| {
            VectorClientError::InternalError(format!("Failed to parse collection config: {}", e))
        })?;

        Ok(config
            .and_then(|c| c.params)
            .and_then(|p| p.vectors)
            .and_then(|v| {
                v.distance
                    .as_deref()
                    .and_then(distance_from_qdrant)
                    .map(|distance| CollectionConfig {
                        vector_size: v.size.unwrap_or_default() as usize,
                        distance,
                        index_type: Some(IndexType::HNSW),
                        hnsw_config: v.hnsw_config.map(|h| HnswConfig {
                            m: h.m.unwrap_or_default() as usize,
                            ef_construct: h.ef_construct.unwrap_or_default() as usize,
                            full_scan_threshold: h.full_scan_threshold.map(|x| x as usize),
                            max_indexing_threads: h.max_indexing_threads.map(|x| x as usize),
                            on_disk: h.on_disk,
                            payload_m: h.payload_m.map(|x| x as usize),
                        }),
                        quantization_config: v
                            .quantization_config
                            .as_ref()
                            .map(parse_quantization_json),
                        ..Default::default()
                    })
            }))
    }
}

/// Rebuild a shared quantization config from its REST representation.
fn parse_quantization_json(qc: &Value) -> QuantizationConfig {
    let quant_type = qc
        .get("scalar")
        .map(|scalar| QuantizationType::Scalar {
            quantile: scalar
                .get("quantile")
                .and_then(Value::as_f64)
                .map(|q| q as f32),
            always_ram: scalar.get("always_ram").and_then(Value::as_bool),
        })
        .or_else(|| {
            qc.get("product").map(|product| {
                let compression = product
                    .get("compression")
                    .and_then(Value::as_u64)
                    .map(|c| match c {
                        8 => CompressionRatio::X8,
                        16 => CompressionRatio::X16,
                        32 => CompressionRatio::X32,
                        64 => CompressionRatio::X64,
                        _ => CompressionRatio::X4,
                    })
                    .unwrap_or(CompressionRatio::X4);
                QuantizationType::Product {
                    compression,
                    always_ram: product.get("always_ram").and_then(Value::as_bool),
                }
            })
        })
        .or_else(|| {
            qc.get("binary").map(|binary| QuantizationType::Binary {
                always_ram: binary.get("always_ram").and_then(Value::as_bool),
            })
        });

    QuantizationConfig {
        enabled: true,
        quant_type,
    }
}

impl QdrantEngine {
    /// Resolve the distance metric of a collection.
    ///
    /// Cached per collection; a cache miss pulls the config from the server
    /// once. Unknown metrics leave Euclid values untouched (with a warning)
    /// because guessing would be worse than passing through.
    async fn metric_for(&self, collection: &str) -> Option<DistanceMetric> {
        self.cached_collection_config(collection)
            .await
            .map(|config| config.distance)
    }

    /// Collection configuration from cache or server; `None` when the server
    /// does not expose one.
    async fn cached_collection_config(&self, name: &str) -> Option<CollectionConfig> {
        if let Some(config) = self.collections.read().await.get(name) {
            return Some(config.clone());
        }
        match self.fetch_remote_collection_config(name).await {
            Ok(Some(config)) => {
                self.collections
                    .write()
                    .await
                    .insert(name.to_string(), config.clone());
                Some(config)
            }
            Ok(None) => {
                warn!(
                    "Server did not report a vector config for collection '{}'; \
                     Euclid scores and thresholds are passed through unconverted",
                    name
                );
                None
            }
            Err(e) => {
                warn!(
                    "Could not fetch collection config for '{}' ({}); \
                     Euclid scores and thresholds are passed through unconverted",
                    name, e
                );
                None
            }
        }
    }

    /// Convert search results, normalizing Euclid raw distances into the
    /// shared similarity contract for the resolved metric.
    fn to_search_results(
        results: Vec<QdrantSearchResult>,
        metric: Option<DistanceMetric>,
    ) -> Vec<SearchResult> {
        results
            .into_iter()
            .map(|r| SearchResult {
                id: r.id,
                score: inbound_result_score(metric, r.score),
                payload: parse_payload(r.payload),
                vector: r.vector.and_then(|v| v.into_vec()),
            })
            .collect()
    }
}

#[async_trait]
impl VectorEngine for QdrantEngine {
    fn name(&self) -> &str {
        "qdrant-http"
    }

    fn version(&self) -> &str {
        QDRANT_VERSION
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        match self.request(reqwest::Method::GET, "/readyz").await {
            Ok(response) => {
                if response.status().is_success() {
                    Ok(HealthStatus::healthy(self.name(), self.version()))
                } else {
                    Ok(HealthStatus::unhealthy(
                        self.name(),
                        self.version(),
                        format!("HTTP {}", response.status()),
                    ))
                }
            }
            Err(e) => Ok(HealthStatus::unhealthy(
                self.name(),
                self.version(),
                e.to_string(),
            )),
        }
    }

    async fn create_collection(&self, name: &str, cfg: CollectionConfig) -> Result<()> {
        debug!("Creating collection '{}' with config: {:?}", name, cfg);

        let body = build_create_collection_body(
            name,
            cfg.vector_size,
            cfg.distance,
            cfg.index_type,
            &cfg.hnsw_config,
            &cfg.quantization_config,
            cfg.on_disk_payload,
            cfg.shard_number,
            cfg.replication_factor,
            cfg.write_consistency_factor,
        )?;

        let response = self
            .request_json(
                reqwest::Method::PUT,
                &format!("/collections/{}", name),
                body,
            )
            .await?;
        self.check_response(response).await?;

        self.collections.write().await.insert(name.to_string(), cfg);

        info!("Collection '{}' created successfully", name);
        Ok(())
    }

    async fn delete_collection(&self, name: &str) -> Result<()> {
        debug!("Deleting collection '{}'", name);
        let response = self
            .request(reqwest::Method::DELETE, &format!("/collections/{}", name))
            .await?;
        self.check_response(response).await?;
        self.collections.write().await.remove(name);
        info!("Collection '{}' deleted successfully", name);
        Ok(())
    }

    async fn collection_exists(&self, name: &str) -> Result<bool> {
        let response = self
            .request(reqwest::Method::GET, &format!("/collections/{}", name))
            .await?;
        Ok(response.status().is_success())
    }

    async fn list_collections(&self) -> Result<Vec<String>> {
        #[derive(serde::Deserialize)]
        struct RawCollection {
            name: String,
        }

        let response = self.request(reqwest::Method::GET, "/collections").await?;
        let raw: Vec<RawCollection> = self.parse_result(response).await?;
        Ok(raw.into_iter().map(|c| c.name).collect())
    }

    async fn collection_info(&self, name: &str) -> Result<CollectionInfo> {
        let response = self
            .request(reqwest::Method::GET, &format!("/collections/{}", name))
            .await?;

        #[derive(serde::Deserialize)]
        struct RawCollectionInfo {
            status: Option<String>,
            points_count: Option<u64>,
            indexed_vectors_count: Option<u64>,
            segments_count: Option<u64>,
            vectors_count: Option<u64>,
        }

        let raw: RawCollectionInfo = self.parse_result(response).await?;

        // No invented defaults: a collection whose config cannot be read back
        // is an error, not a 1536/Cosine lookalike.
        let config = self.cached_collection_config(name).await.ok_or_else(|| {
            VectorClientError::InternalError(format!(
                "collection '{}' exists but its configuration could not be read",
                name
            ))
        })?;

        Ok(CollectionInfo {
            name: name.to_string(),
            vector_count: raw.vectors_count.unwrap_or(0),
            indexed_vector_count: raw.indexed_vectors_count.unwrap_or(0),
            points_count: raw.points_count.unwrap_or(0),
            segments_count: raw.segments_count.unwrap_or(0),
            config,
            status: parse_collection_status(raw.status.as_deref()),
            index: None,
        })
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult> {
        debug!(
            "Upserting point '{}' to collection '{}'",
            point.id, collection
        );

        let id = point_id_to_json(&point.id.to_string());
        let point_json = if let Some(ref payload) = point.payload {
            serde_json::json!({
                "id": id,
                "vector": point.vector,
                "payload": payload,
            })
        } else {
            serde_json::json!({
                "id": id,
                "vector": point.vector,
            })
        };

        let body = build_upsert_body(serde_json::json!([point_json]));
        let response = self
            .request_json(
                reqwest::Method::PUT,
                &format!("/collections/{}/points?wait=true", collection),
                body,
            )
            .await?;
        let result: QdrantUpsertResult = self.parse_result(response).await?;

        Ok(UpsertResult {
            operation_id: result.operation_id,
            status: parse_upsert_status(result.status.as_deref()),
        })
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<UpsertResult> {
        debug!(
            "Upserting {} points to collection '{}'",
            points.len(),
            collection
        );

        let points_json: Vec<Value> = points
            .into_iter()
            .map(|p| {
                let id = point_id_to_json(&p.id.to_string());
                if let Some(ref payload) = p.payload {
                    serde_json::json!({
                        "id": id,
                        "vector": p.vector,
                        "payload": payload,
                    })
                } else {
                    serde_json::json!({
                        "id": id,
                        "vector": p.vector,
                    })
                }
            })
            .collect();

        let body = build_upsert_body(serde_json::json!(points_json));
        let response = self
            .request_json(
                reqwest::Method::PUT,
                &format!("/collections/{}/points?wait=true", collection),
                body,
            )
            .await?;
        let result: QdrantUpsertResult = self.parse_result(response).await?;

        Ok(UpsertResult {
            operation_id: result.operation_id,
            status: parse_upsert_status(result.status.as_deref()),
        })
    }

    async fn delete(&self, collection: &str, point_id: &str) -> Result<DeleteResult> {
        debug!(
            "Deleting point '{}' from collection '{}'",
            point_id, collection
        );

        let id = point_id_to_json(point_id);
        let body = build_delete_by_ids_body(vec![id]);
        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points/delete", collection),
                body,
            )
            .await?;
        self.check_response(response).await?;

        Ok(DeleteResult {
            operation_id: None,
            deleted_count: 1,
        })
    }

    async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<DeleteResult> {
        debug!(
            "Deleting {} points from collection '{}'",
            point_ids.len(),
            collection
        );

        let ids: Vec<Value> = point_ids.iter().map(|id| point_id_to_json(id)).collect();
        let body = build_delete_by_ids_body(ids);
        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points/delete", collection),
                body,
            )
            .await?;
        let result: QdrantUpsertResult = self.parse_result(response).await?;

        // Only report the requested size when the server acknowledges full
        // completion; otherwise the count is unknown (best-effort contract).
        let deleted_count = if result.status.as_deref() == Some("completed") {
            point_ids.len() as u64
        } else {
            0
        };

        Ok(DeleteResult {
            operation_id: result.operation_id,
            deleted_count,
        })
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> Result<DeleteResult> {
        debug!("Deleting points by filter from collection '{}'", collection);

        let filter_value = convert_filter(&filter)?
            .ok_or_else(|| VectorClientError::FilterError("Empty filter".to_string()))?;
        let body = build_delete_by_filter_body(filter_value);
        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points/delete", collection),
                body,
            )
            .await?;
        let result: QdrantUpsertResult = self.parse_result(response).await?;

        Ok(DeleteResult {
            operation_id: result.operation_id,
            deleted_count: if result.status.as_deref() == Some("completed") {
                1
            } else {
                0
            },
        })
    }

    async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
        debug!(
            "Searching in collection '{}' with limit {}",
            collection, query.limit
        );

        let filter_json = if let Some(ref filter) = query.filter {
            convert_filter(filter)?
        } else {
            None
        };

        let metric = self.metric_for(collection).await;
        let params = extract_search_params(&query);
        let score_threshold = params
            .score_threshold
            .and_then(|t| outbound_score_threshold(metric, t));

        let body = build_search_body(
            query.vector,
            params.limit,
            query.offset,
            score_threshold,
            filter_json,
            query.with_payload,
            query.with_vector,
            params.hnsw_ef,
        );

        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points/search", collection),
                body,
            )
            .await?;

        let results: Vec<QdrantSearchResult> = self.parse_result(response).await?;
        Ok(Self::to_search_results(results, metric))
    }

    async fn search_batch(
        &self,
        collection: &str,
        queries: Vec<SearchQuery>,
    ) -> Result<Vec<Vec<SearchResult>>> {
        debug!(
            "Batch searching {} queries in collection '{}'",
            queries.len(),
            collection
        );

        let metric = self.metric_for(collection).await;
        let searches: Result<Vec<Value>> = queries
            .iter()
            .map(|query| {
                let filter_json = if let Some(ref filter) = query.filter {
                    convert_filter(filter)?
                } else {
                    None
                };

                let params = extract_search_params(query);
                let score_threshold = params
                    .score_threshold
                    .and_then(|t| outbound_score_threshold(metric, t));

                Ok(build_search_body(
                    query.vector.clone(),
                    params.limit,
                    query.offset,
                    score_threshold,
                    filter_json,
                    query.with_payload,
                    query.with_vector,
                    params.hnsw_ef,
                ))
            })
            .collect();

        let body = build_search_batch_body(searches?);

        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points/search/batch", collection),
                body,
            )
            .await?;

        let batch_results: Vec<Vec<QdrantSearchResult>> = self.parse_result(response).await?;

        Ok(batch_results
            .into_iter()
            .map(|results| Self::to_search_results(results, metric))
            .collect())
    }

    async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>> {
        debug!(
            "Getting point '{}' from collection '{}'",
            point_id, collection
        );

        let id = point_id_to_json(point_id);
        let body = build_get_body(vec![id], Some(true), Some(true));

        #[derive(serde::Deserialize)]
        struct RawPoint {
            id: crate::types::PointId,
            #[serde(default)]
            payload: Option<Value>,
            #[serde(default)]
            vector: Option<VectorValue>,
        }

        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points", collection),
                body,
            )
            .await?;

        let result: Option<Vec<RawPoint>> = match self.parse_result(response).await {
            Ok(v) => Ok(Some(v)),
            Err(VectorClientError::PointNotFound(_, _)) => Ok(None),
            Err(e) => Err(e),
        }?;

        match result {
            Some(mut points) => {
                if points.is_empty() {
                    return Ok(None);
                }
                let p = points.remove(0);
                Ok(Some(VectorPoint {
                    id: p.id,
                    vector: p.vector.and_then(|v| v.into_vec()).unwrap_or_default(),
                    payload: parse_payload(p.payload),
                }))
            }
            None => Ok(None),
        }
    }

    async fn get_batch(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
    ) -> Result<Vec<Option<VectorPoint>>> {
        debug!(
            "Getting {} points from collection '{}'",
            point_ids.len(),
            collection
        );

        let ids: Vec<Value> = point_ids.iter().map(|id| point_id_to_json(id)).collect();
        let body = build_get_body(ids, Some(true), Some(true));

        #[derive(serde::Deserialize)]
        struct RawPoint {
            id: crate::types::PointId,
            #[serde(default)]
            payload: Option<Value>,
            #[serde(default)]
            vector: Option<VectorValue>,
        }

        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points", collection),
                body,
            )
            .await?;

        let raw_points: Vec<RawPoint> = self.parse_result(response).await?;
        let points_map: HashMap<String, VectorPoint> = raw_points
            .into_iter()
            .map(|p| {
                let id_str = p.id.to_string();
                let vp = VectorPoint {
                    id: p.id,
                    vector: p.vector.and_then(|v| v.into_vec()).unwrap_or_default(),
                    payload: parse_payload(p.payload),
                };
                (id_str, vp)
            })
            .collect();

        Ok(point_ids
            .into_iter()
            .map(|id| points_map.get(id).cloned())
            .collect())
    }

    async fn count(&self, collection: &str) -> Result<u64> {
        #[derive(serde::Deserialize)]
        struct RawInfo {
            points_count: Option<u64>,
        }

        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/collections/{}", collection),
            )
            .await?;
        let info: RawInfo = self.parse_result(response).await?;
        Ok(info.points_count.unwrap_or(0))
    }

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> Result<()> {
        debug!(
            "Setting payload for {} points in collection '{}'",
            point_ids.len(),
            collection
        );

        let ids: Vec<Value> = point_ids.iter().map(|id| point_id_to_json(id)).collect();
        let body = build_set_payload_body(ids, serde_json::to_value(&payload)?);

        let response = self
            .request_json(
                reqwest::Method::PUT,
                &format!("/collections/{}/points/payload", collection),
                body,
            )
            .await?;
        self.check_response(response).await
    }

    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> Result<()> {
        debug!(
            "Deleting payload keys {:?} for {} points in collection '{}'",
            keys,
            point_ids.len(),
            collection
        );

        let ids: Vec<Value> = point_ids.iter().map(|id| point_id_to_json(id)).collect();
        let keys_owned: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        let body = build_delete_payload_body(ids, keys_owned);

        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points/payload/delete", collection),
                body,
            )
            .await?;
        self.check_response(response).await
    }

    async fn set_payload_fields(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        fields: Payload,
    ) -> Result<()> {
        debug!(
            "Merging {} fields into payload for {} points in collection '{}'",
            fields.len(),
            point_ids.len(),
            collection
        );

        let ids: Vec<Value> = point_ids.iter().map(|id| point_id_to_json(id)).collect();
        let body = build_set_payload_body(ids, serde_json::to_value(&fields)?);

        let response = self
            .request_json(
                reqwest::Method::PUT,
                &format!("/collections/{}/points/payload", collection),
                body,
            )
            .await?;
        self.check_response(response).await
    }

    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> Result<(Vec<VectorPoint>, Option<String>)> {
        debug!("Scrolling collection '{}' with limit {}", collection, limit);

        let offset_json = offset.map(point_id_to_json);
        let body = build_scroll_body(limit, offset_json, with_payload, with_vector);

        #[derive(serde::Deserialize)]
        struct RawScrollPoint {
            id: crate::types::PointId,
            #[serde(default)]
            payload: Option<Value>,
            #[serde(default)]
            vector: Option<VectorValue>,
        }

        #[derive(serde::Deserialize)]
        struct ScrollResult {
            points: Vec<RawScrollPoint>,
            next_page_offset: Option<crate::types::PointId>,
        }

        let response = self
            .request_json(
                reqwest::Method::POST,
                &format!("/collections/{}/points/scroll", collection),
                body,
            )
            .await?;

        let scroll_result: ScrollResult = self.parse_result(response).await?;

        let points: Vec<VectorPoint> = scroll_result
            .points
            .into_iter()
            .map(|p| VectorPoint {
                id: p.id,
                vector: p.vector.and_then(|v| v.into_vec()).unwrap_or_default(),
                payload: parse_payload(p.payload),
            })
            .collect();

        let next_page = scroll_result.next_page_offset.map(|id| id.to_string());

        Ok((points, next_page))
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> Result<()> {
        debug!(
            "Creating payload index for field '{}' in collection '{}'",
            field, collection
        );

        let body = build_create_payload_index_body(field, schema);

        let response = self
            .request_json(
                reqwest::Method::PUT,
                &format!("/collections/{}/index", collection),
                body,
            )
            .await?;
        self.check_response(response).await?;

        info!(
            "Payload index created for field '{}' in collection '{}'",
            field, collection
        );
        Ok(())
    }

    async fn delete_payload_index(&self, collection: &str, field: &str) -> Result<()> {
        debug!(
            "Deleting payload index for field '{}' in collection '{}'",
            field, collection
        );

        let response = self
            .request(
                reqwest::Method::DELETE,
                &format!("/collections/{}/index/{}", collection, field),
            )
            .await?;
        self.check_response(response).await?;

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
        debug!("Listing payload indexes for collection '{}'", collection);

        #[derive(serde::Deserialize)]
        struct PayloadSchemaInfo {
            data_type: String,
        }

        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/collections/{}", collection),
            )
            .await?;

        #[derive(serde::Deserialize)]
        struct CollectionInfoWithSchema {
            payload_schema: Option<HashMap<String, PayloadSchemaInfo>>,
        }

        let info: CollectionInfoWithSchema = self.parse_result(response).await?;

        let schema = info.payload_schema.unwrap_or_default();
        let mut indexes = Vec::new();

        for (field, schema_info) in schema {
            let schema_type = match schema_info.data_type.as_str() {
                "keyword" => PayloadSchemaType::Keyword,
                "integer" => PayloadSchemaType::Integer,
                "float" => PayloadSchemaType::Float,
                "text" => PayloadSchemaType::Text,
                "bool" => PayloadSchemaType::Bool,
                "geo" => PayloadSchemaType::Geo,
                "datetime" => PayloadSchemaType::Datetime,
                _ => continue,
            };
            indexes.push((field, schema_type));
        }

        Ok(indexes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_collection_status_green() {
        assert_eq!(
            parse_collection_status(Some("green")),
            CollectionStatus::Green
        );
    }

    #[test]
    fn test_parse_collection_status_yellow() {
        assert_eq!(
            parse_collection_status(Some("yellow")),
            CollectionStatus::Yellow
        );
    }

    #[test]
    fn test_parse_collection_status_red() {
        assert_eq!(parse_collection_status(Some("red")), CollectionStatus::Red);
    }

    #[test]
    fn test_parse_collection_status_grey() {
        assert_eq!(
            parse_collection_status(Some("grey")),
            CollectionStatus::Grey
        );
    }

    #[test]
    fn test_parse_collection_status_gray() {
        assert_eq!(
            parse_collection_status(Some("gray")),
            CollectionStatus::Grey
        );
    }

    #[test]
    fn test_parse_collection_status_case_insensitive() {
        assert_eq!(
            parse_collection_status(Some("GREEN")),
            CollectionStatus::Green
        );
        assert_eq!(
            parse_collection_status(Some("Yellow")),
            CollectionStatus::Yellow
        );
    }

    #[test]
    fn test_parse_collection_status_unknown() {
        assert_eq!(
            parse_collection_status(Some("unknown")),
            CollectionStatus::Grey
        );
    }

    #[test]
    fn test_parse_collection_status_none() {
        assert_eq!(parse_collection_status(None), CollectionStatus::Grey);
    }

    #[test]
    fn test_parse_upsert_status_acknowledged() {
        assert_eq!(
            parse_upsert_status(Some("acknowledged")),
            UpsertStatus::Acknowledged
        );
    }

    #[test]
    fn test_parse_upsert_status_completed() {
        assert_eq!(
            parse_upsert_status(Some("completed")),
            UpsertStatus::Completed
        );
    }

    #[test]
    fn test_parse_upsert_status_unknown() {
        assert_eq!(
            parse_upsert_status(Some("unknown_status")),
            UpsertStatus::Completed
        );
    }

    #[test]
    fn test_parse_upsert_status_none() {
        assert_eq!(parse_upsert_status(None), UpsertStatus::Completed);
    }

    #[test]
    fn test_parse_upsert_status_case_insensitive() {
        assert_eq!(
            parse_upsert_status(Some("COMPLETED")),
            UpsertStatus::Completed
        );
        assert_eq!(
            parse_upsert_status(Some("Acknowledged")),
            UpsertStatus::Acknowledged
        );
    }

    #[test]
    fn test_name_and_version() {
        let config = crate::config::VectorClientConfig::disabled();
        let engine = QdrantEngine {
            client: reqwest::Client::new(),
            base_url: "http://localhost:6333".into(),
            api_key: None,
            config,
            collections: RwLock::new(HashMap::new()),
        };
        assert_eq!(engine.name(), "qdrant-http");
        assert!(engine.version().contains("HTTP REST"));
    }
}

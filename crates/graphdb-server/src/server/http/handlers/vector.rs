use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};

use crate::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::sync::vector_sync::SearchOptions;
use vector_search::{DistanceMetric, VectorFilter};

/// Vector index creation request
#[derive(Debug, Deserialize)]
pub struct CreateVectorIndexRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub vector_size: usize,
    #[serde(default = "default_distance")]
    pub distance: DistanceMetric,
    /// HNSW overrides
    pub hnsw_m: Option<usize>,
    pub hnsw_ef_construct: Option<usize>,
    /// Quantization: scalar/binary/product/none (case-insensitive). None = disabled.
    pub quantization: Option<String>,
    /// Scalar only: quantile in (0,1]
    pub quantile: Option<f32>,
    /// Product only: x4/x8/x16/x32/x64 or integer 4/8/16/32/64
    pub compression: Option<String>,
    /// Keep quantized vectors in RAM
    pub always_ram: Option<bool>,
}

fn default_distance() -> DistanceMetric {
    DistanceMetric::Cosine
}

/// Vector index information
#[derive(Debug, Serialize)]
pub struct VectorIndexInfo {
    pub name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub vector_size: usize,
    pub distance: String,
    pub points_count: u64,
}

/// Vector search request
#[derive(Debug, Deserialize)]
pub struct VectorSearchRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub query_vector: Vec<f32>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub threshold: Option<f32>,
    pub filter: Option<VectorFilter>,
}

fn default_limit() -> usize {
    10
}

/// Vector search result
#[derive(Debug, Serialize)]
pub struct VectorSearchResponse {
    pub results: Vec<VectorSearchResult>,
    pub count: usize,
}

/// Single vector search result
#[derive(Debug, Serialize)]
pub struct VectorSearchResult {
    pub id: String,
    pub score: f32,
    pub vector: Option<Vec<f32>>,
    pub payload: Option<serde_json::Map<String, serde_json::Value>>,
}

/// List of vector indexes response
#[derive(Debug, Serialize)]
pub struct ListVectorIndexesResponse {
    pub indexes: Vec<String>,
    pub count: usize,
}

/// Vector index details response
#[derive(Debug, Serialize)]
pub struct VectorIndexDetailsResponse {
    pub collection_name: String,
    pub status: String,
    pub vectors_count: u64,
    pub points_count: u64,
    pub indexed_vectors_count: u64,
    pub vector_size: usize,
    pub distance: String,
}

/// Create a vector index
pub async fn create_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<CreateVectorIndexRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let collection_name = if request.quantization.is_some()
            || request.hnsw_m.is_some()
            || request.hnsw_ef_construct.is_some()
            || request.quantile.is_some()
            || request.compression.is_some()
            || request.always_ram.is_some()
        {
            let mut config =
                vector_search::CollectionConfig::new(request.vector_size, request.distance);
            if request.hnsw_m.is_some() || request.hnsw_ef_construct.is_some() {
                let mut hnsw = vector_search::HnswConfig::default();
                if let Some(m) = request.hnsw_m {
                    hnsw.m = m;
                }
                if let Some(ef) = request.hnsw_ef_construct {
                    hnsw.ef_construct = ef;
                }
                config = config.with_hnsw(hnsw);
            }
            if let Some(ref q) = request.quantization {
                let q_lower = q.to_lowercase();
                let quant_cfg = match q_lower.as_str() {
                    "none" | "disabled" | "off" => None,
                    "scalar" => {
                        let mut cfg = vector_search::QuantizationConfig::scalar(
                            request.quantile.unwrap_or(0.99),
                        );
                        if let Some(ar) = request.always_ram {
                            cfg = cfg.with_always_ram(ar);
                        }
                        Some(cfg)
                    }
                    "binary" => {
                        let mut cfg = vector_search::QuantizationConfig::binary();
                        if let Some(ar) = request.always_ram {
                            cfg = cfg.with_always_ram(ar);
                        }
                        Some(cfg)
                    }
                    "product" | "pq" => {
                        let ratio = match request
                            .compression
                            .as_deref()
                            .unwrap_or("x4")
                            .to_lowercase()
                            .as_str()
                        {
                            "x4" | "4" => vector_search::CompressionRatio::X4,
                            "x8" | "8" => vector_search::CompressionRatio::X8,
                            "x16" | "16" => vector_search::CompressionRatio::X16,
                            "x32" | "32" => vector_search::CompressionRatio::X32,
                            "x64" | "64" => vector_search::CompressionRatio::X64,
                            other => {
                                return Err(HttpError::InternalError(format!(
                                    "unknown compression '{}', expected x4/x8/x16/x32/x64",
                                    other
                                )))
                            }
                        };
                        let mut cfg = vector_search::QuantizationConfig::product(ratio);
                        if let Some(ar) = request.always_ram {
                            cfg = cfg.with_always_ram(ar);
                        }
                        Some(cfg)
                    }
                    other => {
                        return Err(HttpError::InternalError(format!(
                            "unknown quantization '{}', expected scalar/binary/product/none",
                            other
                        )))
                    }
                };
                if let Some(qc) = quant_cfg {
                    config = config.with_quantization(qc);
                }
            }
            vector_api
                .create_index_with_config(
                    request.space_id,
                    &request.tag_name,
                    &request.field_name,
                    config,
                )
                .await
                .map_err(|e| HttpError::InternalError(e.to_string()))?
        } else {
            vector_api
                .create_index(
                    request.space_id,
                    &request.tag_name,
                    &request.field_name,
                    request.vector_size,
                    request.distance,
                )
                .await
                .map_err(|e| HttpError::InternalError(e.to_string()))?
        };

        Ok(JsonResponse(serde_json::json!({
            "success": true,
            "message": "Vector index created successfully",
            "collection_name": collection_name
        })))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Drop a vector index
pub async fn drop_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((space_id, tag_name, field_name)): Path<(u64, String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        vector_api
            .drop_index(space_id, &tag_name, &field_name)
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(serde_json::json!({
            "success": true,
            "message": "Vector index dropped successfully"
        })))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Get vector index info
pub async fn get_index_info<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((space_id, tag_name, field_name)): Path<(u64, String, String)>,
) -> Result<JsonResponse<VectorIndexDetailsResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        match vector_api.get_index_info(space_id, &tag_name, &field_name) {
            Ok(Some(info)) => Ok(JsonResponse(VectorIndexDetailsResponse {
                collection_name: format!("space_{}_{}_{}", space_id, tag_name, field_name),
                status: "green".to_string(),
                vectors_count: info.vector_count,
                points_count: 0,
                indexed_vectors_count: 0,
                vector_size: info.config.vector_size,
                distance: format!("{:?}", info.config.distance),
            })),
            Ok(None) => Err(HttpError::NotFound("Vector index not found".to_string())),
            Err(e) => Err(HttpError::InternalError(e.to_string())),
        }
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// List all vector indexes
pub async fn list_indexes<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
) -> Result<JsonResponse<ListVectorIndexesResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let indexes = vector_api.list_indexes();
        let count = indexes.len();
        Ok(JsonResponse(ListVectorIndexesResponse { indexes, count }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Search vectors
pub async fn search<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<VectorSearchRequest>,
) -> Result<JsonResponse<VectorSearchResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let mut options = SearchOptions::new(
            request.space_id,
            &request.tag_name,
            &request.field_name,
            request.query_vector,
            request.limit,
        );

        if let Some(threshold) = request.threshold {
            options = options.with_threshold(threshold);
        }

        if let Some(filter) = request.filter {
            options = options.with_filter(filter);
        }

        let results = vector_api
            .search_with_options(options)
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        let count = results.len();
        let search_results: Vec<VectorSearchResult> = results
            .into_iter()
            .map(|r| VectorSearchResult {
                id: r.id.to_string(),
                score: r.score,
                vector: r.vector.map(|v| v.to_vec()),
                payload: r.payload.map(|p| p.into_iter().collect()),
            })
            .collect();

        Ok(JsonResponse(VectorSearchResponse {
            results: search_results,
            count,
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Get vector point by ID
pub async fn get_vector<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((space_id, tag_name, field_name, point_id)): Path<(u64, String, String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let point = vector_api
            .get_vector(space_id, &tag_name, &field_name, &point_id)
            .await
            .map_err(|e: graphdb_api::api::core::error::CoreError| {
                HttpError::InternalError(e.to_string())
            })?;

        match point {
            Some(p) => Ok(JsonResponse(serde_json::json!({
                "success": true,
                "point": {
                    "id": p.id,
                    "vector": p.vector,
                    "payload": p.payload.map(|payload| serde_json::to_value(payload).unwrap_or(serde_json::Value::Null))
                }
            }))),
            None => Ok(JsonResponse(serde_json::json!({
                "success": false,
                "message": "Vector point not found"
            }))),
        }
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Get vector index count
pub async fn count<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((space_id, tag_name, field_name)): Path<(u64, String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let count = vector_api
            .count(space_id, &tag_name, &field_name)
            .await
            .map_err(|e: graphdb_api::api::core::error::CoreError| {
                HttpError::InternalError(e.to_string())
            })?;

        Ok(JsonResponse(serde_json::json!({
            "success": true,
            "count": count
        })))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Set payload request
#[derive(Debug, Deserialize)]
pub struct SetPayloadRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub point_ids: Vec<String>,
    pub payload: serde_json::Map<String, serde_json::Value>,
}

/// Delete payload request
#[derive(Debug, Deserialize)]
pub struct DeletePayloadRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub point_ids: Vec<String>,
    pub keys: Vec<String>,
}

/// Scroll request
#[derive(Debug, Deserialize)]
pub struct ScrollRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub offset: Option<String>,
    pub with_payload: Option<bool>,
    pub with_vector: Option<bool>,
}

/// Scroll response
#[derive(Debug, Serialize)]
pub struct ScrollResponse {
    pub points: Vec<VectorSearchResult>,
    pub next_offset: Option<String>,
}

/// Set payload for vector points
pub async fn set_payload<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<SetPayloadRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let payload: vector_search::types::Payload = request.payload.into_iter().collect();
        let point_ids: Vec<&str> = request.point_ids.iter().map(|s| s.as_str()).collect();
        vector_api
            .set_payload(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                point_ids,
                payload,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(serde_json::json!({
            "success": true,
            "message": "Payload set successfully"
        })))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Merge fields into payload for vector points
pub async fn set_payload_fields<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<SetPayloadRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let fields: vector_search::types::Payload = request.payload.into_iter().collect();
        let point_ids: Vec<&str> = request.point_ids.iter().map(|s| s.as_str()).collect();
        vector_api
            .set_payload_fields(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                point_ids,
                fields,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(serde_json::json!({
            "success": true,
            "message": "Payload fields merged successfully"
        })))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Delete payload keys from vector points
pub async fn delete_payload<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<DeletePayloadRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let point_ids: Vec<&str> = request.point_ids.iter().map(|s| s.as_str()).collect();
        let keys: Vec<&str> = request.keys.iter().map(|s| s.as_str()).collect();
        vector_api
            .delete_payload(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                point_ids,
                keys,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(serde_json::json!({
            "success": true,
            "message": "Payload keys deleted successfully"
        })))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Paginated scroll over vector points
pub async fn scroll<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<ScrollRequest>,
) -> Result<JsonResponse<ScrollResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let (points, next_offset) = vector_api
            .scroll(graphdb_api::api::core::vector_api::ScrollQuery {
                space_id: request.space_id,
                tag_name: &request.tag_name,
                field_name: &request.field_name,
                limit: request.limit,
                offset: request.offset.as_deref(),
                with_payload: request.with_payload,
                with_vector: request.with_vector,
            })
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        let results: Vec<VectorSearchResult> = points
            .into_iter()
            .map(|p| VectorSearchResult {
                id: p.id.to_string(),
                score: 0.0,
                vector: if request.with_vector.unwrap_or(false) {
                    Some(p.vector)
                } else {
                    None
                },
                payload: p.payload.map(|pay| pay.into_iter().collect()),
            })
            .collect();

        Ok(JsonResponse(ScrollResponse {
            points: results,
            next_offset,
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

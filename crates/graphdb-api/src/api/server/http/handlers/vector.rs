use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};
use crate::sync::vector_sync::SearchOptions;
use vector_client::{DistanceMetric, VectorFilter};

/// Vector index creation request
#[derive(Debug, Deserialize)]
pub struct CreateVectorIndexRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub vector_size: usize,
    #[serde(default = "default_distance")]
    pub distance: DistanceMetric,
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
        + StorageTransactionContextOps
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
        let collection_name = vector_api
            .create_index(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                request.vector_size,
                request.distance,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

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
        + StorageTransactionContextOps
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
        + StorageTransactionContextOps
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
        + StorageTransactionContextOps
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
        + StorageTransactionContextOps
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
                payload: None,
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
        + StorageTransactionContextOps
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
            .map_err(|e: crate::api::core::error::CoreError| {
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
        + StorageTransactionContextOps
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
            .map_err(|e: crate::api::core::error::CoreError| {
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

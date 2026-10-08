//! Space handlers for the web management view.
//!
//! Covers space listing, details and statistics.

use axum::{
    extract::{Path, State},
    response::Json,
};
use tokio::task;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{
    error::{WebError, WebResult},
    models::{
        schema::{SpaceDetail, SpaceStatistics},
        ApiResponse,
    },
    WebState,
};

// ==================== Space Handlers ====================

/// List all spaces
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_spaces",
    path = "/api/v1/schema/spaces",
    tag = "WebSchema",
    responses(
        (status = 200, description = "Space list", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
pub async fn list_spaces<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    // Get storage reference before spawn_blocking
    let storage = web_state.core_state.server.get_storage();

    let result = task::spawn_blocking(move || {
        let storage = storage.read();

        let spaces = storage
            .list_spaces()
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let space_list: Vec<serde_json::Value> = spaces
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.space_id,
                    "name": s.space_name,
                    "vid_type": format!("{:?}", s.vid_type),
                })
            })
            .collect();

        Ok::<_, WebError>(serde_json::json!({
            "spaces": space_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Get space details
#[utoipa::path(
    get,
    path = "/api/v1/schema/spaces/{name}/details",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, description = "Space detail", body = ApiResponse<SpaceDetail>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_space_details<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Path(name): Path<String>,
) -> WebResult<Json<ApiResponse<SpaceDetail>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        let space_info = storage
            .get_space(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", name)))?;

        // Get statistics
        let tags = storage
            .list_tags(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_types = storage
            .list_edge_types(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let tag_indexes = storage
            .list_tag_indexes(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_indexes: Vec<linkrs_core::types::Index> = Vec::new();

        Ok::<_, WebError>(SpaceDetail {
            id: space_info.space_id,
            name: space_info.space_name,
            vid_type: format!("{:?}", space_info.vid_type),
            partition_num: 100,
            replica_factor: 1,
            comment: None,
            created_at: 0,
            statistics: SpaceStatistics {
                tag_count: tags.len() as i64,
                edge_type_count: edge_types.len() as i64,
                index_count: (tag_indexes.len() + edge_indexes.len()) as i64,
                estimated_vertex_count: 0,
                estimated_edge_count: 0,
            },
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Get space statistics
#[utoipa::path(
    get,
    path = "/api/v1/schema/spaces/{name}/statistics",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, description = "Space statistics", body = ApiResponse<SpaceStatistics>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_space_statistics<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Path(name): Path<String>,
) -> WebResult<Json<ApiResponse<SpaceStatistics>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Verify space exists
        let _ = storage
            .get_space(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", name)))?;

        let tags = storage
            .list_tags(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_types = storage
            .list_edge_types(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let tag_indexes = storage
            .list_tag_indexes(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_indexes: Vec<linkrs_core::types::Index> = Vec::new();

        Ok::<_, WebError>(SpaceStatistics {
            tag_count: tags.len() as i64,
            edge_type_count: edge_types.len() as i64,
            index_count: (tag_indexes.len() + edge_indexes.len()) as i64,
            estimated_vertex_count: 0,
            estimated_edge_count: 0,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

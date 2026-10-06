//! Index handlers for the web management view.
//!
//! Covers index listing, creation, details, deletion and rebuild.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::Json,
};
use tokio::task;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{
    error::{WebError, WebResult},
    models::{
        schema::{CreateIndexRequest, IndexInfo},
        ApiResponse,
    },
    WebState,
};

// ==================== Index Handlers ====================

/// List all indexes in a space
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_spaces_name_indexes",
    path = "/api/v1/schema/spaces/{name}/indexes",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, description = "Index list", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
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
    State(web_state): State<WebState<S>>,
    Path(space_name): Path<String>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Verify space exists
        let space_info = storage
            .get_space(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", space_name)))?;

        let tag_indexes = storage
            .list_tag_indexes(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let mut index_list: Vec<IndexInfo> = Vec::new();

        for (idx, index) in tag_indexes.iter().enumerate() {
            index_list.push(IndexInfo {
                id: idx as i64,
                name: index.name.clone(),
                index_type: format!("{:?}", index.index_type),
                fields: index.fields.iter().map(|f| f.name.clone()).collect(),
                status: format!("{:?}", index.status),
                progress: None,
                created_at: 0,
            });
        }
        Ok::<_, WebError>(serde_json::json!({
            "space": space_name,
            "space_id": space_info.space_id,
            "indexes": index_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Create a new index
#[utoipa::path(
    post,
    operation_id = "post_api_v1_schema_spaces_name_indexes",
    path = "/api/v1/schema/spaces/{name}/indexes",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    request_body = CreateIndexRequest,
    responses(
        (status = 200, description = "Index created", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
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
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(space_name): Path<String>,
    Json(request): Json<CreateIndexRequest>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        let target = match request.entity_type.as_str() {
            "TAG" => graphdb_api::api_core::IndexTarget::Tag {
                name: request.entity_name,
                fields: request.fields,
            },
            "EDGE" => graphdb_api::api_core::IndexTarget::Edge {
                name: request.entity_name,
                fields: request.fields,
            },
            _ => {
                return Err(WebError::BadRequest(format!(
                    "Invalid entity_type: {}",
                    request.entity_type
                )))
            }
        };

        schema_api
            .create_index(space_id, &request.name, target)
            .map_err(|e| WebError::Internal(format!("Failed to create index: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "id": 0,
            "name": request.name,
            "space": space_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::CREATED, Json(ApiResponse::success(result?))))
}

/// Get index details
#[utoipa::path(
    get,
    path = "/api/v1/schema/spaces/{name}/indexes/{index_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("index_name" = String, Path, description = "Index name")
    ),
    responses(
        (status = 200, description = "Index detail", body = ApiResponse<IndexInfo>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_index<
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
    Path((space_name, index_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<IndexInfo>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Try to get tag index first
        if let Some(index) = storage
            .get_tag_index(&space_name, &index_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
        {
            return Ok::<_, WebError>(IndexInfo {
                id: 0,
                name: index.name,
                index_type: format!("{:?}", index.index_type),
                fields: index.fields.iter().map(|f| f.name.clone()).collect(),
                status: format!("{:?}", index.status),
                progress: None,
                created_at: 0,
            });
        }

        Err(WebError::NotFound(format!(
            "Index '{}' not found in space '{}'",
            index_name, space_name
        )))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Delete index
#[utoipa::path(
    delete,
    path = "/api/v1/schema/spaces/{name}/indexes/{index_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("index_name" = String, Path, description = "Index name")
    ),
    responses(
        (status = 200, description = "Index deleted", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn delete_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, index_name)): Path<(String, String)>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_index(space_id, &index_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete index: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "deleted": true,
            "name": index_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

/// Rebuild index
#[utoipa::path(
    post,
    path = "/api/v1/schema/spaces/{name}/indexes/{index_name}/rebuild",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("index_name" = String, Path, description = "Index name")
    ),
    responses(
        (status = 200, description = "Index rebuilt", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn rebuild_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, index_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .rebuild_index(space_id, &index_name)
            .map_err(|e| WebError::Internal(format!("Failed to rebuild index: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "rebuilt": true,
            "name": index_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

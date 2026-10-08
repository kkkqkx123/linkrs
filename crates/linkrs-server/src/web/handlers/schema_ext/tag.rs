//! Tag handlers for the web management view.
//!
//! Covers tag listing, creation, details, updates and deletion.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::Json,
};
use tokio::task;

use super::common::parse_data_type;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{
    error::{WebError, WebResult},
    models::{
        schema::{PropertyDef, TagDetail, TagSummary, UpdateTagRequest},
        ApiResponse,
    },
    WebState,
};
// ==================== Tag Handlers ====================

/// List all tags in a space
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_spaces_name_tags",
    path = "/api/v1/schema/spaces/{name}/tags",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, description = "Tag list", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn list_tags<
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
        let _ = storage
            .get_space(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", space_name)))?;

        let tags = storage
            .list_tags(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let tag_list: Vec<TagSummary> = tags
            .into_iter()
            .enumerate()
            .map(|(idx, t)| TagSummary {
                id: idx as i64 + 1,
                name: t.tag_name,
                property_count: t.properties.len() as i64,
                index_count: 0,
                created_at: 0,
            })
            .collect();

        Ok::<_, WebError>(serde_json::json!({
            "space": space_name,
            "tags": tag_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Create a new tag
#[utoipa::path(
    post,
    operation_id = "post_api_v1_schema_spaces_name_tags",
    path = "/api/v1/schema/spaces/{name}/tags",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Tag created", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn create_tag<
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
    Json(request): Json<serde_json::Value>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let tag_name = request
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WebError::BadRequest("Tag name is required".to_string()))?
        .to_string();

    // Parse properties before spawn_blocking
    let properties: Vec<linkrs_api::api_core::PropertyDef> = request
        .get("properties")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let name = p.get("name")?.as_str()?;
                    let data_type_str = p.get("data_type")?.as_str()?;
                    let data_type = parse_data_type(data_type_str)?;
                    Some(linkrs_api::api_core::PropertyDef {
                        name: name.to_string(),
                        data_type,
                        nullable: p.get("nullable").and_then(|v| v.as_bool()).unwrap_or(true),
                        default_value: None,
                        comment: p
                            .get("comment")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        // Get space ID
        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .create_tag(space_id, &tag_name, properties)
            .map_err(|e| WebError::Internal(format!("Failed to create tag: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "id": 0,
            "name": tag_name,
            "space": space_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::CREATED, Json(ApiResponse::success(result?))))
}

/// Get tag details
#[utoipa::path(
    get,
    path = "/api/v1/schema/spaces/{name}/tags/{tag_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("tag_name" = String, Path, description = "Tag name")
    ),
    responses(
        (status = 200, description = "Tag detail", body = ApiResponse<TagDetail>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_tag<
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
    Path((space_name, tag_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<TagDetail>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        let tag_info = storage
            .get_tag(&space_name, &tag_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| {
                WebError::NotFound(format!(
                    "Tag '{}' not found in space '{}'",
                    tag_name, space_name
                ))
            })?;

        let properties: Vec<PropertyDef> = tag_info
            .properties
            .into_iter()
            .map(|p| PropertyDef {
                name: p.name,
                data_type: format!("{:?}", p.data_type),
                nullable: p.nullable,
                default_value: p.default.map(|v| format!("{:?}", v)),
            })
            .collect();

        Ok::<_, WebError>(TagDetail {
            id: 0,
            name: tag_info.tag_name,
            properties,
            indexes: vec![],
            created_at: 0,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Update tag
#[utoipa::path(
    put,
    path = "/api/v1/schema/spaces/{name}/tags/{tag_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("tag_name" = String, Path, description = "Tag name")
    ),
    request_body = UpdateTagRequest,
    responses(
        (status = 200, description = "Tag updated", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn update_tag<
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
    Path((space_name, tag_name)): Path<(String, String)>,
    Json(request): Json<UpdateTagRequest>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        // Convert web PropertyDef to core PropertyDef
        let additions = if let Some(props) = request.add_properties {
            let mut core_props = Vec::new();
            for p in props {
                let data_type = parse_data_type(&p.data_type).ok_or_else(|| {
                    WebError::BadRequest(format!("Invalid data type: {}", p.data_type))
                })?;
                core_props.push(linkrs_api::api_core::PropertyDef {
                    name: p.name,
                    data_type,
                    nullable: p.nullable,
                    default_value: None,
                    comment: None,
                });
            }
            core_props
        } else {
            Vec::new()
        };

        let deletions = request.drop_properties.unwrap_or_default();

        schema_api
            .alter_tag(space_id, &tag_name, additions, deletions)
            .map_err(|e| WebError::Internal(format!("Failed to update tag: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "updated": true,
            "name": tag_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Delete tag
#[utoipa::path(
    delete,
    path = "/api/v1/schema/spaces/{name}/tags/{tag_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("tag_name" = String, Path, description = "Tag name")
    ),
    responses(
        (status = 200, description = "Tag deleted", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn delete_tag<
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
    Path((space_name, tag_name)): Path<(String, String)>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_tag(space_id, &tag_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete tag: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "deleted": true,
            "name": tag_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

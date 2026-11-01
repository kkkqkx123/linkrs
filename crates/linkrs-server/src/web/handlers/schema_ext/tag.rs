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
use crate::value::from_json as json_to_core;
use crate::web::{
    error::{WebError, WebResult},
    models::{
        schema::{PropertyDef, TagDetail, UpdateTagRequest},
        ApiResponse,
    },
    WebState,
};
use linkrs_wire::schema::{
    CreateTagRequest, FieldInfo, TagCreateResponse, TagDeleteResponse, TagInfo, TagListResponse,
    TagUpdateResponse,
};
use linkrs_api::api_core::PropertyDef as CorePropertyDef;
// ==================== Tag Handlers ====================

/// List all tags in a space
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_spaces_name_tags",
    path = "/api/v1/schema/spaces/{name}/tags",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, description = "Tag list", body = ApiResponse<TagListResponse>),
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
) -> WebResult<Json<ApiResponse<TagListResponse>>> {
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

        let tag_list: Vec<TagInfo> = tags
            .into_iter()
            .map(|t| TagInfo {
                name: t.tag_name,
                fields: t
                    .properties
                    .iter()
                    .map(|p| FieldInfo {
                        name: p.name.clone(),
                        data_type: format!("{:?}", p.data_type),
                        nullable: p.nullable,
                        default_value: None,
                    })
                    .collect(),
            })
            .collect();

        Ok::<_, WebError>(TagListResponse {
            tags: tag_list,
            space_name,
        })
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
    request_body = CreateTagRequest,
    responses(
        (status = 200, description = "Tag created", body = ApiResponse<TagCreateResponse>),
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
    Json(request): Json<CreateTagRequest>,
) -> WebResult<(StatusCode, Json<ApiResponse<TagCreateResponse>>)> {
    if request.name.trim().is_empty() {
        return Err(WebError::BadRequest("Tag name is required".to_string()));
    }
    let tag_name = request.name.clone();
    let properties: Vec<CorePropertyDef> = request
        .properties
        .into_iter()
        .map(|p| CorePropertyDef {
            name: p.name,
            data_type: parse_data_type(&p.data_type).unwrap_or(linkrs_core::DataType::String),
            nullable: p.nullable,
            default_value: p.default_value.map(|v| json_to_core(&v)),
            comment: p.comment,
        })
        .collect();

    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        // Get space ID
        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .create_tag(space_id, &tag_name, properties)
            .map_err(|e| WebError::Internal(format!("Failed to create tag: {}", e)))?;

        Ok::<_, WebError>(TagCreateResponse {
            message: "Tag created successfully".to_string(),
            tag_name,
            space_name,
        })
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
        (status = 200, description = "Tag updated", body = ApiResponse<TagUpdateResponse>),
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
) -> WebResult<Json<ApiResponse<TagUpdateResponse>>> {
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
                core_props.push(CorePropertyDef {
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

        Ok::<_, WebError>(TagUpdateResponse {
            message: "Tag updated successfully".to_string(),
            tag_name,
            space_name,
        })
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
        (status = 200, description = "Tag deleted", body = ApiResponse<TagDeleteResponse>),
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
) -> WebResult<(StatusCode, Json<ApiResponse<TagDeleteResponse>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_tag(space_id, &tag_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete tag: {}", e)))?;

        Ok::<_, WebError>(TagDeleteResponse {
            message: "Tag deleted successfully".to_string(),
            tag_name,
            space_name,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

//! Edge type handlers for the web management view.
//!
//! Covers edge type listing, creation, details, updates and deletion.

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
        schema::{EdgeTypeDetail, EdgeTypeSummary, PropertyDef, UpdateEdgeTypeRequest},
        ApiResponse,
    },
    WebState,
};
// ==================== Edge Type Handlers ====================

/// List all edge types in a space
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_spaces_name_edge_types",
    path = "/api/v1/schema/spaces/{name}/edge-types",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, description = "Edge type list", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn list_edge_types<
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

        let edge_types = storage
            .list_edge_types(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let edge_type_list: Vec<EdgeTypeSummary> = edge_types
            .into_iter()
            .enumerate()
            .map(|(idx, e)| EdgeTypeSummary {
                id: idx as i64 + 1,
                name: e.edge_type_name,
                property_count: e.properties.len() as i64,
                index_count: 0,
                created_at: 0,
            })
            .collect();

        Ok::<_, WebError>(serde_json::json!({
            "space": space_name,
            "edge_types": edge_type_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Create a new edge type
#[utoipa::path(
    post,
    operation_id = "post_api_v1_schema_spaces_name_edge_types",
    path = "/api/v1/schema/spaces/{name}/edge-types",
    tag = "WebSchema",
    params(("name" = String, Path, description = "Space name")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Edge type created", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn create_edge_type<
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
    let edge_name = request
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WebError::BadRequest("Edge type name is required".to_string()))?
        .to_string();

    // Parse properties before spawn_blocking
    let properties: Vec<graphdb_api::api_core::PropertyDef> = request
        .get("properties")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let name = p.get("name")?.as_str()?;
                    let data_type_str = p.get("data_type")?.as_str()?;
                    let data_type = parse_data_type(data_type_str)?;
                    Some(graphdb_api::api_core::PropertyDef {
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

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .create_edge_type(space_id, &edge_name, properties)
            .map_err(|e| WebError::Internal(format!("Failed to create edge type: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "id": 0,
            "name": edge_name,
            "space": space_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::CREATED, Json(ApiResponse::success(result?))))
}

/// Get edge type details
#[utoipa::path(
    get,
    path = "/api/v1/schema/spaces/{name}/edge-types/{edge_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("edge_name" = String, Path, description = "Edge type name")
    ),
    responses(
        (status = 200, description = "Edge type detail", body = ApiResponse<EdgeTypeDetail>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_edge_type<
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
    Path((space_name, edge_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<EdgeTypeDetail>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        let edge_info = storage
            .get_edge_type(&space_name, &edge_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| {
                WebError::NotFound(format!(
                    "Edge type '{}' not found in space '{}'",
                    edge_name, space_name
                ))
            })?;

        let properties: Vec<PropertyDef> = edge_info
            .properties
            .into_iter()
            .map(|p| PropertyDef {
                name: p.name,
                data_type: format!("{:?}", p.data_type),
                nullable: p.nullable,
                default_value: p.default.map(|v| format!("{:?}", v)),
            })
            .collect();

        Ok::<_, WebError>(EdgeTypeDetail {
            id: 0,
            name: edge_info.edge_type_name,
            properties,
            indexes: vec![],
            created_at: 0,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Update edge type
#[utoipa::path(
    put,
    path = "/api/v1/schema/spaces/{name}/edge-types/{edge_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("edge_name" = String, Path, description = "Edge type name")
    ),
    request_body = UpdateEdgeTypeRequest,
    responses(
        (status = 200, description = "Edge type updated", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn update_edge_type<
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
    Path((space_name, edge_name)): Path<(String, String)>,
    Json(request): Json<UpdateEdgeTypeRequest>,
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
                core_props.push(graphdb_api::api_core::PropertyDef {
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
            .alter_edge_type(space_id, &edge_name, additions, deletions)
            .map_err(|e| WebError::Internal(format!("Failed to update edge type: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "updated": true,
            "name": edge_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Delete edge type
#[utoipa::path(
    delete,
    path = "/api/v1/schema/spaces/{name}/edge-types/{edge_name}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("edge_name" = String, Path, description = "Edge type name")
    ),
    responses(
        (status = 200, description = "Edge type deleted", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn delete_edge_type<
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
    Path((space_name, edge_name)): Path<(String, String)>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_edge_type(space_id, &edge_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete edge type: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "deleted": true,
            "name": edge_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

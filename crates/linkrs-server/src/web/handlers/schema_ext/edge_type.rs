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
use crate::value::from_json as json_to_core;
use crate::web::{
    error::{WebError, WebResult},
    models::{
        schema::{EdgeTypeDetail, PropertyDef, UpdateEdgeTypeRequest},
        ApiResponse,
    },
    WebState,
};
use linkrs_api::api_core::PropertyDef as CorePropertyDef;
use linkrs_wire::schema::{
    CreateEdgeTypeRequest, EdgeCreateResponse, EdgeDeleteResponse, EdgeListResponse, EdgeTypeInfo,
    EdgeUpdateResponse, FieldInfo,
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
        (status = 200, description = "Edge type list", body = ApiResponse<EdgeListResponse>),
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
) -> WebResult<Json<ApiResponse<EdgeListResponse>>> {
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

        let edge_type_list: Vec<EdgeTypeInfo> = edge_types
            .into_iter()
            .map(|e| EdgeTypeInfo {
                name: e.edge_type_name,
                fields: e
                    .properties
                    .iter()
                    .map(|p| FieldInfo {
                        name: p.name.clone(),
                        data_type: format!("{:?}", p.data_type),
                        nullable: p.nullable,
                        default_value: None,
                    })
                    .collect(),
                src_tag: Some(e.src_tag_name),
                dst_tag: Some(e.dst_tag_name),
            })
            .collect();

        Ok::<_, WebError>(EdgeListResponse {
            edge_types: edge_type_list,
            space_name,
        })
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
    request_body = CreateEdgeTypeRequest,
    responses(
        (status = 200, description = "Edge type created", body = ApiResponse<EdgeCreateResponse>),
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
    Json(request): Json<CreateEdgeTypeRequest>,
) -> WebResult<(StatusCode, Json<ApiResponse<EdgeCreateResponse>>)> {
    if request.name.trim().is_empty() {
        return Err(WebError::BadRequest(
            "Edge type name is required".to_string(),
        ));
    }
    let edge_name = request.name.clone();
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

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .create_edge_type(space_id, &edge_name, properties)
            .map_err(|e| WebError::Internal(format!("Failed to create edge type: {}", e)))?;

        Ok::<_, WebError>(EdgeCreateResponse {
            message: "Edge type created successfully".to_string(),
            edge_type_name: edge_name,
            space_name,
        })
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
        (status = 200, description = "Edge type updated", body = ApiResponse<EdgeUpdateResponse>),
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
) -> WebResult<Json<ApiResponse<EdgeUpdateResponse>>> {
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
            .alter_edge_type(space_id, &edge_name, additions, deletions)
            .map_err(|e| WebError::Internal(format!("Failed to update edge type: {}", e)))?;

        Ok::<_, WebError>(EdgeUpdateResponse {
            message: "Edge type updated successfully".to_string(),
            edge_type_name: edge_name,
            space_name,
        })
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
        (status = 200, description = "Edge type deleted", body = ApiResponse<EdgeDeleteResponse>),
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
) -> WebResult<(StatusCode, Json<ApiResponse<EdgeDeleteResponse>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_edge_type(space_id, &edge_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete edge type: {}", e)))?;

        Ok::<_, WebError>(EdgeDeleteResponse {
            message: "Edge type deleted successfully".to_string(),
            edge_type_name: edge_name,
            space_name,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

//! Public edge type management handlers.
//!
//! Handles edge type creation and listing within a space.

use axum::{
    extract::{Extension, Json, Path, State},
    response::Json as JsonResponse,
};
use linkrs_wire::schema::{
    CreateEdgeTypeRequest, EdgeCreateResponse, EdgeDeleteResponse, EdgeDetailResponse,
    EdgeListResponse, EdgeTypeInfo, FieldInfo,
};
use tokio::task;

use super::common::parse_data_type;
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::value::from_json as json_to_core;
use linkrs_api::api_core::PropertyDef as CorePropertyDef;

// ==================== Edge Type related ====================

#[utoipa::path(
    post,
    operation_id = "post_v1_schema_spaces_name_edge_types",
    path = "/v1/schema/spaces/{name}/edge-types",
    tag = "Schema",
    params(("name" = String, Path, description = "Space name")),
    request_body = CreateEdgeTypeRequest,
    responses(
        (status = 200, body = EdgeCreateResponse, description = "Edge type created"),
        (status = 500, description = "Internal error")
    )
)]
/// Creating Edge Types
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
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path(space_name): Path<String>,
    Json(request): Json<CreateEdgeTypeRequest>,
) -> Result<JsonResponse<EdgeCreateResponse>, HttpError> {
    super::super::authz::require_schema_write_session(&state, session_id, &space_name)?;
    let result = task::spawn_blocking(move || {
        let schema_api = state.server.get_schema_api();

        // Get Space ID
        let space_id = schema_api.use_space(&space_name)?;

        // Conversion Attribute Definition
        let properties: Vec<CorePropertyDef> = request
            .properties
            .into_iter()
            .map(|p| CorePropertyDef {
                name: p.name,
                data_type: parse_data_type(&p.data_type),
                nullable: p.nullable,
                default_value: p.default_value.map(|v| json_to_core(&v)),
                comment: p.comment,
            })
            .collect();

        schema_api.create_edge_type(space_id, &request.name, properties)?;

        Ok::<_, HttpError>(EdgeCreateResponse {
            message: "Edge type created successfully".to_string(),
            edge_type_name: request.name,
            space_name,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    operation_id = "get_v1_schema_spaces_name_edge_types",
    path = "/v1/schema/spaces/{name}/edge-types",
    tag = "Schema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, body = EdgeListResponse, description = "Edge type list"),
        (status = 500, description = "Internal error")
    )
)]
/// List all edge types
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
    State(state): State<AppState<S>>,
    Path(space_name): Path<String>,
) -> Result<JsonResponse<EdgeListResponse>, HttpError> {
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage = storage.read();
        let edge_types = storage
            .list_edge_types(&space_name)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let edge_list: Vec<EdgeTypeInfo> = edge_types
            .into_iter()
            .map(|edge| EdgeTypeInfo {
                name: edge.edge_type_name,
                fields: edge
                    .properties
                    .iter()
                    .map(|p| FieldInfo {
                        name: p.name.clone(),
                        data_type: format!("{:?}", p.data_type),
                        nullable: p.nullable,
                        default_value: None,
                    })
                    .collect(),
                src_tag: Some(edge.src_tag_name),
                dst_tag: Some(edge.dst_tag_name),
            })
            .collect();
        Ok::<_, HttpError>(EdgeListResponse {
            edge_types: edge_list,
            space_name,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    operation_id = "get_v1_schema_spaces_name_edge_types_edge",
    path = "/v1/schema/spaces/{name}/edge-types/{edge}",
    tag = "Schema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("edge" = String, Path, description = "Edge type name")
    ),
    responses(
        (status = 200, body = EdgeDetailResponse, description = "Edge type details"),
        (status = 404, description = "Edge type not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Get a single edge type by name
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
    State(state): State<AppState<S>>,
    Path((space_name, edge_name)): Path<(String, String)>,
) -> Result<JsonResponse<EdgeDetailResponse>, HttpError> {
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage = storage.read();
        let edge = storage
            .get_edge_type(&space_name, &edge_name)
            .map_err(|e| HttpError::InternalError(e.to_string()))?
            .ok_or_else(|| {
                HttpError::NotFound(format!(
                    "Edge type '{}' not found in space '{}'",
                    edge_name, space_name
                ))
            })?;
        Ok::<_, HttpError>(EdgeDetailResponse {
            name: edge.edge_type_name,
            space_name,
            src_tag: Some(edge.src_tag_name),
            dst_tag: Some(edge.dst_tag_name),
            fields: edge
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
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    delete,
    operation_id = "delete_v1_schema_spaces_name_edge_types_edge",
    path = "/v1/schema/spaces/{name}/edge-types/{edge}",
    tag = "Schema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("edge" = String, Path, description = "Edge type name")
    ),
    responses(
        (status = 200, body = EdgeDeleteResponse, description = "Edge type deleted"),
        (status = 404, description = "Edge type not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Drop a single edge type by name
pub async fn drop_edge_type<
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
    Extension(session_id): Extension<i64>,
    Path((space_name, edge_name)): Path<(String, String)>,
) -> Result<JsonResponse<EdgeDeleteResponse>, HttpError> {
    super::super::authz::require_schema_write_session(&state, session_id, &space_name)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let mut storage = storage.write();
        let dropped = storage
            .drop_edge_type(&space_name, &edge_name)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        if !dropped {
            return Err(HttpError::NotFound(format!(
                "Edge type '{}' not found in space '{}'",
                edge_name, space_name
            )));
        }
        Ok::<_, HttpError>(EdgeDeleteResponse {
            message: "Edge type deleted successfully".to_string(),
            edge_type_name: edge_name,
            space_name,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

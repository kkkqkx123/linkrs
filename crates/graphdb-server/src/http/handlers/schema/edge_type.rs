//! Public edge type management handlers.
//!
//! Handles edge type creation and listing within a space.

use axum::{
    extract::{Extension, Json, Path, State},
    response::Json as JsonResponse,
};
use graphdb_wire::schema::CreateEdgeTypeRequest;
use tokio::task;

use super::common::parse_data_type;
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::value::from_json as json_to_core;
use graphdb_api::api_core::PropertyDef as CorePropertyDef;

// ==================== Edge Type related ====================

#[utoipa::path(
    post,
    operation_id = "post_v1_schema_spaces_name_edge_types",
    path = "/v1/schema/spaces/{name}/edge-types",
    tag = "Schema",
    params(("name" = String, Path, description = "Space name")),
    request_body = CreateEdgeTypeRequest,
    responses(
        (status = 200, body = serde_json::Value, description = "Edge type created"),
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
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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

        Ok::<_, HttpError>(serde_json::json!({
            "message": "Edge type created successfully",
            "edge_type_name": request.name,
            "space_name": space_name,
        }))
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
        (status = 200, body = serde_json::Value, description = "Edge type list"),
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
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage = storage.read();
        let edge_types = storage
            .list_edge_types(&space_name)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let edge_list: Vec<serde_json::Value> = edge_types
            .into_iter()
            .map(|edge| {
                serde_json::json!({
                    "name": edge.edge_type_name,
                    "src_tag": edge.src_tag_name,
                    "dst_tag": edge.dst_tag_name,
                    "properties": edge.properties.iter().map(|p| {
                        serde_json::json!({
                            "name": p.name,
                            "data_type": format!("{:?}", p.data_type),
                            "nullable": p.nullable,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok::<_, HttpError>(serde_json::json!({
            "edge_types": edge_list,
            "space_name": space_name,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

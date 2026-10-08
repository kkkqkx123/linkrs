//! Public space management handlers.
//!
//! Handles space creation, lookup, deletion and listing.

use axum::{
    extract::{Extension, Json, Path, State},
    response::Json as JsonResponse,
};
use linkrs_wire::schema::CreateSpaceRequest;
use tokio::task;

use super::common::parse_data_type;
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_api::api_core::SpaceConfig;

// ==================== Space related ====================

#[utoipa::path(
    post,
    path = "/v1/schema/spaces",
    tag = "Schema",
    request_body = CreateSpaceRequest,
    responses(
        (status = 200, body = serde_json::Value, description = "Space created"),
        (status = 500, description = "Internal error")
    )
)]
/// Creating a graph space
pub async fn create_space<
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
    Json(request): Json<CreateSpaceRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let schema_api = state.server.get_schema_api();

        let config = SpaceConfig {
            vid_type: parse_data_type(&request.vid_type.unwrap_or_else(|| "STRING".to_string())),
            comment: request.comment,
            partition_num: 100,
            replica_factor: 1,
        };

        schema_api.create_space(&request.name, config)?;

        Ok::<_, HttpError>(serde_json::json!({
            "message": "Space created successfully",
            "space_name": request.name,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    path = "/v1/schema/spaces/{name}",
    tag = "Schema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, body = serde_json::Value, description = "Space details"),
        (status = 500, description = "Internal error")
    )
)]
/// Getting the graph space
pub async fn get_space<
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
    Path(name): Path<String>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let schema_api = state.server.get_schema_api();

        let space_id = schema_api.use_space(&name)?;

        Ok::<_, HttpError>(serde_json::json!({
            "space": {
                "name": name,
                "id": space_id,
            }
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    delete,
    path = "/v1/schema/spaces/{name}",
    tag = "Schema",
    params(("name" = String, Path, description = "Space name")),
    responses(
        (status = 200, body = serde_json::Value, description = "Space deleted"),
        (status = 500, description = "Internal error")
    )
)]
/// Deletion of map space
pub async fn drop_space<
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
    Path(name): Path<String>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let schema_api = state.server.get_schema_api();

        schema_api.drop_space(&name)?;

        Ok::<_, HttpError>(serde_json::json!({
            "message": "Space deleted successfully",
            "space_name": name,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    operation_id = "get_v1_schema_spaces",
    path = "/v1/schema/spaces",
    tag = "Schema",
    responses(
        (status = 200, body = serde_json::Value, description = "Space list"),
        (status = 500, description = "Internal error")
    )
)]
/// List all graph spaces
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
    State(state): State<AppState<S>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let schema_api = state.server.get_schema_api();

        let spaces = schema_api.list_spaces()?;

        let space_list: Vec<serde_json::Value> = spaces
            .into_iter()
            .map(|space| {
                serde_json::json!({
                    "id": space.space_id,
                    "name": space.space_name,
                    "vid_type": format!("{:?}", space.vid_type),
                    "comment": space.comment,
                })
            })
            .collect();

        Ok::<_, HttpError>(serde_json::json!({
            "spaces": space_list,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

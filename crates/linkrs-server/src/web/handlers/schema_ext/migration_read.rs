//! Read-only schema version and migration history for the web view.
//!
//! The main `/v1/schema/versions/*` and `/v1/migration/*` routes are the
//! admin write-capable surface. The web console needs the same history
//! for display, so this module exposes the read-only subset under
//! `/api/v1/schema/...` reusing the storage calls directly.

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use std::collections::HashMap;
use tokio::task;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{
    error::{WebError, WebResult},
    models::ApiResponse,
    WebState,
};

fn parse_is_edge(params: &HashMap<String, String>) -> Result<bool, WebError> {
    match params.get("is_edge") {
        None => Ok(false),
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(WebError::BadRequest(format!(
                "Invalid is_edge value: '{value}'. Expected 'true' or 'false'"
            ))),
        },
    }
}

/// Version history for a tag or edge type (web read-only mirror of
/// `GET /v1/schema/versions/{space}/{label}`).
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_space_versions",
    path = "/api/v1/schema/spaces/{name}/versions/{label}",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type")
    ),
    responses(
        (status = 200, description = "Version history", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_label_version_history<
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
    Path((space, label)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let is_edge = parse_is_edge(&params)?;
    let storage = web_state.core_state.server.get_storage();
    let result = task::spawn_blocking(move || {
        let storage_read = storage.read();
        let history = if is_edge {
            storage_read
                .get_edge_version_history(&space, &label)
                .map_err(|e| WebError::Storage(e.to_string()))?
        } else {
            storage_read
                .get_vertex_version_history(&space, &label)
                .map_err(|e| WebError::Storage(e.to_string()))?
        };
        let versions: Vec<serde_json::Value> = match history {
            None => Vec::new(),
            Some(handle) => handle
                .change_log
                .get_versions()
                .iter()
                .map(|version| {
                    let changes = handle
                        .change_log
                        .get_version_changes(*version)
                        .cloned()
                        .unwrap_or_default();
                    let timestamp_ms = changes.iter().map(|c| c.timestamp_ms).max().unwrap_or(0);
                    serde_json::json!({
                        "version": version,
                        "timestamp_ms": timestamp_ms,
                        "change_count": changes.len(),
                    })
                })
                .collect(),
        };
        Ok::<_, WebError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "versions": versions,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {e}")))?;
    Ok(Json(ApiResponse::success(result?)))
}

/// Migration history for a label (web read-only mirror of
/// `GET /v1/migration/history/{space}/{label}`).
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_space_migration_history",
    path = "/api/v1/schema/spaces/{name}/migrations/{label}/history",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name")
    ),
    responses(
        (status = 200, description = "Migration history", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_label_migration_history<
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
    Path((space, label)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let is_edge = parse_is_edge(&params)?;
    let storage = web_state.core_state.server.get_storage();
    let result = task::spawn_blocking(move || {
        let storage_read = storage.read();
        let history = storage_read
            .list_migration_history(&space, &label, is_edge)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let applied = storage_read
            .get_applied_versions(&space, &label, is_edge)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let history_json: Vec<serde_json::Value> = history
            .into_iter()
            .map(|record| serde_json::to_value(&record).unwrap_or(serde_json::Value::Null))
            .collect();
        Ok::<_, WebError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "applied_versions": applied,
            "history": history_json,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {e}")))?;
    Ok(Json(ApiResponse::success(result?)))
}

/// Migration status for a label (web read-only mirror of
/// `GET /v1/migration/status/{space}/{label}`).
#[utoipa::path(
    get,
    operation_id = "get_api_v1_schema_space_migration_status",
    path = "/api/v1/schema/spaces/{name}/migrations/{label}/status",
    tag = "WebSchema",
    params(
        ("name" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name")
    ),
    responses(
        (status = 200, description = "Migration status", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_label_migration_status<
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
    Path((space, label)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let is_edge = parse_is_edge(&params)?;
    let storage = web_state.core_state.server.get_storage();
    let result = task::spawn_blocking(move || {
        let storage_read = storage.read();
        let applied = storage_read
            .get_applied_versions(&space, &label, is_edge)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let history = storage_read
            .list_migration_history(&space, &label, is_edge)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let latest = applied.iter().max().copied().unwrap_or(0);
        Ok::<_, WebError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "latest_applied_version": latest,
            "applied_versions": applied,
            "history_count": history.len(),
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {e}")))?;
    Ok(Json(ApiResponse::success(result?)))
}

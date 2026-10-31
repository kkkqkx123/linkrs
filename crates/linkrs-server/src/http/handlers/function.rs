//! Custom function HTTP handler

use axum::{
    extract::{Extension, Json, Path, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;
use serde_json;

use crate::http::error::HttpError;
use crate::http::state::AppState;
use crate::services::function::{
    op_error_to_http, register_udf_from_source, unregister_udf_by_name,
};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[utoipa::path(
    post,
    path = "/v1/functions",
    tag = "Function",
    request_body = RegisterFunctionRequest,
    responses(
        (status = 200, body = serde_json::Value, description = "Function registered"),
        (status = 500, description = "Internal error")
    )
)]
/// Register a custom function
pub async fn register<
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
    Json(request): Json<RegisterFunctionRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let implementation = match &request.implementation {
        Some(serde_json::Value::String(path)) => path.clone(),
        Some(other) => {
            return Err(HttpError::bad_request(format!(
                "implementation must be a local UDF library path string, got {other}"
            )));
        }
        None => String::new(),
    };
    let registry = state.server.get_function_registry();
    let registered_name = register_udf_from_source(&registry, &request.name, &implementation)
        .map_err(op_error_to_http)?;
    let registry_guard = registry.read();
    let return_type = registry_guard
        .get_return_type(&registered_name)
        .map(|t| format!("{t:?}"))
        .unwrap_or_else(|| "unknown".to_string());

    Ok(JsonResponse(serde_json::json!({
        "function_id": registered_name,
        "name": registered_name,
        "function_type": "custom",
        "return_type": return_type,
        "status": "registered",
        "message": "Function registered successfully",
    })))
}

#[utoipa::path(
    get,
    path = "/v1/functions",
    tag = "Function",
    responses(
        (status = 200, body = serde_json::Value, description = "Function list"),
        (status = 500, description = "Internal error")
    )
)]
/// List all functions
pub async fn list<
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
    let registry = state.server.get_function_registry();
    let registry_guard = registry.read();

    let function_names = registry_guard.function_names();
    let functions: Vec<serde_json::Value> = function_names
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
            })
        })
        .collect();

    Ok(JsonResponse(serde_json::json!({
        "functions": functions,
        "total": functions.len(),
    })))
}

#[utoipa::path(
    get,
    path = "/v1/functions/{name}",
    tag = "Function",
    params(("name" = String, Path, description = "Function name")),
    responses(
        (status = 200, body = serde_json::Value, description = "Function details"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Obtain function details
pub async fn info<
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
    let registry = state.server.get_function_registry();
    let registry_guard = registry.read();

    if !registry_guard.contains(&name) {
        return Err(HttpError::not_found(format!(
            "Function '{}' does not exist",
            name
        )));
    }

    let is_builtin = registry_guard.get_builtin(&name).is_some();
    let is_custom = registry_guard.get_custom(&name).is_some();

    let function_type = if is_builtin {
        "builtin"
    } else if is_custom {
        "custom"
    } else {
        "unknown"
    };
    let return_type = registry_guard
        .get_return_type(&name)
        .map(|t| format!("{t:?}"))
        .unwrap_or_else(|| "unknown".to_string());
    // Dynamic UDFs record their library load time; builtins and in-process
    // customs have no registration event, so the field stays null.
    let registered_at = registry_guard
        .dynamic_udf_mtime(&name)
        .map(|mtime| chrono::DateTime::<chrono::Utc>::from(mtime).to_rfc3339());

    Ok(JsonResponse(serde_json::json!({
        "name": name,
        "type": function_type,
        "is_builtin": is_builtin,
        "is_custom": is_custom,
        "parameters": [],
        "return_type": return_type,
        "registered_at": registered_at,
    })))
}

#[utoipa::path(
    delete,
    path = "/v1/functions/{name}",
    tag = "Function",
    params(("name" = String, Path, description = "Function name")),
    responses(
        (status = 200, body = serde_json::Value, description = "Function unregistered"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Logout function
pub async fn unregister<
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
    super::authz::require_admin_session(&state, session_id)?;
    let registry = state.server.get_function_registry();
    unregister_udf_by_name(&registry, &name).map_err(op_error_to_http)?;

    Ok(JsonResponse(serde_json::json!({
        "message": "Function unregistered",
        "name": name,
    })))
}

/// Registration function request.
///
/// Only `name` and `implementation` (local dynamic-library path) decide
/// registration. The remaining metadata fields are accepted for client
/// compatibility but ignored; the loaded library is the source of truth.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RegisterFunctionRequest {
    pub name: String,
    #[serde(rename = "type", default)]
    pub function_type: String,
    #[serde(default)]
    pub parameters: Vec<String>,
    #[serde(rename = "return_type", default)]
    pub return_type: String,
    pub description: Option<String>,
    pub implementation: Option<serde_json::Value>,
}

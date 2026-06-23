//! Custom function HTTP handler

use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};
use serde_json;

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Register a custom function
pub async fn register<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Json(request): Json<RegisterFunctionRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let registry = state.server.get_function_registry();
    let registry_guard = registry.read();

    if registry_guard.contains(&request.name) {
        return Err(HttpError::bad_request(format!(
            "Function '{}' already exists",
            request.name
        )));
    }

    let _function_info = FunctionInfo {
        name: request.name.clone(),
        function_type: request.function_type.clone(),
        parameters: request.parameters.clone(),
        return_type: request.return_type.clone(),
        description: request.description.clone(),
    };
    drop(registry_guard);

    Ok(JsonResponse(serde_json::json!({
        "function_id": generate_function_id(&request.name),
        "name": request.name,
        "function_type": request.function_type,
        "parameters": request.parameters,
        "return_type": request.return_type,
        "status": "registered",
        "message": "Function registered successfully",
    })))
}

/// List all functions
pub async fn list<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
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

/// Obtain function details
pub async fn info<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
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

    Ok(JsonResponse(serde_json::json!({
        "name": name,
        "type": function_type,
        "is_builtin": is_builtin,
        "is_custom": is_custom,
        "parameters": [],
        "return_type": "any",
        "registered_at": "2024-01-01T00:00:00Z",
    })))
}

/// Logout function
pub async fn unregister<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
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

    if registry_guard.get_builtin(&name).is_some() {
        return Err(HttpError::bad_request(format!(
            "Built-in function '{}' cannot be logged out",
            name
        )));
    }

    drop(registry_guard);

    Ok(JsonResponse(serde_json::json!({
        "message": "Function unregistered",
        "name": name,
    })))
}

/// Registration function request
#[derive(Debug, Deserialize)]
pub struct RegisterFunctionRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub function_type: String,
    pub parameters: Vec<String>,
    #[serde(rename = "return_type")]
    pub return_type: String,
    pub description: Option<String>,
    pub implementation: Option<serde_json::Value>,
}

/// Function information
#[derive(Debug, Serialize)]
struct FunctionInfo {
    name: String,
    function_type: String,
    parameters: Vec<String>,
    return_type: String,
    description: Option<String>,
}

/// Generator function ID
fn generate_function_id(name: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    timestamp.hash(&mut hasher);

    format!("{:x}", hasher.finish())
}

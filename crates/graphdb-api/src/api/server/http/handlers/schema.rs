use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;
use tokio::task;

use crate::api::core::{PropertyDef, SpaceConfig};
use crate::api::server::http::{error::HttpError, state::AppState};
use crate::core::DataType;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

// ==================== Space related ====================

#[derive(Debug, Deserialize)]
pub struct CreateSpaceRequest {
    pub name: String,
    #[serde(default)]
    pub vid_type: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Creating a graph space
pub async fn create_space<
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
    Json(request): Json<CreateSpaceRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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

/// Getting the graph space
pub async fn get_space<
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

/// Deletion of map space
pub async fn drop_space<
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

/// List all graph spaces
pub async fn list_spaces<
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

// ==================== Tag related ====================

#[derive(Debug, Deserialize)]
pub struct CreateTagRequest {
    pub name: String,
    pub properties: Vec<PropertyDefInput>,
}

#[derive(Debug, Deserialize)]
pub struct PropertyDefInput {
    pub name: String,
    pub data_type: String,
    #[serde(default)]
    pub nullable: bool,
}

/// Creating Tags
pub async fn create_tag<
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
    Path(space_name): Path<String>,
    Json(request): Json<CreateTagRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let schema_api = state.server.get_schema_api();

        // Get Space ID
        let space_id = schema_api.use_space(&space_name)?;

        // Conversion Attribute Definition
        let properties: Vec<PropertyDef> = request
            .properties
            .into_iter()
            .map(|p| PropertyDef {
                name: p.name,
                data_type: parse_data_type(&p.data_type),
                nullable: p.nullable,
                default_value: None,
                comment: None,
            })
            .collect();

        schema_api.create_tag(space_id, &request.name, properties)?;

        Ok::<_, HttpError>(serde_json::json!({
            "message": "Tag created successfully",
            "tag_name": request.name,
            "space_name": space_name,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// List all tags
pub async fn list_tags<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    Path(space_name): Path<String>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    // Returns an empty list for now, since SchemaApi doesn't have a list_tags method.
    Ok(JsonResponse(serde_json::json!({
        "tags": [],
        "space_name": space_name,
        "note": "This feature is pending implementation",
    })))
}

// ==================== Edge Type related ====================

#[derive(Debug, Deserialize)]
pub struct CreateEdgeTypeRequest {
    pub name: String,
    pub properties: Vec<PropertyDefInput>,
}

/// Creating Edge Types
pub async fn create_edge_type<
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
    Path(space_name): Path<String>,
    Json(request): Json<CreateEdgeTypeRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let schema_api = state.server.get_schema_api();

        // Get Space ID
        let space_id = schema_api.use_space(&space_name)?;

        // Conversion Attribute Definition
        let properties: Vec<PropertyDef> = request
            .properties
            .into_iter()
            .map(|p| PropertyDef {
                name: p.name,
                data_type: parse_data_type(&p.data_type),
                nullable: p.nullable,
                default_value: None,
                comment: None,
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

/// List all edge types
pub async fn list_edge_types<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    Path(space_name): Path<String>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    // Returns an empty list for now, since SchemaApi doesn't have a list_edge_types method.
    Ok(JsonResponse(serde_json::json!({
        "edge_types": [],
        "space_name": space_name,
        "note": "This feature is pending implementation",
    })))
}

// ==================== Auxiliary Functions ====================

fn parse_data_type(type_str: &str) -> DataType {
    match type_str.to_uppercase().as_str() {
        "INT" | "INTEGER" => DataType::Int,
        "FLOAT" | "DOUBLE" => DataType::Float,
        "STRING" | "STR" => DataType::String,
        "BOOL" | "BOOLEAN" => DataType::Bool,
        _ => DataType::String, // String types are used by default
    }
}

// ==================== Schema Versioning ====================

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ChangeInfo {
    pub change_type: String,
    pub description: String,
    pub details: std::collections::HashMap<String, String>,
}

/// Parse is_edge query parameter, failing on invalid values
fn parse_is_edge_param(query: &std::collections::HashMap<String, String>) -> Result<bool, HttpError> {
    match query.get("is_edge") {
        None => Ok(false),
        Some(v) => match v.to_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(HttpError::BadRequest(format!(
                "Invalid is_edge value: '{}'. Expected 'true' or 'false'",
                v
            ))),
        },
    }
}

/// Get version history for a label (vertex tag or edge type)
pub async fn get_version_history<
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
    Path((space, label)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let is_edge = parse_is_edge_param(&query)?;

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();

        let history = if is_edge {
            storage_read.get_edge_version_history(&space, &label)
        } else {
            storage_read.get_vertex_version_history(&space, &label)
        }
        .map_err(|e| HttpError::InternalError(format!("Failed to get version history: {}", e)))?;

        let versions = history
            .map(|h| {
                h.change_log
                    .get_versions()
                    .iter()
                    .map(|&version| {
                        let changes: Vec<_> = h.change_log
                            .get_version_changes(version)
                            .map(|v| v.clone())
                            .unwrap_or_default()
                            .into_iter()
                            .map(|change| {
                                ChangeInfo {
                                    change_type: format!("{:?}", change.details),
                                    description: change.details.description(),
                                    details: {
                                        let mut d = std::collections::HashMap::new();
                                        d.insert("description".to_string(), change.details.description());
                                        d
                                    },
                                }
                            })
                            .collect();

                        serde_json::json!({
                            "version": version,
                            "timestamp_ms": 0,
                            "changes": changes,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok::<_, HttpError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "versions": versions,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// Get schema changes between two versions
pub async fn get_schema_changes<
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
    Path((space, label, from_version, to_version)): Path<(String, String, u64, u64)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let is_edge = parse_is_edge_param(&query)?;

    // Validate version range: from_version must be <= to_version
    if from_version > to_version {
        return Err(HttpError::BadRequest(
            format!(
                "Invalid version range: from_version ({}) must be <= to_version ({})",
                from_version, to_version
            )
        ));
    }

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();

        let changes = if is_edge {
            storage_read.get_edge_schema_changes(&space, &label, from_version, to_version)
        } else {
            storage_read.get_vertex_schema_changes(&space, &label, from_version, to_version)
        }
        .map_err(|e| HttpError::InternalError(format!("Failed to get schema changes: {}", e)))?;

        let change_list: Vec<_> = changes
            .iter()
            .map(|change| {
                let mut details_map = std::collections::HashMap::new();
                details_map.insert("description".to_string(), change.details.description());
                serde_json::json!({
                    "change_type": format!("{:?}", change.details),
                    "description": change.details.description(),
                    "details": details_map,
                })
            })
            .collect();

        Ok::<_, HttpError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "from_version": from_version,
            "to_version": to_version,
            "changes": change_list,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// Detect breaking changes between two versions
pub async fn detect_breaking_changes<
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
    Path((space, label, from_version, to_version)): Path<(String, String, u64, u64)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let is_edge = parse_is_edge_param(&query)?;

    // Validate version range: from_version must be <= to_version
    if from_version > to_version {
        return Err(HttpError::BadRequest(
            format!(
                "Invalid version range: from_version ({}) must be <= to_version ({})",
                from_version, to_version
            )
        ));
    }

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();

        let changes = if is_edge {
            storage_read.detect_edge_breaking_changes(&space, &label, from_version, to_version)
        } else {
            storage_read.detect_vertex_breaking_changes(&space, &label, from_version, to_version)
        }
        .map_err(|e| HttpError::InternalError(format!("Failed to detect breaking changes: {}", e)))?;

        let has_breaking = !changes.is_empty();
        let change_list: Vec<_> = changes
            .iter()
            .map(|change| {
                let mut details_map = std::collections::HashMap::new();
                details_map.insert("description".to_string(), change.details.description());
                serde_json::json!({
                    "change_type": format!("{:?}", change.details),
                    "description": change.details.description(),
                    "details": details_map,
                })
            })
            .collect();

        let recommendation = if has_breaking {
            format!(
                "Found {} breaking changes. Data migration may be required.",
                change_list.len()
            )
        } else {
            "No breaking changes detected".to_string()
        };

        Ok::<_, HttpError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "from_version": from_version,
            "to_version": to_version,
            "has_breaking_changes": has_breaking,
            "changes": change_list,
            "recommendation": recommendation,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}



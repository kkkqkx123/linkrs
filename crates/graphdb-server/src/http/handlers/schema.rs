use crate::value::from_json as json_to_core;
use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use graphdb_wire::schema::{CreateEdgeTypeRequest, CreateSpaceRequest, CreateTagRequest};
use tokio::task;

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use graphdb_api::api_core::{PropertyDef as CorePropertyDef, SpaceConfig};
use graphdb_core::DataType;
use graphdb_migration::{generate_edge_plan_with_expand, generate_vertex_plan_with_expand};

// ==================== Space related ====================

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

// ==================== Tag related ====================

/// Creating Tags
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
    State(state): State<AppState<S>>,
    Path(space_name): Path<String>,
    Json(request): Json<CreateTagRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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
        + StorageOperationContextOps
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
    Path(space_name): Path<String>,
    Json(request): Json<CreateEdgeTypeRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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
    // The wire `data_type` is the core `DataType` Display output; parse it
    // back through the same `FromStr` source of truth. Unrecognized types
    // fall back to String (previous behavior).
    type_str.parse::<DataType>().unwrap_or(DataType::String)
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
fn parse_is_edge_param(
    query: &std::collections::HashMap<String, String>,
) -> Result<bool, HttpError> {
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
        + StorageOperationContextOps
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
                        let changes: Vec<_> = h
                            .change_log
                            .get_version_changes(version)
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|change| ChangeInfo {
                                change_type: format!("{:?}", change.details),
                                description: change.details.description(),
                                details: {
                                    let mut d = std::collections::HashMap::new();
                                    d.insert(
                                        "description".to_string(),
                                        change.details.description(),
                                    );
                                    d
                                },
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
        + StorageOperationContextOps
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
        return Err(HttpError::BadRequest(format!(
            "Invalid version range: from_version ({}) must be <= to_version ({})",
            from_version, to_version
        )));
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
        + StorageOperationContextOps
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
        return Err(HttpError::BadRequest(format!(
            "Invalid version range: from_version ({}) must be <= to_version ({})",
            from_version, to_version
        )));
    }

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();

        let changes = if is_edge {
            storage_read.detect_edge_breaking_changes(&space, &label, from_version, to_version)
        } else {
            storage_read.detect_vertex_breaking_changes(&space, &label, from_version, to_version)
        }
        .map_err(|e| {
            HttpError::InternalError(format!("Failed to detect breaking changes: {}", e))
        })?;

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

// ==================== Migration ====================

#[derive(serde::Deserialize)]
pub struct MigrationPlanQuery {
    pub from_version: Option<u64>,
    pub to_version: Option<u64>,
    pub is_edge: Option<bool>,
    pub expand_contract: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct MigrationExecuteRequest {
    pub plan_json: String,
}

#[derive(serde::Deserialize)]
pub struct MigrationRollbackRequest {
    pub plan_json: String,
}

pub async fn create_migration_plan<
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
    Path((space, label)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<MigrationPlanQuery>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let from_version = query
        .from_version
        .ok_or_else(|| HttpError::BadRequest("from_version required".into()))?;
    let to_version = query
        .to_version
        .ok_or_else(|| HttpError::BadRequest("to_version required".into()))?;
    let is_edge = query.is_edge.unwrap_or(false);
    let expand_contract = query.expand_contract.unwrap_or(false);

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();
        let plan = if is_edge {
            if expand_contract {
                generate_edge_plan_with_expand(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                    true,
                )
            } else {
                graphdb_migration::generate_edge_plan(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                )
            }
        } else {
            if expand_contract {
                generate_vertex_plan_with_expand(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                    true,
                )
            } else {
                graphdb_migration::generate_vertex_plan(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                )
            }
        }
        .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok::<_, HttpError>(serde_json::json!({
            "plan": plan,
            "plan_json": serde_json::to_string(&plan).unwrap(),
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

pub async fn execute_migration<
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
    Json(req): Json<MigrationExecuteRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let plan: graphdb_migration::MigrationPlan =
        serde_json::from_str(&req.plan_json).map_err(|e| HttpError::BadRequest(e.to_string()))?;

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let stats = state.server.get_stats_manager();
        let start = std::time::Instant::now();
        stats.record_migration_start();
        let mut storage_write = storage.write();
        let listener = crate::http::handlers::migration_progress::BroadcastEventListener::new(
            &plan.target.space,
            &plan.target.label,
            plan.target.is_edge,
        );
        let report = graphdb_migration::execute_migration_plan_with_progress(
            &mut *storage_write,
            &plan,
            &graphdb_migration::NoopProgress,
            Some(&listener),
        )
        .map_err(|e| HttpError::InternalError(e.to_string()));
        let elapsed = start.elapsed().as_millis() as u64;
        match &report {
            Ok(r) if r.success => stats.record_migration_success(r.rows_migrated, elapsed),
            Ok(_) => stats.record_migration_failure(elapsed),
            Err(_) => stats.record_migration_failure(elapsed),
        }
        let report = report?;
        Ok::<_, HttpError>(serde_json::json!({
            "success": report.success,
            "steps_completed": report.steps_completed,
            "rows_migrated": report.rows_migrated,
            "errors": report.errors,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

pub async fn rollback_migration<
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
    Json(req): Json<MigrationRollbackRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let plan: graphdb_migration::MigrationPlan =
        serde_json::from_str(&req.plan_json).map_err(|e| HttpError::BadRequest(e.to_string()))?;

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let mut storage_write = storage.write();
        let report = graphdb_migration::rollback_migration(&mut *storage_write, &plan)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok::<_, HttpError>(serde_json::json!({
            "success": report.success,
            "steps_completed": report.steps_completed,
            "rows_migrated": report.rows_migrated,
            "errors": report.errors,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

pub async fn dry_run_migration<
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
    Json(req): Json<MigrationExecuteRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let mut plan: graphdb_migration::MigrationPlan =
        serde_json::from_str(&req.plan_json).map_err(|e| HttpError::BadRequest(e.to_string()))?;
    plan.dry_run = true;

    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let mut storage_write = storage.write();
        let report = graphdb_migration::execute_migration_plan(&mut *storage_write, &plan)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok::<_, HttpError>(serde_json::json!({
            "success": report.success,
            "steps_completed": report.steps_completed,
            "rows_migrated": report.rows_migrated,
            "errors": report.errors,
            "preview": true,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

pub async fn migration_history<
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
    Path((space, label)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let is_edge = parse_is_edge_param(&query)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();
        let history = storage_read
            .list_migration_history(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let versions = storage_read
            .get_applied_versions(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        Ok::<_, HttpError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "applied_versions": versions,
            "history": history,
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

pub async fn migration_status<
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
    Path((space, label)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let is_edge = parse_is_edge_param(&query)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();
        let applied = storage_read
            .get_applied_versions(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let history = storage_read
            .list_migration_history(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let latest = applied.iter().max().copied().unwrap_or(0);
        Ok::<_, HttpError>(serde_json::json!({
            "space": space,
            "label": label,
            "is_edge": is_edge,
            "latest_applied_version": latest,
            "applied_versions": applied,
            "history_count": history.len(),
        }))
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

//! Schema version inspection handlers.
//!
//! Covers version history, change ranges and breaking change detection.

use axum::{
    extract::{Path, State},
    response::Json as JsonResponse,
};
use tokio::task;

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_wire::schema::{
    BreakingChangesResponse, SchemaChangeEntry, SchemaChangesResponse, SchemaVersionEntry,
    VersionHistoryResponse,
};

// ==================== Schema Versioning ====================

/// Parse is_edge query parameter, failing on invalid values
pub(crate) fn parse_is_edge_param(
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

fn to_change_entry(change_type: String, description: String) -> SchemaChangeEntry {
    let mut details = std::collections::HashMap::new();
    details.insert("description".to_string(), description.clone());
    SchemaChangeEntry {
        change_type,
        description,
        details,
    }
}

#[utoipa::path(
    get,
    path = "/v1/schema/versions/{space}/{label}",
    tag = "Schema",
    params(
        ("space" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type")
    ),
    responses(
        (status = 200, body = VersionHistoryResponse, description = "Schema version history"),
        (status = 500, description = "Internal error")
    )
)]
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
) -> Result<JsonResponse<VersionHistoryResponse>, HttpError> {
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
                        let version_changes = h
                            .change_log
                            .get_version_changes(version)
                            .cloned()
                            .unwrap_or_default();
                        let timestamp_ms = version_changes
                            .iter()
                            .map(|c| c.timestamp_ms)
                            .max()
                            .unwrap_or(0);
                        let changes: Vec<SchemaChangeEntry> = version_changes
                            .into_iter()
                            .map(|change| {
                                let description = change.details.description();
                                to_change_entry(format!("{:?}", change.details), description)
                            })
                            .collect();

                        SchemaVersionEntry {
                            version,
                            timestamp_ms,
                            changes,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok::<_, HttpError>(VersionHistoryResponse {
            space,
            label,
            is_edge,
            versions,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    path = "/v1/schema/changes/{space}/{label}/{from_version}/{to_version}",
    tag = "Schema",
    params(
        ("space" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name"),
        ("from_version" = u64, Path, description = "Start schema version"),
        ("to_version" = u64, Path, description = "End schema version"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type")
    ),
    responses(
        (status = 200, body = SchemaChangesResponse, description = "Schema changes between versions"),
        (status = 500, description = "Internal error")
    )
)]
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
) -> Result<JsonResponse<SchemaChangesResponse>, HttpError> {
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

        let change_list: Vec<SchemaChangeEntry> = changes
            .iter()
            .map(|change| {
                let description = change.details.description();
                to_change_entry(format!("{:?}", change.details), description)
            })
            .collect();

        Ok::<_, HttpError>(SchemaChangesResponse {
            space,
            label,
            is_edge,
            from_version,
            to_version,
            changes: change_list,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    path = "/v1/schema/breaking-changes/{space}/{label}/{from_version}/{to_version}",
    tag = "Schema",
    params(
        ("space" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Label or edge type name"),
        ("from_version" = u64, Path, description = "Start schema version"),
        ("to_version" = u64, Path, description = "End schema version"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type")
    ),
    responses(
        (status = 200, body = BreakingChangesResponse, description = "Breaking change report"),
        (status = 500, description = "Internal error")
    )
)]
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
) -> Result<JsonResponse<BreakingChangesResponse>, HttpError> {
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
        let change_list: Vec<SchemaChangeEntry> = changes
            .iter()
            .map(|change| {
                let description = change.details.description();
                to_change_entry(format!("{:?}", change.details), description)
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

        Ok::<_, HttpError>(BreakingChangesResponse {
            space,
            label,
            is_edge,
            from_version,
            to_version,
            has_breaking_changes: has_breaking,
            changes: change_list,
            recommendation,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

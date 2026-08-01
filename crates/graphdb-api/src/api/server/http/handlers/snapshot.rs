//! Cold snapshot management endpoints (list / load / remove / export / merge).

use axum::{
    extract::{Path, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;
use serde_json;

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    ColdSnapshotInfo, StorageClient, StorageOperationContextOps, StorageSchemaContextOps,
    StorageSnapshotOps, StorageSyncContextOps,
};

fn to_json_info(info: &ColdSnapshotInfo) -> serde_json::Value {
    serde_json::json!({
        "label": info.label,
        "label_name": info.label_name,
        "snapshot_ts": info.snapshot_ts,
        "edge_count": info.edge_count,
        "file_path": info.file_path,
        "file_size": info.file_size,
        "checksum": info.checksum,
    })
}

/// List all registered cold snapshots.
pub async fn list_cold_snapshots<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let storage = state.server.get_storage();
    let infos = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || storage.read().list_cold_snapshots())
            .await
            .map_err(|e| HttpError::internal(format!("Failed to list snapshots: {:?}", e)))?
            .map_err(|e| HttpError::internal(format!("Failed to list snapshots: {}", e)))?
    };
    let items: Vec<serde_json::Value> = infos.iter().map(to_json_info).collect();
    Ok(JsonResponse(serde_json::json!({ "snapshots": items })))
}

#[derive(Deserialize)]
pub struct LoadSnapshotRequest {
    pub path: String,
}

/// Register a cold snapshot from a `.lkcs` file path on the server.
pub async fn load_cold_snapshot<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    axum::extract::Json(request): axum::extract::Json<LoadSnapshotRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let storage = state.server.get_storage();
    let path = std::path::PathBuf::from(request.path);
    let info = {
        let storage = storage.clone();
        let path = path.clone();
        tokio::task::spawn_blocking(move || storage.read().load_cold_snapshot(&path))
            .await
            .map_err(|e| HttpError::internal(format!("Failed to load snapshot: {:?}", e)))?
            .map_err(|e| HttpError::internal(format!("Failed to load snapshot: {}", e)))?
    };
    Ok(JsonResponse(to_json_info(&info)))
}

/// Drop all cold snapshots of an edge label from the registry.
pub async fn remove_cold_snapshot<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path(label): Path<u32>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let storage = state.server.get_storage();
    {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || storage.read().remove_cold_snapshot(label))
            .await
            .map_err(|e| HttpError::internal(format!("Failed to remove snapshot: {:?}", e)))?
            .map_err(|e| HttpError::internal(format!("Failed to remove snapshot: {}", e)))?
    }
    Ok(JsonResponse(serde_json::json!({ "removed": true, "label": label })))
}

#[derive(Deserialize)]
pub struct ExportSnapshotRequest {
    pub label: u32,
    pub path: String,
}

/// Re-export the most recent cold snapshot of a label to a path.
pub async fn export_cold_snapshot<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    axum::extract::Json(request): axum::extract::Json<ExportSnapshotRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let storage = state.server.get_storage();
    let path = std::path::PathBuf::from(request.path);
    let info = {
        let storage = storage.clone();
        let path = path.clone();
        tokio::task::spawn_blocking(move || storage.read().export_cold_snapshot(request.label, &path))
            .await
            .map_err(|e| HttpError::internal(format!("Failed to export snapshot: {:?}", e)))?
            .map_err(|e| HttpError::internal(format!("Failed to export snapshot: {}", e)))?
    };
    Ok(JsonResponse(to_json_info(&info)))
}

#[derive(Deserialize)]
pub struct MergeSnapshotsRequest {
    pub labels: Vec<u32>,
}

/// Consolidate every registered version of the given labels into a single
/// snapshot per label.
pub async fn merge_cold_snapshots<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    axum::extract::Json(request): axum::extract::Json<MergeSnapshotsRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let storage = state.server.get_storage();
    let infos = {
        let storage = storage.clone();
        let labels = request.labels.clone();
        tokio::task::spawn_blocking(move || storage.read().merge_cold_snapshots(&labels))
            .await
            .map_err(|e| HttpError::internal(format!("Failed to merge snapshots: {:?}", e)))?
            .map_err(|e| HttpError::internal(format!("Failed to merge snapshots: {}", e)))?
    };
    let items: Vec<serde_json::Value> = infos.iter().map(to_json_info).collect();
    Ok(JsonResponse(serde_json::json!({ "merged": items })))
}

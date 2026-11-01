//! Background freeze control handlers.
//!
//! Exposes freeze statistics and manual freeze triggers.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSnapshotOps,
    StorageSyncContextOps,
};

#[utoipa::path(
    get,
    path = "/v1/statistics/freeze",
    tag = "Statistics",
    responses(
        (status = 200, body = linkrs_wire::meta::FreezeStatsResponse, description = "Background freeze statistics"),
        (status = 500, description = "Internal error")
    )
)]
/// Get background freeze statistics
pub async fn freeze_stats<
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
) -> Result<JsonResponse<linkrs_wire::meta::FreezeStatsResponse>, HttpError> {
    let storage = state.server.get_storage();

    let freeze_stats = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            let storage = storage.read();
            storage.get_freeze_stats()
        })
        .await
        .map_err(|e| HttpError::internal(format!("Failed to get freeze stats: {:?}", e)))?
    };

    match freeze_stats {
        Some(stats) => Ok(JsonResponse(linkrs_wire::meta::FreezeStatsResponse {
            enabled: true,
            freeze_count: stats.freeze_count,
            total_frozen_edges: stats.total_frozen_edges,
            last_freeze_duration_ms: stats.last_freeze_duration_ms,
            current_delta_edges: stats.current_delta_edges,
            message: None,
        })),
        None => Ok(JsonResponse(linkrs_wire::meta::FreezeStatsResponse {
            enabled: false,
            freeze_count: 0,
            total_frozen_edges: 0,
            last_freeze_duration_ms: 0,
            current_delta_edges: 0,
            message: Some("Background freeze manager not configured".to_string()),
        })),
    }
}

#[utoipa::path(
    post,
    path = "/v1/statistics/freeze",
    tag = "Statistics",
    responses(
        (status = 200, body = linkrs_wire::meta::FreezeTriggerResponse, description = "Background freeze triggered"),
        (status = 500, description = "Internal error")
    )
)]
/// Trigger background freeze manually
pub async fn trigger_freeze<
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
) -> Result<JsonResponse<linkrs_wire::meta::FreezeTriggerResponse>, HttpError> {
    let storage = state.server.get_storage();

    let result = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            let storage = storage.read();
            storage.trigger_background_freeze()
        })
        .await
        .map_err(|e| HttpError::internal(format!("Failed to trigger freeze: {:?}", e)))?
    };

    match result {
        Ok(()) => Ok(JsonResponse(linkrs_wire::meta::FreezeTriggerResponse {
            status: "ok".to_string(),
            message: "Background freeze triggered successfully".to_string(),
        })),
        Err(e) => Err(HttpError::internal(format!(
            "Background freeze failed: {}",
            e
        ))),
    }
}

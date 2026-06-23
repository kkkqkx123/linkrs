use axum::{extract::State, response::Json as JsonResponse};
use serde::Serialize;

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Sync status response
#[derive(Debug, Serialize)]
pub struct SyncStatusResponse {
    pub is_running: bool,
    pub dlq_size: usize,
    pub unrecovered_dlq_size: usize,
}

/// Get sync status
pub async fn status<
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
) -> Result<JsonResponse<SyncStatusResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service.sync_api();

    if let Some(sync_api) = sync_api {
        Ok(JsonResponse(SyncStatusResponse {
            is_running: sync_api.is_running(),
            dlq_size: sync_api.get_dlq_size(),
            unrecovered_dlq_size: sync_api.get_unrecovered_dlq_size(),
        }))
    } else {
        // Sync manager not available, return disabled status
        Ok(JsonResponse(SyncStatusResponse {
            is_running: false,
            dlq_size: 0,
            unrecovered_dlq_size: 0,
        }))
    }
}

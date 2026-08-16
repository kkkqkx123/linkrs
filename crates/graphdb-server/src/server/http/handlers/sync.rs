use axum::{extract::State, response::Json as JsonResponse};
use serde::Serialize;

use crate::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

/// Sync status response
#[derive(Debug, Serialize)]
pub struct SyncStatusResponse {
    pub is_running: bool,
    pub dlq_size: usize,
    pub unrecovered_dlq_size: usize,
    pub outbox_pending: usize,
    pub outbox_retries: u64,
    pub outbox_dead_lettered: usize,
    pub outbox_leased: usize,
    pub outbox_oldest_event_age_ms: u64,
    pub outbox_write_amplification_bytes: u64,
    pub outbox_lock_wait_nanos: u64,
    pub outbox_persist_operations: u64,
}

/// Get sync status
pub async fn status<
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
) -> Result<JsonResponse<SyncStatusResponse>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service.sync_api();

    if let Some(sync_api) = sync_api {
        let outbox = sync_api.outbox_stats();
        Ok(JsonResponse(SyncStatusResponse {
            is_running: sync_api.is_running(),
            dlq_size: sync_api.get_dlq_size(),
            unrecovered_dlq_size: sync_api.get_unrecovered_dlq_size(),
            outbox_pending: outbox.pending,
            outbox_retries: outbox.retries,
            outbox_dead_lettered: outbox.dead_lettered,
            outbox_leased: outbox.leased,
            outbox_oldest_event_age_ms: outbox.oldest_event_age_ms,
            outbox_write_amplification_bytes: outbox.write_amplification_bytes,
            outbox_lock_wait_nanos: outbox.lock_wait_nanos,
            outbox_persist_operations: outbox.persist_operations,
        }))
    } else {
        // Sync manager not available, return disabled status
        Ok(JsonResponse(SyncStatusResponse {
            is_running: false,
            dlq_size: 0,
            unrecovered_dlq_size: 0,
            outbox_pending: 0,
            outbox_retries: 0,
            outbox_dead_lettered: 0,
            outbox_leased: 0,
            outbox_oldest_event_age_ms: 0,
            outbox_write_amplification_bytes: 0,
            outbox_lock_wait_nanos: 0,
            outbox_persist_operations: 0,
        }))
    }
}

/// Retry delivery of pending durable outbox entries.
pub async fn retry_outbox<
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
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service
        .sync_api()
        .ok_or_else(|| HttpError::bad_request("Synchronization is not configured"))?;
    let delivered = sync_api
        .retry_outbox_projection()
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!({
        "delivered": delivered,
        "status": "completed",
    })))
}

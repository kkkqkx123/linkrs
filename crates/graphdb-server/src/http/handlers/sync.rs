use axum::{
    extract::{Query, State},
    Json,
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};

use crate::http::{error::HttpError, state::AppState};
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

/// Diagnostics query for outbox frontiers and degraded state.
pub async fn diagnostics<
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
    let diag = sync_api
        .sync_diagnostics()
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!(diag)))
}

#[derive(Debug, Deserialize)]
pub struct DeadLetterQuery {
    pub target: Option<String>,
    pub index_id: Option<u64>,
    pub generation: Option<u64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn dead_letters<
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
    Query(params): Query<DeadLetterQuery>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service
        .sync_api()
        .ok_or_else(|| HttpError::bad_request("Synchronization is not configured"))?;
    let target = params
        .target
        .as_deref()
        .map(|s| graphdb_core::types::TargetId::new(s.to_string()).map_err(HttpError::bad_request))
        .transpose()?;
    let rows = sync_api
        .list_dead_letters(
            target.as_ref(),
            params.index_id,
            params.generation,
            params.limit.unwrap_or(100),
            params.offset.unwrap_or(0),
        )
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!({ "dead_letters": rows })))
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RequeueRequest {
    pub target: Option<String>,
    pub index_id: Option<u64>,
    pub generation: Option<u64>,
    pub limit: Option<usize>,
    pub event_ids: Option<Vec<i64>>,
}

pub async fn requeue<
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
    Json(payload): Json<RequeueRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service
        .sync_api()
        .ok_or_else(|| HttpError::bad_request("Synchronization is not configured"))?;
    if let Some(ids) = payload.event_ids {
        let mut requeued = 0usize;
        for id in ids {
            if sync_api
                .requeue_dead_letter(id)
                .map_err(HttpError::transaction_message)?
            {
                requeued += 1;
            }
        }
        return Ok(JsonResponse(serde_json::json!({ "requeued": requeued })));
    }
    let target = payload
        .target
        .as_deref()
        .map(|s| graphdb_core::types::TargetId::new(s.to_string()).map_err(HttpError::bad_request))
        .transpose()?;
    let requeued = sync_api
        .requeue_dead_letters_batch(
            target.as_ref(),
            payload.index_id,
            payload.generation,
            payload.limit.unwrap_or(100),
        )
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!({ "requeued": requeued })))
}

#[derive(Debug, Deserialize)]
pub struct DegradedQuery {
    pub target: Option<String>,
    pub index_id: Option<u64>,
    pub generation: Option<u64>,
}

pub async fn degraded_ranges<
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
    Query(params): Query<DegradedQuery>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service
        .sync_api()
        .ok_or_else(|| HttpError::bad_request("Synchronization is not configured"))?;
    let target = params
        .target
        .as_deref()
        .map(|s| graphdb_core::types::TargetId::new(s.to_string()).map_err(HttpError::bad_request))
        .transpose()?;
    let rows = sync_api
        .list_degraded_ranges(target.as_ref(), params.index_id, params.generation)
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!({ "degraded_ranges": rows })))
}

#[derive(Debug, Deserialize)]
pub struct ClearDegradedRequest {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub start_lsn: u64,
    pub end_lsn: u64,
}

pub async fn degraded_clear<
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
    Json(payload): Json<ClearDegradedRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service
        .sync_api()
        .ok_or_else(|| HttpError::bad_request("Synchronization is not configured"))?;
    let target =
        graphdb_core::types::TargetId::new(payload.target).map_err(HttpError::bad_request)?;
    let cleared = sync_api
        .clear_degraded_range(
            &target,
            payload.index_id,
            payload.generation,
            graphdb_core::types::CommitLsn::new(payload.start_lsn),
            graphdb_core::types::CommitLsn::new(payload.end_lsn),
        )
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!({ "cleared": cleared })))
}

#[derive(Debug, Deserialize)]
pub struct RetentionRunRequest {
    pub grace_lsn_distance: Option<u64>,
    pub max_age_ms: Option<u64>,
}

pub async fn retention_run<
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
    Json(payload): Json<RetentionRunRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let graph_service = state.server.get_graph_service();
    let sync_api = graph_service
        .sync_api()
        .ok_or_else(|| HttpError::bad_request("Synchronization is not configured"))?;
    let grace = payload.grace_lsn_distance.unwrap_or(10_000);
    let max_age = payload.max_age_ms.unwrap_or(86_400_000 * 30);
    let (pruned, archived, retention_lsn) = sync_api
        .run_retention_once(grace, max_age)
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!({
        "pruned": pruned,
        "archived": archived,
        "retention_lsn": retention_lsn,
    })))
}

pub async fn retention_status<
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
    let retention = sync_api
        .retention_lsn()
        .map_err(HttpError::transaction_message)?;
    Ok(JsonResponse(serde_json::json!({ "retention_lsn": retention.get() })))
}

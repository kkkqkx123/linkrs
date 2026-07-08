//! Batch processing of HTTP handlers

use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde_json;

use crate::api::server::batch::{
    AddBatchItemsRequest, AddBatchItemsResponse, BatchStatusResponse, CreateBatchRequest,
    CreateBatchResponse, ExecuteBatchResponse,
};
use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Create batch tasks
pub async fn create<
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
    Json(request): Json<CreateBatchRequest>,
) -> Result<JsonResponse<CreateBatchResponse>, HttpError> {
    let batch_manager = state.server.get_batch_manager();

    match batch_manager.create_task(request.space_id, request.batch_type, request.batch_size) {
        Ok(task) => Ok(JsonResponse(CreateBatchResponse {
            batch_id: task.id,
            status: task.status,
            created_at: task.created_at.to_rfc3339(),
        })),
        Err(e) => Err(HttpError::InternalError(format!(
            "Failed to create batch task: {}",
            e
        ))),
    }
}

/// Obtaining the status of batch tasks
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
    Path(batch_id): Path<String>,
) -> Result<JsonResponse<BatchStatusResponse>, HttpError> {
    let batch_manager = state.server.get_batch_manager();

    match batch_manager.get_task(&batch_id) {
        Some(task) => Ok(JsonResponse(BatchStatusResponse {
            batch_id: task.id,
            status: task.status,
            progress: task.progress,
            created_at: task.created_at.to_rfc3339(),
            updated_at: task.updated_at.to_rfc3339(),
        })),
        None => Err(HttpError::NotFound(format!(
            "Batch task does not exist: {}",
            batch_id
        ))),
    }
}

/// Add multiple items in batches
pub async fn add_items<
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
    Path(batch_id): Path<String>,
    Json(request): Json<AddBatchItemsRequest>,
) -> Result<JsonResponse<AddBatchItemsResponse>, HttpError> {
    let batch_manager = state.server.get_batch_manager();

    match batch_manager.add_items(&batch_id, request.items) {
        Ok(accepted) => {
            let task = batch_manager.get_task(&batch_id).ok_or_else(|| {
                HttpError::NotFound(format!("Batch task does not exist: {}", batch_id))
            })?;

            Ok(JsonResponse(AddBatchItemsResponse {
                accepted,
                buffered: accepted,
                total_buffered: task.progress.buffered,
            }))
        }
        Err(e) => Err(HttpError::BadRequest(format!(
            "Adding batch items failed: {}",
            e
        ))),
    }
}

/// Perform batch tasks
pub async fn execute<
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
    Path(batch_id): Path<String>,
) -> Result<JsonResponse<ExecuteBatchResponse>, HttpError> {
    let batch_manager = state.server.get_batch_manager();

    // Retrieve task information in order to obtain the space_id.
    let task = batch_manager.get_task(&batch_id).ok_or_else(|| {
        HttpError::NotFound(format!("The batch task does not exist: {}", batch_id))
    })?;

    // Query the `space_name` using the `space_id`.
    let space_name = {
        let storage = state.server.get_storage();
        let storage = storage.write();
        match storage.get_space_by_id(task.space_id) {
            Ok(Some(space_info)) => space_info.space_name,
            Ok(None) => {
                return Err(HttpError::NotFound(format!(
                    "The graph space does not exist: {}",
                    task.space_id
                )))
            }
            Err(e) => {
                return Err(HttpError::InternalError(format!(
                    "Querying the graph space failed: {}",
                    e
                )))
            }
        }
    };

    match batch_manager.execute_task(&batch_id, &space_name).await {
        Ok(result) => {
            let task = batch_manager.get_task(&batch_id).ok_or_else(|| {
                HttpError::NotFound(format!("Batch task does not exist: {}", batch_id))
            })?;

            Ok(JsonResponse(ExecuteBatchResponse {
                batch_id: batch_id.clone(),
                status: task.status,
                result,
                completed_at: Some(task.updated_at.to_rfc3339()),
            }))
        }
        Err(e) => Err(HttpError::InternalError(format!(
            "Failed to execute batch tasks: {}",
            e
        ))),
    }
}

/// Cancel the batch task.
pub async fn cancel<
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
    Path(batch_id): Path<String>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let batch_manager = state.server.get_batch_manager();

    match batch_manager.cancel_task(&batch_id) {
        Ok(()) => Ok(JsonResponse(serde_json::json!({
            "message": "Batch task cancelled",
            "batch_id": batch_id,
        }))),
        Err(e) => Err(HttpError::BadRequest(format!(
            "Failed to cancel the batch task: {}",
            e
        ))),
    }
}

/// Delete batch tasks.
pub async fn delete<
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
    Path(batch_id): Path<String>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let batch_manager = state.server.get_batch_manager();

    match batch_manager.remove_task(&batch_id) {
        Ok(()) => Ok(JsonResponse(serde_json::json!({
            "message": "Batch task deleted",
            "batch_id": batch_id,
        }))),
        Err(e) => Err(HttpError::NotFound(format!(
            "Failed to delete batch tasks: {}",
            e
        ))),
    }
}

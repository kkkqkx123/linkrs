use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};
use tokio::task;

use crate::api::core::{SavepointId, TransactionHandle};
use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};
use crate::transaction::{DurabilityLevel, IsolationLevel, TransactionOptions};

#[derive(Debug, Deserialize)]
pub struct BeginTransactionRequest {
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub query_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub statement_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub idle_timeout_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct TransactionResponse {
    pub transaction_id: u64,
    pub status: String,
}

/// Start a transaction
pub async fn begin<
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
    Json(request): Json<BeginTransactionRequest>,
) -> Result<JsonResponse<TransactionResponse>, HttpError> {
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();

        let options = TransactionOptions {
            read_only: request.read_only,
            timeout: request.timeout_seconds.map(std::time::Duration::from_secs),
            durability: DurabilityLevel::Sync,
            isolation_level: IsolationLevel::default(),
            query_timeout: request
                .query_timeout_seconds
                .map(std::time::Duration::from_secs),
            statement_timeout: request
                .statement_timeout_seconds
                .map(std::time::Duration::from_secs),
            idle_timeout: request
                .idle_timeout_seconds
                .map(std::time::Duration::from_secs),
            two_phase_commit: false,
        };

        match txn_api.begin(options) {
            Ok(handle) => Ok::<_, HttpError>(TransactionResponse {
                transaction_id: handle.id(),
                status: "Active".to_string(),
            }),
            Err(e) => Err(HttpError::InternalError(format!(
                "Failed to begin transaction: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// Submit the transaction
pub async fn commit<
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
    Path(txn_id): Path<u64>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);

        match txn_api.commit(handle) {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Transaction committed successfully",
                "transaction_id": txn_id,
            })),
            Err(e) => Err(HttpError::InternalError(format!(
                "Failed to commit transaction: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// Roll back a transaction
pub async fn rollback<
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
    Path(txn_id): Path<u64>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);

        match txn_api.rollback(handle) {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Transaction rolled back successfully",
                "transaction_id": txn_id,
            })),
            Err(e) => Err(HttpError::InternalError(format!(
                "Failed to rollback transaction: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// ---------------------------------------------------------------------------
/// Savepoint endpoints
/// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateSavepointRequest {
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SavepointResponse {
    pub savepoint_id: u64,
    pub transaction_id: u64,
    pub name: Option<String>,
}

/// Create a savepoint within a transaction
pub async fn create_savepoint<
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
    Path(txn_id): Path<u64>,
    Json(request): Json<CreateSavepointRequest>,
) -> Result<JsonResponse<SavepointResponse>, HttpError> {
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);

        match txn_api.create_savepoint(handle, request.name.clone()) {
            Ok(sp_id) => Ok::<_, HttpError>(SavepointResponse {
                savepoint_id: sp_id.0,
                transaction_id: txn_id,
                name: request.name,
            }),
            Err(e) => Err(HttpError::InternalError(format!(
                "Failed to create savepoint: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// List all savepoints for a transaction
pub async fn get_savepoints<
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
    Path(txn_id): Path<u64>,
) -> Result<JsonResponse<Vec<serde_json::Value>>, HttpError> {
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);

        match txn_api.get_savepoints(handle) {
            Ok(savepoints) => Ok::<_, HttpError>(
                savepoints
                    .into_iter()
                    .map(|sp| {
                        serde_json::json!({
                            "id": sp.id,
                            "name": sp.name,
                            "created_at": format!("{:?}", sp.created_at),
                        })
                    })
                    .collect(),
            ),
            Err(e) => Err(HttpError::InternalError(format!(
                "Failed to list savepoints: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// Roll back to savepoint
pub async fn rollback_to_savepoint<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + graphdb_storage::storage::UndoTarget
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((txn_id, savepoint_id)): Path<(u64, u64)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);
        let sp_id = SavepointId(savepoint_id);
        let storage = state.server.get_storage();
        let storage_guard = storage.read();
        let storage_ref = &*storage_guard;

        match txn_api.rollback_to_savepoint(handle, sp_id, storage_ref) {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Rolled back to savepoint successfully",
                "transaction_id": txn_id,
                "savepoint_id": savepoint_id,
            })),
            Err(e) => Err(HttpError::InternalError(format!(
                "Failed to rollback to savepoint: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// Release (delete) a savepoint
pub async fn release_savepoint<
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
    Path((txn_id, savepoint_id)): Path<(u64, u64)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);
        let sp_id = SavepointId(savepoint_id);

        match txn_api.release_savepoint(handle, sp_id) {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Savepoint released successfully",
                "transaction_id": txn_id,
                "savepoint_id": savepoint_id,
            })),
            Err(e) => Err(HttpError::InternalError(format!(
                "Failed to release savepoint: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

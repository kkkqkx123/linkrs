use axum::{
    extract::{Json, Path, State},
    http::HeaderMap,
    response::Json as JsonResponse,
};
use graphdb_wire::meta::{BeginTransactionRequest, TransactionResponse};
use serde::{Deserialize, Serialize};
use tokio::task;

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use graphdb_transaction::{DurabilityLevel, IsolationLevel, TransactionOptions};
use graphdb_api::api_core::{SavepointId, TransactionHandle};

/// Start a transaction
pub async fn begin<
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
    headers: HeaderMap,
    Json(request): Json<BeginTransactionRequest>,
) -> Result<JsonResponse<TransactionResponse>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = task::spawn_blocking(move || {
        let txn_manager = state.server.get_txn_manager();

        let isolation_level = match request
            .isolation_level
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("repeatable_read") | Some("repeatable read") => {
                IsolationLevel::RepeatableRead
            }
            Some("read_committed") | Some("read committed") => IsolationLevel::ReadCommitted,
            Some("serializable") => IsolationLevel::Serializable,
            Some(value) => {
                return Err(HttpError::bad_request(format!(
                    "Unsupported isolation level: {}",
                    value
                )))
            }
        };
        let options = TransactionOptions {
            read_only: request.read_only,
            timeout: request.timeout_seconds.map(std::time::Duration::from_secs),
            durability: DurabilityLevel::Sync,
            isolation_level,
            query_timeout: request
                .query_timeout_seconds
                .map(std::time::Duration::from_secs),
            statement_timeout: request
                .statement_timeout_seconds
                .map(std::time::Duration::from_secs),
            idle_timeout: request
                .idle_timeout_seconds
                .map(std::time::Duration::from_secs),
        };

        let transaction = match owner {
            Some(owner) => txn_manager.begin_transaction_with_owner(options, owner),
            None => txn_manager.begin_transaction(options),
        };

        match transaction {
            Ok(handle) => Ok::<_, HttpError>(TransactionResponse {
                transaction_id: handle.as_u64(),
                status: "Active".to_string(),
            }),
            Err(e) => Err(HttpError::transaction_message(format!(
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
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path(txn_id): Path<u64>,
    headers: HeaderMap,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = task::spawn_blocking(move || {
        match state
            .server
            .get_txn_manager()
            .commit_transaction_as_owner(txn_id.into(), owner.as_deref())
        {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Transaction committed successfully",
                "transaction_id": txn_id,
            })),
            Err(e) => Err(HttpError::transaction_message(format!(
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
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path(txn_id): Path<u64>,
    headers: HeaderMap,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = task::spawn_blocking(move || {
        match state
            .server
            .get_txn_manager()
            .abort_transaction_as_owner(txn_id.into(), owner.as_deref())
        {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Transaction rolled back successfully",
                "transaction_id": txn_id,
            })),
            Err(e) => Err(HttpError::transaction_message(format!(
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
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path(txn_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<CreateSavepointRequest>,
) -> Result<JsonResponse<SavepointResponse>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);

        state
            .server
            .get_txn_manager()
            .check_transaction_owner(txn_id.into(), owner.as_deref())
            .map_err(|error| HttpError::transaction_message(error.to_string()))?;
        match txn_api.create_savepoint(handle, request.name.clone()) {
            Ok(sp_id) => Ok::<_, HttpError>(SavepointResponse {
                savepoint_id: sp_id.0,
                transaction_id: txn_id,
                name: request.name,
            }),
            Err(e) => Err(HttpError::transaction_message(format!(
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
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path(txn_id): Path<u64>,
    headers: HeaderMap,
) -> Result<JsonResponse<Vec<serde_json::Value>>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);

        state
            .server
            .get_txn_manager()
            .check_transaction_owner(txn_id.into(), owner.as_deref())
            .map_err(|error| HttpError::transaction_message(error.to_string()))?;
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
            Err(e) => Err(HttpError::transaction_message(format!(
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
        + StorageOperationContextOps
        + graphdb_storage::UndoTarget
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((txn_id, savepoint_id)): Path<(u64, u64)>,
    headers: HeaderMap,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);
        let sp_id = SavepointId(savepoint_id);
        let storage = state.server.get_storage();
        let storage_guard = storage.read();
        let storage_ref = &*storage_guard;

        state
            .server
            .get_txn_manager()
            .check_transaction_owner(txn_id.into(), owner.as_deref())
            .map_err(|error| HttpError::transaction_message(error.to_string()))?;
        match txn_api.rollback_to_savepoint(handle, sp_id, storage_ref) {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Rolled back to savepoint successfully",
                "transaction_id": txn_id,
                "savepoint_id": savepoint_id,
            })),
            Err(e) => Err(HttpError::transaction_message(format!(
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
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((txn_id, savepoint_id)): Path<(u64, u64)>,
    headers: HeaderMap,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = task::spawn_blocking(move || {
        let txn_api = state.server.get_txn_api();
        let handle = TransactionHandle::from(txn_id);
        let sp_id = SavepointId(savepoint_id);

        state
            .server
            .get_txn_manager()
            .check_transaction_owner(txn_id.into(), owner.as_deref())
            .map_err(|error| HttpError::transaction_message(error.to_string()))?;
        match txn_api.release_savepoint(handle, sp_id) {
            Ok(()) => Ok::<_, HttpError>(serde_json::json!({
                "message": "Savepoint released successfully",
                "transaction_id": txn_id,
                "savepoint_id": savepoint_id,
            })),
            Err(e) => Err(HttpError::transaction_message(format!(
                "Failed to release savepoint: {}",
                e
            ))),
        }
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

/// List transactions, including transactions waiting for recovery cleanup.
pub async fn list_transactions<
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
) -> Result<JsonResponse<Vec<serde_json::Value>>, HttpError> {
    let manager = state.server.get_txn_manager();
    let transactions = manager
        .list_transactions()
        .into_iter()
        .map(|info| {
            serde_json::json!({
                "transaction_id": info.id.as_u64(),
                "state": info.state.to_string(),
                "owner": info.owner,
                "read_timestamp": info.read_timestamp,
                "write_timestamp": info.write_timestamp,
                "elapsed_ms": info.elapsed.as_millis(),
                "last_activity_ms": info.last_activity.as_millis(),
                "rollback_only": info.rollback_only,
                "staged_bytes": info.staged_bytes,
                "undo_bytes": info.undo_bytes,
                "blocking_reason": info.blocking_reason,
            })
        })
        .collect();
    Ok(JsonResponse(transactions))
}

/// Return transaction outcome and resource gauges.
pub async fn metrics<
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
    let manager = state.server.get_txn_manager();
    let stats = manager.stats();
    let resources = manager.resource_metrics();
    Ok(JsonResponse(serde_json::json!({
        "outcomes": {
            "begun": stats.total_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "active": stats.active_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "committed": stats.committed_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "aborted": stats.aborted_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "timeout": stats.timeout_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "conflict": stats.conflict_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "disconnect": stats.disconnect_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "recovery_abort": stats.recovery_abort_transactions.load(std::sync::atomic::Ordering::Relaxed),
            "cleanup_failure": stats.cleanup_failure_transactions.load(std::sync::atomic::Ordering::Relaxed),
        },
        "resources": {
            "active_statements": stats.active_statements.load(std::sync::atomic::Ordering::Relaxed),
            "active_snapshots": resources.active_snapshots,
            "pending_writes": resources.pending_writes,
            "committed_frontier_lag": resources.committed_frontier_lag,
            "staged_wal_bytes": resources.staged_wal_bytes,
            "undo_bytes": resources.undo_bytes,
            "checkpoint_drain_time_ms": resources.checkpoint_drain_time.as_millis(),
        },
    })))
}

/// Kill a transaction after validating its owner header.
pub async fn kill_transaction<
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
    Path(txn_id): Path<u64>,
    headers: HeaderMap,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok());
    state
        .server
        .get_txn_manager()
        .kill_transaction(txn_id.into(), owner)
        .map_err(|error| HttpError::transaction_message(error.to_string()))?;
    Ok(JsonResponse(serde_json::json!({
        "transaction_id": txn_id,
        "status": "Aborted",
    })))
}

/// Retry outbox projection for a specific transaction's pending intents.
pub async fn retry_transaction_outbox<
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
    Path(txn_id): Path<u64>,
    headers: HeaderMap,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let owner = headers
        .get("x-transaction-owner")
        .and_then(|value| value.to_str().ok());
    let manager = state.server.get_txn_manager();
    manager
        .check_transaction_owner(txn_id.into(), owner)
        .map_err(|error| HttpError::transaction_message(error.to_string()))?;
    let delivered = manager
        .retry_outbox_projection()
        .map_err(|error| HttpError::transaction_message(error.to_string()))?;
    Ok(JsonResponse(serde_json::json!({
        "transaction_id": txn_id,
        "delivered": delivered,
        "status": "completed",
    })))
}

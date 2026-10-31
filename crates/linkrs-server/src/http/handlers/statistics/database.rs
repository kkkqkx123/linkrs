//! Database overview handler.
//!
//! The assembly lives in [`crate::services::statistics`]; this handler only
//! gathers the storage snapshot and maps the response.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::services::statistics::collect_database_overview;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_wire::meta::DatabaseOverviewResponse;

#[utoipa::path(
    get,
    path = "/v1/statistics/database",
    tag = "Statistics",
    responses(
        (status = 200, body = DatabaseOverviewResponse, description = "Database statistics"),
        (status = 500, description = "Internal error")
    )
)]
/// Obtain database statistics
pub async fn database<
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
) -> Result<JsonResponse<DatabaseOverviewResponse>, HttpError> {
    let stats_manager = state.server.get_stats_manager();
    let storage = state.server.get_storage();

    // Using `spawn_blocking` in an asynchronous context to acquire a storage lock
    let storage_stats = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            let storage = storage.read();
            storage.get_storage_stats()
        })
        .await
        .map_err(|e| HttpError::internal(format!("Failed to get storage statistics: {:?}", e)))?
    };

    Ok(JsonResponse(collect_database_overview(
        stats_manager,
        &storage_stats,
    )))
}

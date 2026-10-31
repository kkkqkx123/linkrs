//! Migration statistics handler.
//!
//! The counters live in [`crate::services::statistics`]; this handler only
//! maps the response onto the HTTP envelope.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::services::statistics::migration_core;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[utoipa::path(
    get,
    path = "/v1/statistics/migration",
    tag = "Statistics",
    responses(
        (status = 200, body = serde_json::Value, description = "Migration statistics"),
        (status = 500, description = "Internal error")
    )
)]
/// Migration statistics
pub async fn migration<
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
    Ok(JsonResponse(migration_core(&state)))
}

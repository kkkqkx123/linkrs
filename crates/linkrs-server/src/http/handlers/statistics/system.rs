//! System resource handler.
//!
//! The probes live in [`crate::services::statistics`]; this handler only maps
//! the response onto the HTTP envelope.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::services::statistics::collect_system_resource;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_wire::meta::SystemResourceResponse;

#[utoipa::path(
    get,
    path = "/v1/statistics/system",
    tag = "Statistics",
    responses(
        (status = 200, body = SystemResourceResponse, description = "System resource usage"),
        (status = 500, description = "Internal error")
    )
)]
/// Obtaining information about the use of system resources
pub async fn system<
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
) -> Result<JsonResponse<SystemResourceResponse>, HttpError> {
    Ok(JsonResponse(collect_system_resource(&state).await))
}

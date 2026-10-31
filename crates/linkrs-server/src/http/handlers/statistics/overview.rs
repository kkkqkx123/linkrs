//! Aggregated monitoring overview handler.
//!
//! The aggregation lives in [`crate::services::statistics`]; this handler only
//! maps the response onto the HTTP envelope.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::services::statistics::overview_core;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_wire::meta::OverviewResponse;

#[utoipa::path(
    get,
    operation_id = "get_v1_statistics_overview",
    path = "/v1/statistics/overview",
    tag = "Statistics",
    responses(
        (status = 200, body = OverviewResponse, description = "Aggregated monitoring overview"),
        (status = 500, description = "Internal error")
    )
)]
/// Aggregated overview across system, query, storage, transaction and sync.
pub async fn overview<
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
) -> Result<JsonResponse<OverviewResponse>, HttpError> {
    Ok(JsonResponse(overview_core(&state).await?))
}

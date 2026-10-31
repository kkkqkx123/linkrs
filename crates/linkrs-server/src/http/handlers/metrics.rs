//! Prometheus metrics endpoint.
//!
//! Renders the live `StatsManager` state in the Prometheus text exposition
//! format so generic monitoring stacks can scrape this instance next to the
//! JSON statistics API. The renderer itself lives in `linkrs-metrics` so the
//! embedded API and CLI expose byte-identical output.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_metrics::{render_prometheus, PROMETHEUS_CONTENT_TYPE};

#[utoipa::path(
    get,
    path = "/v1/metrics",
    operation_id = "get_v1_metrics",
    tag = "Statistics",
    responses(
        (status = 200, description = "Prometheus metrics in the text exposition format"),
        (status = 500, description = "Internal error")
    )
)]
/// Serve all collected metrics in the Prometheus text format.
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
) -> Result<Response, HttpError> {
    let stats = state.server.get_stats_manager().clone();
    let body = render_prometheus(&stats);
    Ok(([(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)], body).into_response())
}

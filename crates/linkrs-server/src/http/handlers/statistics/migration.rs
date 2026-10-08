//! Migration statistics handler.
//!
//! Reports migration counters from the stats manager and migration engine.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_metrics::MetricType;

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
    let stats_manager = state.server.get_stats_manager();
    let total = stats_manager
        .get_value(MetricType::MigrationTotalCount)
        .unwrap_or(0);
    let rows = stats_manager
        .get_value(MetricType::MigrationRowsMigrated)
        .unwrap_or(0);
    let duration = stats_manager
        .get_value(MetricType::MigrationDurationMs)
        .unwrap_or(0);
    let errors = stats_manager
        .get_value(MetricType::MigrationErrorsTotal)
        .unwrap_or(0);
    let active = stats_manager
        .get_value(MetricType::MigrationActiveCount)
        .unwrap_or(0);
    let avg_duration = if total > 0 {
        duration as f64 / total as f64
    } else {
        0.0
    };
    let snapshot = linkrs_migration::global_migration_metrics().snapshot();
    Ok(JsonResponse(serde_json::json!({
        "stats_manager": {
            "total_migrations": total,
            "active_migrations": active,
            "rows_migrated": rows,
            "duration_ms": duration,
            "avg_duration_ms": avg_duration,
            "errors": errors,
        },
        "migration_metrics": {
            "total_migrations": snapshot.total_migrations,
            "successful_migrations": snapshot.successful_migrations,
            "failed_migrations": snapshot.failed_migrations,
            "total_rows_migrated": snapshot.total_rows_migrated,
            "total_duration_ms": snapshot.total_duration_ms,
        }
    })))
}

//! Aggregated monitoring overview handler.
//!
//! Combines system, database, storage, transaction and sync snapshots.

use axum::{extract::State, response::Json as JsonResponse};

use super::database::collect_database_overview;
use super::system::collect_system_resource;
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_wire::meta::{
    LatencyPercentilesUs, OverviewErrors, OverviewResponse, OverviewStorage, OverviewSync,
    OverviewTimeseriesPoint, OverviewTransaction,
};

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
    let stats_manager = state.server.get_stats_manager();
    let storage = state.server.get_storage();

    let storage_stats = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            let storage = storage.read();
            storage.get_storage_stats()
        })
        .await
        .map_err(|e| HttpError::internal(format!("Failed to get storage statistics: {:?}", e)))?
    };

    let system = collect_system_resource(&state).await;
    let database = collect_database_overview(stats_manager, &storage_stats);
    let latency = stats_manager.query_latency_snapshot();
    let errors = stats_manager.error_snapshot();
    let storage_snapshot = stats_manager.storage_snapshot();
    let checkpoint = stats_manager.checkpoint_snapshot();
    let txn = stats_manager.transaction_snapshot();

    let graph_service = state.server.get_graph_service();
    let sync_summary = if let Some(sync_api) = graph_service.sync_api() {
        let outbox = sync_api.outbox_stats();
        OverviewSync {
            is_running: sync_api.is_running(),
            outbox_pending: outbox.pending,
            outbox_retries: outbox.retries,
            outbox_dead_lettered: outbox.dead_lettered,
        }
    } else {
        OverviewSync {
            is_running: false,
            outbox_pending: 0,
            outbox_retries: 0,
            outbox_dead_lettered: 0,
        }
    };

    let timeseries = stats_manager
        .query_timeseries(300)
        .into_iter()
        .map(|b| OverviewTimeseriesPoint {
            second: b.second,
            queries: b.queries,
            avg_latency_ms: b.avg_latency_ms(),
            errors: b.errors,
        })
        .collect::<Vec<_>>();

    Ok(JsonResponse(OverviewResponse {
        system,
        database,
        query_latency_us: LatencyPercentilesUs {
            avg: latency.avg_us,
            p50: latency.p50_us,
            p95: latency.p95_us,
            p99: latency.p99_us,
        },
        errors: OverviewErrors {
            total: errors.total_errors,
        },
        storage: OverviewStorage {
            read_ops: storage_snapshot.read_ops,
            write_ops: storage_snapshot.write_ops,
            fragmentation_permille: storage_snapshot.fragmentation_permille,
            tombstone_count: storage_snapshot.tombstone_count,
            checkpoint_success: checkpoint.success_count,
            checkpoint_failure: checkpoint.failure_count,
        },
        transaction: OverviewTransaction {
            begun: txn.begun,
            committed: txn.committed,
            rolled_back: txn.rolled_back,
            active: txn.active,
        },
        sync: sync_summary,
        timeseries,
    }))
}

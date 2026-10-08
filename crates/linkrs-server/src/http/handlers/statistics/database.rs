//! Database overview handlers.
//!
//! Assembles storage and performance counters into the database overview.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_metrics::MetricType;
use linkrs_wire::meta::{
    CheckpointSummary, DatabaseOverviewResponse, DatabasePerformance, DatabaseSearchSummary,
    DatabaseSpaces, DatabaseStorage, LatencyPercentilesUs,
};

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

pub(crate) fn collect_database_overview(
    stats_manager: &linkrs_metrics::StatsManager,
    storage_stats: &crate::storage::StorageStats,
) -> DatabaseOverviewResponse {
    // Obtain statistics related to the query from both sources
    let total_queries = stats_manager.get_value(MetricType::NumQueries).unwrap_or(0);
    let active_queries = stats_manager
        .get_value(MetricType::NumActiveQueries)
        .unwrap_or(0);

    // Obtain the cache size
    let cache_size = stats_manager.query_cache_size();

    // Calculating performance metrics over the recent window.
    let recent_queries = stats_manager.get_recent_queries(100);
    let avg_latency_ms = if recent_queries.is_empty() {
        0.0
    } else {
        recent_queries
            .iter()
            .map(|q| q.total_duration_us)
            .sum::<u64>() as f64
            / recent_queries.len() as f64
            / 1000.0
    };

    // QPS from the per-second aggregation, not from recent-profile spans.
    let qps = stats_manager.get_current_qps() as f64;

    // Obtain search metrics
    let num_search_queries = stats_manager
        .get_value(MetricType::NumSearchQueries)
        .unwrap_or(0);
    let num_search_errors = stats_manager
        .get_value(MetricType::NumSearchErrors)
        .unwrap_or(0);
    let search_latency_ms = stats_manager
        .get_value(MetricType::SearchLatencyMs)
        .unwrap_or(0);
    let num_index_operations = stats_manager
        .get_value(MetricType::NumIndexOperations)
        .unwrap_or(0);
    let num_delete_operations = stats_manager
        .get_value(MetricType::NumDeleteOperations)
        .unwrap_or(0);
    let cache_hit_count = stats_manager
        .get_value(MetricType::SearchCacheHitCount)
        .unwrap_or(0);
    let cache_miss_count = stats_manager
        .get_value(MetricType::SearchCacheMissCount)
        .unwrap_or(0);

    let avg_search_latency_ms = if num_search_queries > 0 {
        search_latency_ms as f64 / num_search_queries as f64
    } else {
        0.0
    };

    let search_cache_hit_rate = if cache_hit_count + cache_miss_count > 0 {
        cache_hit_count as f64 / (cache_hit_count + cache_miss_count) as f64
    } else {
        0.0
    };

    let latency = stats_manager.query_latency_snapshot();
    let errors = stats_manager.error_snapshot();
    let storage_snapshot = stats_manager.storage_snapshot();
    let checkpoint = stats_manager.checkpoint_snapshot();

    DatabaseOverviewResponse {
        spaces: DatabaseSpaces {
            count: storage_stats.total_spaces,
            total_vertices: storage_stats.total_vertices,
            total_edges: storage_stats.total_edges,
        },
        storage: DatabaseStorage {
            // StorageStats semantics: total is allocated bytes including
            // holes and overhead, data is the live estimate, index is the
            // derived residual `total - data` (fragmentation plus overhead).
            total_size_bytes: storage_stats.total_size_bytes,
            index_size_bytes: storage_stats.index_size_bytes,
            data_size_bytes: storage_stats.data_size_bytes,
            fragmentation_permille: storage_snapshot.fragmentation_permille,
            wasted_bytes: storage_snapshot.wasted_bytes,
            tombstone_count: storage_snapshot.tombstone_count,
            tombstone_memory_bytes: storage_snapshot.tombstone_memory_bytes,
            dirty_pages: storage_snapshot.dirty_pages,
            dirty_pages_total: storage_snapshot.dirty_pages_total,
            checkpoint: CheckpointSummary {
                success_count: checkpoint.success_count,
                failure_count: checkpoint.failure_count,
                trigger_count: checkpoint.trigger_count,
                avg_duration_us: checkpoint.avg_duration_us,
                triggered_by_wal_size: checkpoint.triggered_by_wal_size,
                triggered_by_interval: checkpoint.triggered_by_interval,
                triggered_explicit: checkpoint.triggered_explicit,
            },
        },
        performance: DatabasePerformance {
            total_queries,
            active_queries,
            query_cache_size: cache_size,
            queries_per_second: qps,
            avg_latency_ms,
            cache_hit_rate: search_cache_hit_rate,
            cache_hit_rate_source: "search_cache".to_string(),
            error_total: errors.total_errors,
            latency_percentiles_us: Some(LatencyPercentilesUs {
                avg: latency.avg_us,
                p50: latency.p50_us,
                p95: latency.p95_us,
                p99: latency.p99_us,
            }),
        },
        search: DatabaseSearchSummary {
            total_queries: num_search_queries,
            total_errors: num_search_errors,
            avg_latency_ms: avg_search_latency_ms,
            total_index_operations: num_index_operations,
            total_delete_operations: num_delete_operations,
            cache_hit_count,
            cache_miss_count,
            cache_hit_rate: search_cache_hit_rate,
        },
    }
}

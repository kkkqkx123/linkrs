//! Query and search statistics handlers.
//!
//! Covers session statistics, query aggregations, search metrics,
//! single query portraits and migration counters.

use axum::{
    extract::{Path, Query, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use graphdb_metrics::MetricType;
use graphdb_wire::meta::{
    CacheSection, DeleteSection, ExecutorSummaryEntry, IndexSection, LatencyPercentilesUs,
    QueryPatternEntry, QueryProfileDetailResponse, QueryProfileExecutor, QueryStatsResponse,
    SearchIndexEntry, SearchSection, SearchStatsResponse, SlowQueryInfo,
};
use graphdb_wire::query::QueryStageTimings;

#[utoipa::path(
    get,
    path = "/v1/statistics/sessions/{id}",
    tag = "Statistics",
    params(("id" = i64, Path, description = "Session id")),
    responses(
        (status = 200, body = serde_json::Value, description = "Session statistics"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Obtaining session statistics
pub async fn session<
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
    Path(session_id): Path<i64>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let session_manager = state.server.get_session_manager();
    let stats_manager = state.server.get_stats_manager();

    let session = session_manager
        .find_session(session_id)
        .ok_or_else(|| HttpError::NotFound(format!("Session does not exist: {}", session_id)))?;

    // Obtain session-related query statistics
    let session_queries = stats_manager.get_session_queries(session_id, 1000);
    let total_queries = session_queries.len() as u64;

    // Calculate the average execution time.
    let avg_execution_time_ms = if total_queries > 0 {
        session_queries
            .iter()
            .map(|q| q.total_duration_us)
            .sum::<u64>() as f64
            / total_queries as f64
            / 1000.0
    } else {
        0.0
    };

    // Obtain session-level change statistics
    let session_stats = session.statistics();
    let total_changes = session_stats.total_changes();
    let last_insert_vertex_id = session_stats.last_insert_vertex_id();
    let last_insert_edge_id = session_stats.last_insert_edge_id();

    Ok(JsonResponse(serde_json::json!({
        "session_id": session_id,
        "username": session.user(),
        "statistics": {
            "total_queries": total_queries,
            "total_changes": total_changes,
            "last_insert_vertex_id": last_insert_vertex_id,
            "last_insert_edge_id": last_insert_edge_id,
            "avg_execution_time_ms": avg_execution_time_ms,
        },
    })))
}

#[utoipa::path(
    get,
    path = "/v1/statistics/queries",
    tag = "Statistics",
    params(
        ("from" = Option<String>, Query, description = "Start of the query time window"),
        ("to" = Option<String>, Query, description = "End of the query time window")
    ),
    responses(
        (status = 200, body = QueryStatsResponse, description = "Query statistics"),
        (status = 500, description = "Internal error")
    )
)]
/// Obtain query statistics
pub async fn queries<
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
    Query(params): Query<QueryStatsParams>,
) -> Result<JsonResponse<QueryStatsResponse>, HttpError> {
    let stats_manager = state.server.get_stats_manager();

    // Get the total number of queries from both sources
    let total_queries = stats_manager.get_value(MetricType::NumQueries).unwrap_or(0);

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let from_secs =
        parse_time_bound(params.from.as_deref()).or(Some(now_secs.saturating_sub(3600)));
    let to_secs = parse_time_bound(params.to.as_deref());

    let window_profiles = stats_manager.profiles_in_window(from_secs, to_secs, 1000);
    let slow_queries = window_profiles
        .into_iter()
        .filter(|p| p.total_duration_us >= stats_manager_slow_threshold(stats_manager))
        .take(10)
        .map(slow_info_from_profile)
        .collect::<Vec<_>>();

    // Obtain statistics for various types of queries
    let match_queries = stats_manager
        .get_value(MetricType::NumMatchQueries)
        .unwrap_or(0);
    let create_queries = stats_manager
        .get_value(MetricType::NumCreateQueries)
        .unwrap_or(0);
    let update_queries = stats_manager
        .get_value(MetricType::NumUpdateQueries)
        .unwrap_or(0);
    let delete_queries = stats_manager
        .get_value(MetricType::NumDeleteQueries)
        .unwrap_or(0);
    let insert_queries = stats_manager
        .get_value(MetricType::NumInsertQueries)
        .unwrap_or(0);
    let go_queries = stats_manager
        .get_value(MetricType::NumGoQueries)
        .unwrap_or(0);
    let fetch_queries = stats_manager
        .get_value(MetricType::NumFetchQueries)
        .unwrap_or(0);
    let lookup_queries = stats_manager
        .get_value(MetricType::NumLookupQueries)
        .unwrap_or(0);
    let show_queries = stats_manager
        .get_value(MetricType::NumShowQueries)
        .unwrap_or(0);

    let error_snapshot = stats_manager.error_snapshot();
    let patterns = stats_manager
        .pattern_snapshot(10)
        .into_iter()
        .map(|p| QueryPatternEntry {
            normalized_query: p.normalized_query,
            query_type: p.query_type,
            labels: p.labels,
            execution_count: p.execution_count,
            avg_duration_ms: p.avg_duration_ms,
            p95_duration_ms: p.p95_duration_ms,
            p99_duration_ms: p.p99_duration_ms,
            error_rate: p.error_rate,
            error_count: p.error_count,
        })
        .collect::<Vec<_>>();
    let executors = stats_manager
        .executor_summary_snapshot()
        .into_iter()
        .map(|e| ExecutorSummaryEntry {
            executor_type: e.executor_type,
            count: e.count,
            total_time_ms: e.total_time_ms,
            total_rows: e.total_rows,
            avg_time_ms: e.avg_time_ms,
        })
        .collect::<Vec<_>>();
    let latency = stats_manager.query_latency_snapshot();

    Ok(JsonResponse(QueryStatsResponse {
        total_queries,
        slow_queries,
        query_types: graphdb_wire::meta::QueryTypeStatistics {
            match_queries,
            create_queries,
            update_queries,
            delete_queries,
            insert_queries,
            go_queries,
            fetch_queries,
            lookup_queries,
            show_queries,
        },
        errors_by_type: error_snapshot.errors_by_type,
        errors_by_phase: error_snapshot.errors_by_phase,
        error_total: error_snapshot.total_errors,
        top_patterns: patterns,
        executor_summary: executors,
        latency_percentiles_us: Some(LatencyPercentilesUs {
            avg: latency.avg_us,
            p50: latency.p50_us,
            p95: latency.p95_us,
            p99: latency.p99_us,
        }),
        from: params.from,
        to: params.to,
    }))
}

#[utoipa::path(
    get,
    operation_id = "get_v1_statistics_search",
    path = "/v1/statistics/search",
    tag = "Statistics",
    responses(
        (status = 200, body = SearchStatsResponse, description = "Search statistics"),
        (status = 500, description = "Internal error")
    )
)]
/// Obtain search statistics
pub async fn search<
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
) -> Result<JsonResponse<SearchStatsResponse>, HttpError> {
    let stats_manager = state.server.get_stats_manager();

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
    let num_index_errors = stats_manager
        .get_value(MetricType::NumIndexErrors)
        .unwrap_or(0);
    let index_latency_ms = stats_manager
        .get_value(MetricType::IndexLatencyMs)
        .unwrap_or(0);
    let num_delete_operations = stats_manager
        .get_value(MetricType::NumDeleteOperations)
        .unwrap_or(0);
    let num_delete_errors = stats_manager
        .get_value(MetricType::NumDeleteErrors)
        .unwrap_or(0);
    let delete_latency_ms = stats_manager
        .get_value(MetricType::DeleteLatencyMs)
        .unwrap_or(0);
    let search_result_count = stats_manager
        .get_value(MetricType::SearchResultCount)
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

    let avg_index_latency_ms = if num_index_operations > 0 {
        index_latency_ms as f64 / num_index_operations as f64
    } else {
        0.0
    };

    let avg_delete_latency_ms = if num_delete_operations > 0 {
        delete_latency_ms as f64 / num_delete_operations as f64
    } else {
        0.0
    };

    let cache_hit_rate = if cache_hit_count + cache_miss_count > 0 {
        cache_hit_count as f64 / (cache_hit_count + cache_miss_count) as f64
    } else {
        0.0
    };

    let (search_avg_us, search_p50_us, search_p95_us, search_p99_us) =
        stats_manager.get_search_latency_percentiles();

    let by_index = stats_manager
        .search_index_breakdown()
        .into_iter()
        .map(|b| SearchIndexEntry {
            index: b.index,
            search_queries: b.search_queries,
            search_errors: b.search_errors,
            search_latency_ms: b.search_latency_ms,
            avg_search_latency_ms: b.avg_search_latency_ms,
            index_operations: b.index_operations,
            index_errors: b.index_errors,
            index_latency_ms: b.index_latency_ms,
        })
        .collect::<Vec<_>>();

    Ok(JsonResponse(SearchStatsResponse {
        search: SearchSection {
            total_queries: num_search_queries,
            total_errors: num_search_errors,
            total_latency_ms: search_latency_ms,
            avg_latency_ms: avg_search_latency_ms,
            total_results: search_result_count,
            latency_percentiles_us: LatencyPercentilesUs {
                avg: search_avg_us,
                p50: search_p50_us,
                p95: search_p95_us,
                p99: search_p99_us,
            },
        },
        index: IndexSection {
            total_operations: num_index_operations,
            total_errors: num_index_errors,
            total_latency_ms: index_latency_ms,
            avg_latency_ms: avg_index_latency_ms,
        },
        delete: DeleteSection {
            total_operations: num_delete_operations,
            total_errors: num_delete_errors,
            total_latency_ms: delete_latency_ms,
            avg_latency_ms: avg_delete_latency_ms,
        },
        cache: CacheSection {
            hit_count: cache_hit_count,
            miss_count: cache_miss_count,
            hit_rate: cache_hit_rate,
        },
        by_index,
    }))
}

#[utoipa::path(
    get,
    operation_id = "get_v1_statistics_query_profile",
    path = "/v1/statistics/queries/{trace_id}",
    tag = "Statistics",
    params(("trace_id" = String, Path, description = "Query trace id")),
    responses(
        (status = 200, body = QueryProfileDetailResponse, description = "Query portrait detail"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Single query portrait by trace id.
pub async fn query_profile_detail<
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
    Path(trace_id): Path<String>,
) -> Result<JsonResponse<QueryProfileDetailResponse>, HttpError> {
    let stats_manager = state.server.get_stats_manager();
    let profile = stats_manager.get_query_profile(&trace_id).ok_or_else(|| {
        HttpError::NotFound(format!("Query portrait does not exist: {}", trace_id))
    })?;
    let status = match profile.status {
        graphdb_metrics::QueryStatus::Success => "success",
        graphdb_metrics::QueryStatus::Failed => "failed",
    }
    .to_string();
    let error = profile
        .error_info
        .as_ref()
        .map(|info| info.error_message.clone())
        .or(profile.error_message.clone());
    Ok(JsonResponse(QueryProfileDetailResponse {
        trace_id: profile.trace_id.clone(),
        session_id: profile.session_id,
        query: profile.query_text.clone(),
        duration_ms: profile.total_duration_us as f64 / 1000.0,
        status,
        error,
        stages: Some(stages_from_profile(&profile)),
        executors: profile
            .executor_stats
            .iter()
            .map(|e| QueryProfileExecutor {
                executor_type: e.executor_type.clone(),
                duration_ms: e.duration_ms(),
                rows: e.rows_processed(),
                memory_bytes: e.memory_used(),
            })
            .collect(),
        result_count: Some(profile.result_count),
        plan_node_count: Some(profile.plan_node_count),
    }))
}

/// Query statistical parameters
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct QueryStatsParams {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

pub(crate) fn parse_time_bound(s: Option<&str>) -> Option<u64> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(v) = s.parse::<u64>() {
        return Some(v);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp().max(0) as u64);
    }
    None
}

pub(crate) fn stats_manager_slow_threshold(stats: &graphdb_metrics::StatsManager) -> u64 {
    stats.slow_query_threshold_us()
}

pub(crate) fn stages_from_profile(profile: &graphdb_metrics::QueryProfile) -> QueryStageTimings {
    QueryStageTimings {
        parse_ms: profile.stages.parse_ms(),
        validate_ms: profile.stages.validate_ms(),
        plan_ms: profile.stages.plan_ms(),
        optimize_ms: profile.stages.optimize_ms(),
        execute_ms: profile.stages.execute_ms(),
    }
}

pub(crate) fn slow_info_from_profile(profile: graphdb_metrics::QueryProfile) -> SlowQueryInfo {
    let status = match profile.status {
        graphdb_metrics::QueryStatus::Success => "success",
        graphdb_metrics::QueryStatus::Failed => "failed",
    }
    .to_string();
    SlowQueryInfo {
        trace_id: profile.trace_id.clone(),
        session_id: profile.session_id,
        query: profile.query_text.clone(),
        duration_ms: profile.total_duration_us as f64 / 1000.0,
        status,
        started_at_secs: Some(profile.started_at_secs),
        stages: Some(stages_from_profile(&profile)),
        plan_node_count: Some(profile.plan_node_count),
    }
}

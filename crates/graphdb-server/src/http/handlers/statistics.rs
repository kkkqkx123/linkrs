//! Statistical information about the HTTP processor

use axum::{
    extract::{Path, Query, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;
use serde_json;

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSnapshotOps,
    StorageSyncContextOps,
};
use graphdb_metrics::MetricType;
use graphdb_wire::meta::{
    CacheSection, CheckpointSummary, ConnectionStats, DatabaseOverviewResponse,
    DatabasePerformance, DatabaseSearchSummary, DatabaseSpaces, DatabaseStorage, DeleteSection,
    ExecutorSummaryEntry, IndexSection, LatencyPercentilesUs, MemoryUsage, OverviewErrors,
    OverviewResponse, OverviewStorage, OverviewSync, OverviewTimeseriesPoint, OverviewTransaction,
    QueryPatternEntry, QueryProfileDetailResponse, QueryProfileExecutor, QueryStatsResponse,
    SearchIndexEntry, SearchSection, SearchStatsResponse, SlowQueryInfo, SystemResourceResponse,
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

/// Obtaining memory information (number of bytes used and total number of bytes)
/// Implementing cross-platform support using the sysinfo crate
fn get_memory_info() -> (u64, u64) {
    use sysinfo::System;

    // Create an instance of system information and refresh the memory information.
    let mut sys = System::new();
    sys.refresh_memory();

    // Get the total system memory and the used memory (both converted to bytes).
    let total_memory = sys.total_memory() * 1024;
    let used_memory = sys.used_memory() * 1024;

    (used_memory, total_memory)
}

/// Obtain the percentage of CPU usage with two samples so the delta is real.
async fn get_cpu_usage_two_sample() -> f64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_cpu_usage();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    sys.refresh_cpu_usage();

    let cpus = sys.cpus();
    if cpus.is_empty() {
        0.0
    } else {
        let avg_usage: f32 =
            cpus.iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / cpus.len() as f32;
        avg_usage as f64
    }
}

fn get_process_memory_bytes() -> Option<u64> {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let pid = sysinfo::get_current_pid().ok()?;
    sys.process(pid).map(|p| p.memory() * 1024)
}

fn get_process_uptime_secs() -> Option<u64> {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let pid = sysinfo::get_current_pid().ok()?;
    sys.process(pid).map(|p| p.run_time())
}

fn dir_size(path: &std::path::Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.is_file() {
        return Some(metadata.len());
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            visited += 1;
            if visited > 200_000 {
                return Some(total);
            }
            if let Ok(md) = entry.metadata() {
                if md.is_dir() {
                    stack.push(entry.path());
                } else {
                    total = total.saturating_add(md.len());
                }
            }
        }
    }
    Some(total)
}

fn max_file_descriptors() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/self/limits").ok()?;
    for line in content.lines() {
        if line.trim_start().starts_with("Max open files") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                if let Ok(v) = parts[3].parse::<u64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn parse_time_bound(s: Option<&str>) -> Option<u64> {
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

fn stats_manager_slow_threshold(stats: &graphdb_metrics::StatsManager) -> u64 {
    stats.slow_query_threshold_us()
}

fn stages_from_profile(profile: &graphdb_metrics::QueryProfile) -> QueryStageTimings {
    QueryStageTimings {
        parse_ms: profile.stages.parse_ms(),
        validate_ms: profile.stages.validate_ms(),
        plan_ms: profile.stages.plan_ms(),
        optimize_ms: profile.stages.optimize_ms(),
        execute_ms: profile.stages.execute_ms(),
    }
}

fn slow_info_from_profile(profile: graphdb_metrics::QueryProfile) -> SlowQueryInfo {
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

async fn collect_system_resource<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
) -> SystemResourceResponse {
    let session_manager = state.server.get_session_manager();
    let stats_manager = state.server.get_stats_manager();

    let active_connections = session_manager.active_session_count().await;
    let max_connections = session_manager.max_connections();

    let (memory_used, memory_total) = get_memory_info();
    let cpu_usage = get_cpu_usage_two_sample().await;
    let process_memory = get_process_memory_bytes();
    let uptime_secs = get_process_uptime_secs().or(Some(sysinfo::System::uptime()));
    let max_fds = max_file_descriptors();

    let storage_path = state.server.get_config().storage_path().to_string();
    let data_path = std::path::PathBuf::from(&storage_path);
    let data_dir_size = dir_size(&data_path);
    let wal_dir_size = dir_size(&data_path.join("wal"));

    stats_manager.record_resource_sample(memory_used, memory_total, cpu_usage);

    SystemResourceResponse {
        cpu_usage_percent: cpu_usage,
        memory_usage: MemoryUsage {
            used_bytes: memory_used,
            total_bytes: memory_total,
        },
        connections: ConnectionStats {
            active: active_connections,
            total: active_connections,
            max: max_connections,
        },
        process_memory_bytes: process_memory,
        uptime_secs,
        data_dir_size_bytes: data_dir_size,
        wal_dir_size_bytes: wal_dir_size,
        max_file_descriptors: max_fds,
    }
}

fn collect_database_overview(
    stats_manager: &graphdb_metrics::StatsManager,
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

#[utoipa::path(
    get,
    path = "/v1/statistics/freeze",
    tag = "Statistics",
    responses(
        (status = 200, body = serde_json::Value, description = "Background freeze statistics"),
        (status = 500, description = "Internal error")
    )
)]
/// Get background freeze statistics
pub async fn freeze_stats<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let storage = state.server.get_storage();

    let freeze_stats = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            let storage = storage.read();
            storage.get_freeze_stats()
        })
        .await
        .map_err(|e| HttpError::internal(format!("Failed to get freeze stats: {:?}", e)))?
    };

    match freeze_stats {
        Some(stats) => Ok(JsonResponse(serde_json::json!({
            "freeze_count": stats.freeze_count,
            "total_frozen_edges": stats.total_frozen_edges,
            "last_freeze_duration_ms": stats.last_freeze_duration_ms,
            "current_delta_edges": stats.current_delta_edges,
        }))),
        None => Ok(JsonResponse(serde_json::json!({
            "enabled": false,
            "message": "Background freeze manager not configured",
        }))),
    }
}

#[utoipa::path(
    post,
    path = "/v1/statistics/freeze",
    tag = "Statistics",
    responses(
        (status = 200, body = serde_json::Value, description = "Background freeze triggered"),
        (status = 500, description = "Internal error")
    )
)]
/// Trigger background freeze manually
pub async fn trigger_freeze<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let storage = state.server.get_storage();

    let result = {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            let storage = storage.read();
            storage.trigger_background_freeze()
        })
        .await
        .map_err(|e| HttpError::internal(format!("Failed to trigger freeze: {:?}", e)))?
    };

    match result {
        Ok(()) => Ok(JsonResponse(serde_json::json!({
            "status": "ok",
            "message": "Background freeze triggered successfully",
        }))),
        Err(e) => Err(HttpError::internal(format!(
            "Background freeze failed: {}",
            e
        ))),
    }
}

/// Query statistical parameters
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct QueryStatsParams {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

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
    let snapshot = graphdb_migration::global_migration_metrics().snapshot();
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

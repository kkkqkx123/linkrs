//! Monitoring statistics shared by both transports.
//!
//! Assembles system resource probes, database counters, search metrics and
//! migration counters from the stats manager and storage snapshots.

use linkrs_metrics::MetricType;
use linkrs_wire::meta::{
    CacheSection, CheckpointSummary, ConnectionStats, DatabaseOverviewResponse,
    DatabasePerformance, DatabaseSearchSummary, DatabaseSpaces, DatabaseStorage, DeleteSection,
    EmbeddingSection, IndexSection, LatencyPercentilesUs, MemoryUsage, OverviewErrors,
    OverviewResponse, OverviewStorage, OverviewSync, OverviewTimeseriesPoint, OverviewTransaction,
    RerankSection, SearchIndexEntry, SearchSection, SearchStatsResponse, SystemResourceResponse,
    VectorSearchSection,
};

use crate::http::error::HttpError;
use crate::http::state::AppState;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

// ── System ────────────────────────────────────────────────────────────────

/// Collect process and host resource usage for the system endpoint.
pub(crate) async fn collect_system_resource<
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

/// Obtaining memory information (number of bytes used and total number of bytes)
/// Implementing cross-platform support using the sysinfo crate
pub(crate) fn get_memory_info() -> (u64, u64) {
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
pub(crate) async fn get_cpu_usage_two_sample() -> f64 {
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

pub(crate) fn get_process_memory_bytes() -> Option<u64> {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let pid = sysinfo::get_current_pid().ok()?;
    sys.process(pid).map(|p| p.memory() * 1024)
}

pub(crate) fn get_process_uptime_secs() -> Option<u64> {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let pid = sysinfo::get_current_pid().ok()?;
    sys.process(pid).map(|p| p.run_time())
}

pub(crate) fn dir_size(path: &std::path::Path) -> Option<u64> {
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

pub(crate) fn max_file_descriptors() -> Option<u64> {
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

// ── Database ──────────────────────────────────────────────────────────────

/// Assemble the database overview from stats and storage counters.
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

// ── Overview ──────────────────────────────────────────────────────────────

/// Aggregate system, database, storage, transaction and sync snapshots.
pub(crate) async fn overview_core<
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
) -> Result<OverviewResponse, HttpError> {
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

    let system = collect_system_resource(state).await;
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

    Ok(OverviewResponse {
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
    })
}

// ── Search ────────────────────────────────────────────────────────────────

/// Assemble the search statistics response.
pub(crate) fn search_core<S>(state: &AppState<S>) -> SearchStatsResponse
where
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
{
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
    let vector_queries = stats_manager
        .get_value(MetricType::VectorSearchOps)
        .unwrap_or(0);
    let vector_errors = stats_manager
        .get_value(MetricType::VectorSearchErrors)
        .unwrap_or(0);
    let vector_latency_ms = stats_manager
        .get_value(MetricType::VectorSearchLatencyMs)
        .unwrap_or(0);
    let vector_upserts = stats_manager
        .get_value(MetricType::VectorUpsertOps)
        .unwrap_or(0);
    let vector_upsert_errors = stats_manager
        .get_value(MetricType::VectorUpsertErrors)
        .unwrap_or(0);
    let vector_deletes = stats_manager
        .get_value(MetricType::VectorDeleteOps)
        .unwrap_or(0);
    let vector_delete_errors = stats_manager
        .get_value(MetricType::VectorDeleteErrors)
        .unwrap_or(0);
    let vector_disabled_skips = stats_manager
        .get_value(MetricType::VectorDisabledSkips)
        .unwrap_or(0);
    let embedding_ops = stats_manager
        .get_value(MetricType::VectorEmbeddingOps)
        .unwrap_or(0);
    let embedding_errors = stats_manager
        .get_value(MetricType::VectorEmbeddingErrors)
        .unwrap_or(0);
    let embedding_latency_ms = stats_manager
        .get_value(MetricType::VectorEmbeddingLatencyMs)
        .unwrap_or(0);
    let embedding_prompt_tokens = stats_manager
        .get_value(MetricType::VectorEmbeddingPromptTokens)
        .unwrap_or(0);
    let embedding_total_tokens = stats_manager
        .get_value(MetricType::VectorEmbeddingTotalTokens)
        .unwrap_or(0);
    let rerank_ops = stats_manager
        .get_value(MetricType::VectorRerankOps)
        .unwrap_or(0);
    let rerank_errors = stats_manager
        .get_value(MetricType::VectorRerankErrors)
        .unwrap_or(0);
    let rerank_latency_ms = stats_manager
        .get_value(MetricType::VectorRerankLatencyMs)
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

    let avg_vector_latency_ms = if vector_queries > 0 {
        vector_latency_ms as f64 / vector_queries as f64
    } else {
        0.0
    };
    let avg_embedding_latency_ms = if embedding_ops > 0 {
        embedding_latency_ms as f64 / embedding_ops as f64
    } else {
        0.0
    };
    let avg_rerank_latency_ms = if rerank_ops > 0 {
        rerank_latency_ms as f64 / rerank_ops as f64
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

    SearchStatsResponse {
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
        vector: VectorSearchSection {
            total_queries: vector_queries,
            total_errors: vector_errors,
            total_latency_ms: vector_latency_ms,
            avg_latency_ms: avg_vector_latency_ms,
            total_upserts: vector_upserts,
            upsert_errors: vector_upsert_errors,
            total_deletes: vector_deletes,
            delete_errors: vector_delete_errors,
            disabled_skips: vector_disabled_skips,
        },
        embedding: EmbeddingSection {
            total_operations: embedding_ops,
            total_errors: embedding_errors,
            total_latency_ms: embedding_latency_ms,
            avg_latency_ms: avg_embedding_latency_ms,
            prompt_tokens: embedding_prompt_tokens,
            total_tokens: embedding_total_tokens,
        },
        rerank: RerankSection {
            total_operations: rerank_ops,
            total_errors: rerank_errors,
            total_latency_ms: rerank_latency_ms,
            avg_latency_ms: avg_rerank_latency_ms,
        },
    }
}

// ── Migration ─────────────────────────────────────────────────────────────

/// Assemble the migration statistics response.
pub(crate) fn migration_core<S>(state: &AppState<S>) -> serde_json::Value
where
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
{
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
    serde_json::json!({
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
    })
}

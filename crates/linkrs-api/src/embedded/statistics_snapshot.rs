//! In-process statistics snapshots for the embedded API.
//!
//! The server exposes these snapshots over `/v1/statistics/*`. Embedded hosts
//! share the same `StatsManager` and storage counters, so the sections are
//! assembled here as plain JSON to keep `linkrs-api` free of wire DTOs.
//! The `system` section stays server-only: it depends on host resource probes
//! and listener/session bookkeeping that do not exist in-process.

use crate::embedded::database::GraphDatabase;
use linkrs_metrics::{MetricType, StatsManager};
use linkrs_storage::{StorageClient, StorageSnapshotOps};

fn latency_percentiles(stats: &StatsManager) -> serde_json::Value {
    let latency = stats.query_latency_snapshot();
    serde_json::json!({
        "avg": latency.avg_us,
        "p50": latency.p50_us,
        "p95": latency.p95_us,
        "p99": latency.p99_us,
    })
}

fn number(stats: &StatsManager, metric: MetricType) -> u64 {
    stats.get_value(metric).unwrap_or(0)
}

fn database_section(
    stats: &StatsManager,
    storage_stats: &linkrs_storage::StorageStats,
) -> serde_json::Value {
    let total_queries = number(stats, MetricType::NumQueries);
    let active_queries = number(stats, MetricType::NumActiveQueries);
    let cache_size = stats.query_cache_size();

    let recent_queries = stats.get_recent_queries(100);
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
    let qps = stats.get_current_qps() as f64;

    let num_search_queries = number(stats, MetricType::NumSearchQueries);
    let num_search_errors = number(stats, MetricType::NumSearchErrors);
    let search_latency_ms = number(stats, MetricType::SearchLatencyMs);
    let num_index_operations = number(stats, MetricType::NumIndexOperations);
    let num_delete_operations = number(stats, MetricType::NumDeleteOperations);
    let cache_hit_count = number(stats, MetricType::SearchCacheHitCount);
    let cache_miss_count = number(stats, MetricType::SearchCacheMissCount);

    let avg_search_latency_ms = if num_search_queries > 0 {
        search_latency_ms as f64 / num_search_queries as f64
    } else {
        0.0
    };
    let cache_hit_rate = if cache_hit_count + cache_miss_count > 0 {
        cache_hit_count as f64 / (cache_hit_count + cache_miss_count) as f64
    } else {
        0.0
    };

    let storage_snapshot = stats.storage_snapshot();
    let checkpoint = stats.checkpoint_snapshot();
    let errors = stats.error_snapshot();

    serde_json::json!({
        "spaces": {
            "count": storage_stats.total_spaces,
            "total_vertices": storage_stats.total_vertices,
            "total_edges": storage_stats.total_edges,
        },
        "storage": {
            "total_size_bytes": storage_stats.total_size_bytes,
            "index_size_bytes": storage_stats.index_size_bytes,
            "data_size_bytes": storage_stats.data_size_bytes,
            "fragmentation_permille": storage_snapshot.fragmentation_permille,
            "wasted_bytes": storage_snapshot.wasted_bytes,
            "tombstone_count": storage_snapshot.tombstone_count,
            "tombstone_memory_bytes": storage_snapshot.tombstone_memory_bytes,
            "dirty_pages": storage_snapshot.dirty_pages,
            "dirty_pages_total": storage_snapshot.dirty_pages_total,
            "checkpoint": {
                "success_count": checkpoint.success_count,
                "failure_count": checkpoint.failure_count,
                "trigger_count": checkpoint.trigger_count,
                "avg_duration_us": checkpoint.avg_duration_us,
                "triggered_by_wal_size": checkpoint.triggered_by_wal_size,
                "triggered_by_interval": checkpoint.triggered_by_interval,
                "triggered_explicit": checkpoint.triggered_explicit,
            },
        },
        "performance": {
            "total_queries": total_queries,
            "active_queries": active_queries,
            "query_cache_size": cache_size,
            "queries_per_second": qps,
            "avg_latency_ms": avg_latency_ms,
            "cache_hit_rate": cache_hit_rate,
            "cache_hit_rate_source": "search_cache",
            "error_total": errors.total_errors,
            "latency_percentiles_us": latency_percentiles(stats),
        },
        "search": {
            "total_queries": num_search_queries,
            "total_errors": num_search_errors,
            "avg_latency_ms": avg_search_latency_ms,
            "total_index_operations": num_index_operations,
            "total_delete_operations": num_delete_operations,
            "cache_hit_count": cache_hit_count,
            "cache_miss_count": cache_miss_count,
            "cache_hit_rate": cache_hit_rate,
        },
    })
}

fn profile_json(profile: &linkrs_metrics::QueryProfile, detail: bool) -> serde_json::Value {
    let status = match profile.status {
        linkrs_metrics::QueryStatus::Success => "success",
        linkrs_metrics::QueryStatus::Failed => "failed",
    };
    let error = profile
        .error_info
        .as_ref()
        .map(|info| info.error_message.clone())
        .or_else(|| profile.error_message.clone());
    let stages = serde_json::json!({
        "parse_ms": profile.stages.parse_ms(),
        "validate_ms": profile.stages.validate_ms(),
        "plan_ms": profile.stages.plan_ms(),
        "optimize_ms": profile.stages.optimize_ms(),
        "execute_ms": profile.stages.execute_ms(),
    });

    if detail {
        let executors: Vec<serde_json::Value> = profile
            .executor_stats
            .iter()
            .map(|e| {
                serde_json::json!({
                    "executor_type": e.executor_type,
                    "duration_ms": e.duration_ms(),
                    "rows": e.rows_processed(),
                    "memory_bytes": e.memory_used(),
                })
            })
            .collect();
        serde_json::json!({
            "trace_id": profile.trace_id,
            "session_id": profile.session_id,
            "query": profile.query_text,
            "duration_ms": profile.total_duration_us as f64 / 1000.0,
            "status": status,
            "error": error,
            "stages": stages,
            "executors": executors,
            "result_count": profile.result_count,
            "plan_node_count": profile.plan_node_count,
        })
    } else {
        serde_json::json!({
            "trace_id": profile.trace_id,
            "session_id": profile.session_id,
            "query": profile.query_text,
            "duration_ms": profile.total_duration_us as f64 / 1000.0,
            "status": status,
            "started_at_secs": profile.started_at_secs,
            "stages": stages,
            "plan_node_count": profile.plan_node_count,
        })
    }
}

impl<S: StorageClient + Clone + Send + Sync + 'static> GraphDatabase<S> {
    /// Aggregated statistics across database, query, storage, transaction and
    /// sync snapshots.
    ///
    /// Embedded equivalent of `GET /v1/statistics/overview`. The `system`
    /// section is intentionally absent: it depends on host probes that only
    /// the server runtime provides.
    pub fn statistics_overview(&self) -> serde_json::Value {
        let stats = self.stats_manager();
        let storage_stats = self.storage().get_storage_stats();

        let latency = latency_percentiles(&stats);
        let errors = stats.error_snapshot();
        let storage_snapshot = stats.storage_snapshot();
        let checkpoint = stats.checkpoint_snapshot();
        let txn = stats.transaction_snapshot();

        let sync_summary = match self.sync_api() {
            Some(sync_api) => {
                let outbox = sync_api.outbox_stats();
                serde_json::json!({
                    "is_running": sync_api.is_running(),
                    "outbox_pending": outbox.pending,
                    "outbox_retries": outbox.retries,
                    "outbox_dead_lettered": outbox.dead_lettered,
                })
            }
            None => serde_json::json!({
                "is_running": false,
                "outbox_pending": 0,
                "outbox_retries": 0,
                "outbox_dead_lettered": 0,
            }),
        };

        let timeseries: Vec<serde_json::Value> = stats
            .query_timeseries(300)
            .into_iter()
            .map(|b| {
                serde_json::json!({
                    "second": b.second,
                    "queries": b.queries,
                    "avg_latency_ms": b.avg_latency_ms(),
                    "errors": b.errors,
                })
            })
            .collect();

        serde_json::json!({
            "database": database_section(&stats, &storage_stats),
            "query_latency_us": latency,
            "errors": {
                "total": errors.total_errors,
            },
            "storage": {
                "read_ops": storage_snapshot.read_ops,
                "write_ops": storage_snapshot.write_ops,
                "fragmentation_permille": storage_snapshot.fragmentation_permille,
                "tombstone_count": storage_snapshot.tombstone_count,
                "checkpoint_success": checkpoint.success_count,
                "checkpoint_failure": checkpoint.failure_count,
            },
            "transaction": {
                "begun": txn.begun,
                "committed": txn.committed,
                "rolled_back": txn.rolled_back,
                "active": txn.active,
            },
            "sync": sync_summary,
            "timeseries": timeseries,
        })
    }

    /// Search statistics: fulltext search, index maintenance, cache and vector
    /// engine counters.
    ///
    /// Embedded equivalent of `GET /v1/statistics/search`.
    pub fn search_statistics(&self) -> serde_json::Value {
        let stats = self.stats_manager();

        let num_search_queries = number(&stats, MetricType::NumSearchQueries);
        let num_search_errors = number(&stats, MetricType::NumSearchErrors);
        let search_latency_ms = number(&stats, MetricType::SearchLatencyMs);
        let num_index_operations = number(&stats, MetricType::NumIndexOperations);
        let num_index_errors = number(&stats, MetricType::NumIndexErrors);
        let index_latency_ms = number(&stats, MetricType::IndexLatencyMs);
        let num_delete_operations = number(&stats, MetricType::NumDeleteOperations);
        let num_delete_errors = number(&stats, MetricType::NumDeleteErrors);
        let delete_latency_ms = number(&stats, MetricType::DeleteLatencyMs);
        let search_result_count = number(&stats, MetricType::SearchResultCount);
        let cache_hit_count = number(&stats, MetricType::SearchCacheHitCount);
        let cache_miss_count = number(&stats, MetricType::SearchCacheMissCount);
        let vector_queries = number(&stats, MetricType::VectorSearchOps);
        let vector_errors = number(&stats, MetricType::VectorSearchErrors);
        let vector_latency_ms = number(&stats, MetricType::VectorSearchLatencyMs);
        let vector_upserts = number(&stats, MetricType::VectorUpsertOps);
        let vector_upsert_errors = number(&stats, MetricType::VectorUpsertErrors);
        let vector_deletes = number(&stats, MetricType::VectorDeleteOps);
        let vector_delete_errors = number(&stats, MetricType::VectorDeleteErrors);
        let vector_disabled_skips = number(&stats, MetricType::VectorDisabledSkips);
        let embedding_ops = number(&stats, MetricType::VectorEmbeddingOps);
        let embedding_errors = number(&stats, MetricType::VectorEmbeddingErrors);
        let embedding_latency_ms = number(&stats, MetricType::VectorEmbeddingLatencyMs);
        let embedding_prompt_tokens = number(&stats, MetricType::VectorEmbeddingPromptTokens);
        let embedding_total_tokens = number(&stats, MetricType::VectorEmbeddingTotalTokens);
        let rerank_ops = number(&stats, MetricType::VectorRerankOps);
        let rerank_errors = number(&stats, MetricType::VectorRerankErrors);
        let rerank_latency_ms = number(&stats, MetricType::VectorRerankLatencyMs);

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
        let hit_rate = if cache_hit_count + cache_miss_count > 0 {
            cache_hit_count as f64 / (cache_hit_count + cache_miss_count) as f64
        } else {
            0.0
        };

        let (avg_us, p50_us, p95_us, p99_us) = stats.get_search_latency_percentiles();
        let by_index: Vec<serde_json::Value> = stats
            .search_index_breakdown()
            .into_iter()
            .map(|b| {
                serde_json::json!({
                    "index": b.index,
                    "search_queries": b.search_queries,
                    "search_errors": b.search_errors,
                    "search_latency_ms": b.search_latency_ms,
                    "avg_search_latency_ms": b.avg_search_latency_ms,
                    "index_operations": b.index_operations,
                    "index_errors": b.index_errors,
                    "index_latency_ms": b.index_latency_ms,
                })
            })
            .collect();

        serde_json::json!({
            "search": {
                "total_queries": num_search_queries,
                "total_errors": num_search_errors,
                "total_latency_ms": search_latency_ms,
                "avg_latency_ms": avg_search_latency_ms,
                "total_results": search_result_count,
                "latency_percentiles_us": {
                    "avg": avg_us,
                    "p50": p50_us,
                    "p95": p95_us,
                    "p99": p99_us,
                },
            },
            "index": {
                "total_operations": num_index_operations,
                "total_errors": num_index_errors,
                "total_latency_ms": index_latency_ms,
                "avg_latency_ms": avg_index_latency_ms,
            },
            "delete": {
                "total_operations": num_delete_operations,
                "total_errors": num_delete_errors,
                "total_latency_ms": delete_latency_ms,
                "avg_latency_ms": avg_delete_latency_ms,
            },
            "cache": {
                "hit_count": cache_hit_count,
                "miss_count": cache_miss_count,
                "hit_rate": hit_rate,
            },
            "by_index": by_index,
            "vector": {
                "total_queries": vector_queries,
                "total_errors": vector_errors,
                "total_latency_ms": vector_latency_ms,
                "avg_latency_ms": avg_vector_latency_ms,
                "total_upserts": vector_upserts,
                "upsert_errors": vector_upsert_errors,
                "total_deletes": vector_deletes,
                "delete_errors": vector_delete_errors,
                "disabled_skips": vector_disabled_skips,
            },
            "embedding": {
                "total_operations": embedding_ops,
                "total_errors": embedding_errors,
                "total_latency_ms": embedding_latency_ms,
                "avg_latency_ms": avg_embedding_latency_ms,
                "prompt_tokens": embedding_prompt_tokens,
                "total_tokens": embedding_total_tokens,
            },
            "rerank": {
                "total_operations": rerank_ops,
                "total_errors": rerank_errors,
                "total_latency_ms": rerank_latency_ms,
                "avg_latency_ms": avg_rerank_latency_ms,
            },
        })
    }

    /// Background freeze statistics.
    ///
    /// Embedded equivalent of `GET /v1/statistics/freeze`; returns the
    /// `enabled: false` shape when no freeze manager is configured.
    pub fn freeze_statistics(&self) -> serde_json::Value
    where
        S: StorageSnapshotOps,
    {
        match self.storage().get_freeze_stats() {
            Some(stats) => serde_json::json!({
                "freeze_count": stats.freeze_count,
                "total_frozen_edges": stats.total_frozen_edges,
                "last_freeze_duration_ms": stats.last_freeze_duration_ms,
                "current_delta_edges": stats.current_delta_edges,
            }),
            None => serde_json::json!({
                "enabled": false,
                "message": "Background freeze manager not configured",
            }),
        }
    }

    /// Single query portrait by trace id.
    ///
    /// Embedded equivalent of `GET /v1/statistics/queries/{trace_id}`.
    pub fn query_profile(&self, trace_id: &str) -> Option<serde_json::Value> {
        self.stats_manager()
            .get_query_profile(trace_id)
            .map(|profile| profile_json(&profile, true))
    }

    /// Slow queries ordered by recency, capped at `limit`.
    ///
    /// Embedded equivalent of the `slow_queries` array in
    /// `GET /v1/statistics/queries`.
    pub fn slow_queries(&self, limit: usize) -> Vec<serde_json::Value> {
        let stats = self.stats_manager();
        let threshold = stats.slow_query_threshold_us();
        stats
            .profiles_in_window(None, None, limit)
            .into_iter()
            .filter(|p| p.total_duration_us >= threshold)
            .take(limit)
            .map(|profile| profile_json(&profile, false))
            .collect()
    }
}

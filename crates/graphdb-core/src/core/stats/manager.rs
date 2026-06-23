//! Statistics Manager
//!
//! Provides unified management of query metrics, query portraits and error statistics.

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::aggregated_stats::AggregatedStatsManager;
use super::error_stats::{ErrorInfo, ErrorStatsManager, ErrorType, QueryPhase};
use super::latency_histogram::LatencyHistogram;
use super::metrics::QueryMetrics;
use super::profile::QueryProfile;
use super::slow_query_logger::{SlowQueryConfig, SlowQueryLogger};
use super::utils::micros_to_millis;

/// Space metrics type alias
type SpaceMetrics = Arc<DashMap<MetricType, Arc<MetricValue>>>;

/// Type of indicator
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetricType {
    NumAuthFailedSessions,
    NumQueries,
    NumActiveQueries,
    QueryParseTimeUs,
    QueryValidateTimeUs,
    QueryPlanTimeUs,
    QueryOptimizeTimeUs,
    QueryExecuteTimeUs,
    QueryTotalTimeUs,
    QueryPlanNodeCount,
    QueryResultRowCount,
    // Query Type Statistics
    NumMatchQueries,
    NumCreateQueries,
    NumUpdateQueries,
    NumDeleteQueries,
    NumInsertQueries,
    NumGoQueries,
    NumFetchQueries,
    NumLookupQueries,
    NumShowQueries,
    // Search metrics
    NumSearchQueries,
    NumSearchErrors,
    SearchLatencyMs,
    NumIndexOperations,
    NumIndexErrors,
    IndexLatencyMs,
    NumDeleteOperations,
    NumDeleteErrors,
    DeleteLatencyMs,
    SearchResultCount,
    SearchCacheHitCount,
    SearchCacheMissCount,
    // Classified search errors
    SearchErrorIndexNotFound,
    SearchErrorEngineError,
    SearchErrorIoError,
    SearchErrorSerialization,
    SearchErrorInternal,
    // Storage metrics
    StorageReadOps,
    StorageWriteOps,
    StorageReadLatencyUs,
    StorageWriteLatencyUs,
    StorageErrors,
    StorageCacheHitCount,
    StorageCacheMissCount,
    // Transaction metrics
    TxnBeginCount,
    TxnCommitCount,
    TxnRollbackCount,
    TxnActiveCount,
    TxnConflictCount,
    // Sync metrics
    SyncOperations,
    SyncLatencyMs,
    SyncErrors,
    SyncQueueDepth,
    // Index metrics
    IndexScanCount,
    IndexLookupLatencyUs,
    IndexMemoryUsage,
    IndexWriteOps,
    IndexWriteLatencyUs,
    // Vector metrics
    VectorSearchOps,
    VectorSearchErrors,
    VectorSearchLatencyMs,
    VectorUpsertOps,
    VectorUpsertErrors,
    VectorUpsertLatencyMs,
    VectorDeleteOps,
    VectorDeleteErrors,
    VectorDeleteLatencyMs,
    VectorBufferFlushOps,
    VectorBufferFlushLatencyMs,
    VectorEmbeddingOps,
    VectorEmbeddingErrors,
    VectorEmbeddingLatencyMs,
    // CSR (Compressed Sparse Row) metrics
    CsrInsertions,
    CsrDeletions,
    CsrOverflowExpansions,
    CsrCompactions,
    CsrEdgesCompacted,
    CsrBytesAllocated,
    // MVCC Tombstone metrics
    TombstoneCount,
    TombstoneMemoryBytes,
    TombstoneGCCount,
    TombstoneOldestTsMin,
    TombstoneNewestTsMax,
    TombstoneActiveSnapshots,
    // Write Backpressure metrics (Mutable CSR monitoring)
    MutableCsrBytes,
    MutableCsrFreezeCount,
    MutableCsrPeakBytes,
}

/// metric
#[derive(Debug)]
pub struct MetricValue {
    pub value: AtomicU64,
    pub timestamp: AtomicU64,
}

impl MetricValue {
    pub fn new(value: u64) -> Self {
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            value: AtomicU64::new(value),
            timestamp: AtomicU64::new(timestamp_secs),
        }
    }

    pub fn increment(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
        self.update_timestamp();
    }

    pub fn add(&self, amount: u64) {
        self.value.fetch_add(amount, Ordering::Relaxed);
        self.update_timestamp();
    }

    pub fn decrement(&self) {
        let _ = self
            .value
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                if v > 0 {
                    Some(v - 1)
                } else {
                    Some(0)
                }
            });
        self.update_timestamp();
    }

    pub fn set(&self, value: u64) {
        self.value.store(value, Ordering::Relaxed);
        self.update_timestamp();
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn get_timestamp(&self) -> u64 {
        self.timestamp.load(Ordering::Relaxed)
    }

    fn update_timestamp(&self) {
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.timestamp.store(timestamp_secs, Ordering::Relaxed);
    }
}

/// Statistics Manager
///
/// Unified management of query metrics, query profiling and error statistics.
#[derive(Debug)]
pub struct StatsManager {
    metrics: Arc<DashMap<MetricType, Arc<MetricValue>>>,
    space_metrics: Arc<DashMap<String, SpaceMetrics>>,
    index_metrics: Arc<DashMap<String, SpaceMetrics>>,
    last_query_metrics: Arc<RwLock<Option<QueryMetrics>>>,
    query_profiles: Arc<RwLock<VecDeque<QueryProfile>>>,
    query_latency_histogram: Arc<RwLock<LatencyHistogram>>,
    search_latency_histogram: Arc<RwLock<LatencyHistogram>>,
    monitoring_enabled: bool,
    profile_cache_size: usize,
    slow_query_threshold_us: u64,
    error_stats: ErrorStatsManager,
    slow_query_logger: Option<Arc<SlowQueryLogger>>,
    aggregated_stats: AggregatedStatsManager,
}

impl StatsManager {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(DashMap::new()),
            space_metrics: Arc::new(DashMap::new()),
            index_metrics: Arc::new(DashMap::new()),
            last_query_metrics: Arc::new(RwLock::new(None)),
            query_profiles: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            query_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            search_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            monitoring_enabled: true,
            profile_cache_size: 1000,
            slow_query_threshold_us: 1_000_000,
            error_stats: ErrorStatsManager::new(),
            slow_query_logger: None,
            aggregated_stats: AggregatedStatsManager::new(),
        }
    }

    pub fn with_config(
        monitoring_enabled: bool,
        profile_cache_size: usize,
        slow_query_threshold_us: u64,
    ) -> Self {
        Self {
            metrics: Arc::new(DashMap::new()),
            space_metrics: Arc::new(DashMap::new()),
            index_metrics: Arc::new(DashMap::new()),
            last_query_metrics: Arc::new(RwLock::new(None)),
            query_profiles: Arc::new(RwLock::new(VecDeque::with_capacity(profile_cache_size))),
            query_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            search_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            monitoring_enabled,
            profile_cache_size,
            slow_query_threshold_us,
            error_stats: ErrorStatsManager::new(),
            slow_query_logger: None,
            aggregated_stats: AggregatedStatsManager::new(),
        }
    }

    /// Create StatsManager with slow query logger
    pub fn with_slow_query_logger(
        monitoring_enabled: bool,
        profile_cache_size: usize,
        slow_query_threshold_us: u64,
        slow_query_config: SlowQueryConfig,
    ) -> Result<Self, std::io::Error> {
        let logger = Arc::new(SlowQueryLogger::new(slow_query_config)?);

        Ok(Self {
            metrics: Arc::new(DashMap::new()),
            space_metrics: Arc::new(DashMap::new()),
            index_metrics: Arc::new(DashMap::new()),
            last_query_metrics: Arc::new(RwLock::new(None)),
            query_profiles: Arc::new(RwLock::new(VecDeque::with_capacity(profile_cache_size))),
            query_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            search_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            monitoring_enabled,
            profile_cache_size,
            slow_query_threshold_us,
            error_stats: ErrorStatsManager::new(),
            slow_query_logger: Some(logger),
            aggregated_stats: AggregatedStatsManager::new(),
        })
    }

    pub fn record_query_profile(&self, profile: QueryProfile) {
        if !self.monitoring_enabled {
            return;
        }

        if profile.total_duration_us >= self.slow_query_threshold_us {
            self.write_slow_query_log(&profile);
        }

        let mut profiles = self.query_profiles.write();
        if profiles.len() >= self.profile_cache_size {
            profiles.pop_front();
        }
        profiles.push_back(profile);
    }

    fn write_slow_query_log(&self, profile: &QueryProfile) {
        // Record to aggregated stats
        self.aggregated_stats.record_query(profile, true);

        if let Some(ref logger) = self.slow_query_logger {
            logger.log(profile);
        } else {
            self.write_slow_query_log_fallback(profile);
        }
    }

    fn write_slow_query_log_fallback(&self, profile: &QueryProfile) {
        let executor_summary: Vec<String> = profile
            .executor_stats
            .iter()
            .map(|stat| {
                format!(
                    "{}[id={}, {}ms, rows={}, mem={}]",
                    stat.executor_type,
                    stat.executor_id,
                    stat.duration_ms(),
                    stat.rows_processed(),
                    stat.memory_used()
                )
            })
            .collect();

        let error_str = if let Some(ref info) = profile.error_info {
            format!(
                " [error={} phase={}]: {}",
                info.error_type, info.error_phase, info.error_message
            )
        } else if let Some(ref msg) = profile.error_message {
            format!(" [error]: {}", msg)
        } else {
            String::new()
        };

        log::warn!(
            "Slow query [trace_id={}] [session_id={}] [duration={}ms] [status={}]\n\
Queries: {}\n\
Stage statistics: parse={}ms validate={}ms plan={}ms optimize={}ms execute={}ms\n\
Number of results: {} Number of executors: {} Total executor time: {}ms\n\\\
Executor details: {}{}",
            profile.trace_id,
            profile.session_id,
            micros_to_millis(profile.total_duration_us),
            match profile.status {
                super::profile::QueryStatus::Success => "success",
                super::profile::QueryStatus::Failed => "failed",
            },
            profile.query_text,
            profile.stages.parse_ms(),
            profile.stages.validate_ms(),
            profile.stages.plan_ms(),
            profile.stages.optimize_ms(),
            profile.stages.execute_ms(),
            profile.result_count,
            profile.executor_stats.len(),
            profile.total_executor_time_ms(),
            executor_summary.join(", "),
            error_str
        );
    }

    pub fn get_recent_queries(&self, limit: usize) -> Vec<QueryProfile> {
        let profiles = self.query_profiles.write();
        profiles.iter().rev().take(limit).cloned().collect()
    }

    pub fn get_slow_queries(&self, limit: usize) -> Vec<QueryProfile> {
        let profiles = self.query_profiles.write();
        profiles
            .iter()
            .filter(|p| p.total_duration_us >= self.slow_query_threshold_us)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn get_all_slow_queries(&self, limit: usize) -> SlowQueryStats {
        let profiles = self.query_profiles.write();
        let slow_queries: Vec<_> = profiles
            .iter()
            .filter(|p| p.total_duration_us >= self.slow_query_threshold_us)
            .take(limit)
            .cloned()
            .collect();

        let total = slow_queries.len() as u64;
        let durations: Vec<f64> = slow_queries
            .iter()
            .map(|p| p.total_duration_us as f64 / 1_000_000.0)
            .collect();

        let (min, max, avg) = if durations.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            let min = durations.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = durations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let avg = durations.iter().sum::<f64>() / durations.len() as f64;
            (min, max, avg)
        };

        SlowQueryStats {
            total,
            min_duration_secs: min,
            max_duration_secs: max,
            avg_duration_secs: avg,
        }
    }
}

/// Slow query statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowQueryStats {
    pub total: u64,
    pub min_duration_secs: f64,
    pub max_duration_secs: f64,
    pub avg_duration_secs: f64,
}

impl StatsManager {
    pub fn get_query_profile(&self, trace_id: &str) -> Option<QueryProfile> {
        let profiles = self.query_profiles.write();
        profiles.iter().find(|p| p.trace_id == trace_id).cloned()
    }

    pub fn get_session_queries(&self, session_id: i64, limit: usize) -> Vec<QueryProfile> {
        let profiles = self.query_profiles.write();
        profiles
            .iter()
            .filter(|p| p.session_id == session_id)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn get_executor_stats_summary(&self) -> HashMap<String, (u64, u64, usize)> {
        let profiles = self.query_profiles.write();
        let mut stats: HashMap<String, (u64, u64, usize)> = HashMap::new();

        for profile in profiles.iter() {
            for exec_stat in &profile.executor_stats {
                let entry = stats
                    .entry(exec_stat.executor_type.clone())
                    .or_insert((0, 0, 0));
                entry.0 += exec_stat.duration_ms() as u64;
                entry.1 += exec_stat.rows_processed() as u64;
                entry.2 += 1;
            }
        }

        stats
    }

    pub fn clear_query_cache(&self) {
        let mut profiles = self.query_profiles.write();
        profiles.clear();
    }

    pub fn query_cache_size(&self) -> usize {
        let profiles = self.query_profiles.write();
        profiles.len()
    }

    pub fn add_value(&self, metric_type: MetricType) {
        let metric = self
            .metrics
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.increment();
    }

    pub fn add_value_with_amount(&self, metric_type: MetricType, amount: u64) {
        let metric = self
            .metrics
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.add(amount);
    }

    pub fn dec_value(&self, metric_type: MetricType) {
        if let Some(metric) = self.metrics.get(&metric_type) {
            metric.decrement();
        }
    }

    pub fn set_value(&self, metric_type: MetricType, value: u64) {
        let metric = self
            .metrics
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.set(value);
    }

    pub fn add_space_metric(&self, space_name: &str, metric_type: MetricType) {
        let space_map = self
            .space_metrics
            .entry(space_name.to_string())
            .or_insert_with(|| Arc::new(DashMap::new()));
        let metric = space_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.increment();
    }

    pub fn add_space_metric_with_amount(
        &self,
        space_name: &str,
        metric_type: MetricType,
        amount: u64,
    ) {
        let space_map = self
            .space_metrics
            .entry(space_name.to_string())
            .or_insert_with(|| Arc::new(DashMap::new()));
        let metric = space_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.add(amount);
    }

    pub fn add_index_metric(&self, index_name: &str, metric_type: MetricType) {
        let index_map = self
            .index_metrics
            .entry(index_name.to_string())
            .or_insert_with(|| Arc::new(DashMap::new()));
        let metric = index_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.increment();
    }

    pub fn add_index_metric_with_amount(
        &self,
        index_name: &str,
        metric_type: MetricType,
        amount: u64,
    ) {
        let index_map = self
            .index_metrics
            .entry(index_name.to_string())
            .or_insert_with(|| Arc::new(DashMap::new()));
        let metric = index_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.add(amount);
    }

    pub fn dec_space_metric(&self, space_name: &str, metric_type: MetricType) {
        if let Some(space_map) = self.space_metrics.get(space_name) {
            if let Some(metric) = space_map.get(&metric_type) {
                metric.decrement();
            }
        }
    }

    pub fn get_value(&self, metric_type: MetricType) -> Option<u64> {
        self.metrics.get(&metric_type).map(|metric| metric.get())
    }

    pub fn get_space_value(&self, space_name: &str, metric_type: MetricType) -> Option<u64> {
        self.space_metrics
            .get(space_name)
            .and_then(|space_map| space_map.get(&metric_type).map(|metric| metric.get()))
    }

    pub fn get_index_value(&self, index_name: &str, metric_type: MetricType) -> Option<u64> {
        self.index_metrics
            .get(index_name)
            .and_then(|index_map| index_map.get(&metric_type).map(|metric| metric.get()))
    }

    pub fn get_all_index_metrics(&self, index_name: &str) -> Option<HashMap<MetricType, u64>> {
        self.index_metrics.get(index_name).map(|index_map| {
            index_map
                .iter()
                .map(|entry| (*entry.key(), entry.value().get()))
                .collect()
        })
    }

    pub fn get_all_metrics(&self) -> HashMap<MetricType, u64> {
        self.metrics
            .iter()
            .map(|entry| (*entry.key(), entry.value().get()))
            .collect()
    }

    pub fn get_all_space_metrics(&self, space_name: &str) -> Option<HashMap<MetricType, u64>> {
        self.space_metrics.get(space_name).map(|space_map| {
            space_map
                .iter()
                .map(|entry| (*entry.key(), entry.value().get()))
                .collect()
        })
    }

    pub fn reset_metric(&self, metric_type: MetricType) {
        if let Some(metric) = self.metrics.get(&metric_type) {
            metric.set(0);
        }
    }

    pub fn reset_all_metrics(&self) {
        for metric in self.metrics.iter() {
            metric.value().set(0);
        }
    }

    pub fn reset_space_metrics(&self, space_name: &str) {
        if let Some(space_map) = self.space_metrics.get(space_name) {
            for metric in space_map.iter() {
                metric.value().set(0);
            }
        }
    }

    pub fn record_error(&self, error_type: ErrorType, phase: QueryPhase) {
        self.error_stats.record_error(error_type, phase);
    }

    pub fn get_error_count(&self, error_type: ErrorType) -> u64 {
        self.error_stats.get_error_count(error_type)
    }

    pub fn get_error_count_by_phase(&self, phase: QueryPhase) -> u64 {
        self.error_stats.get_error_count_by_phase(phase)
    }

    pub fn get_all_error_counts(&self) -> HashMap<ErrorType, u64> {
        self.error_stats.get_all_error_counts()
    }

    pub fn get_all_error_counts_by_phase(&self) -> HashMap<QueryPhase, u64> {
        self.error_stats.get_all_error_counts_by_phase()
    }

    pub fn reset_error_counts(&self) {
        self.error_stats.reset_error_counts();
    }

    pub fn record_failed_query(&self, mut profile: QueryProfile, error_info: ErrorInfo) {
        profile.mark_failed_with_info(error_info.clone());
        self.record_error(error_info.error_type, error_info.error_phase);
        self.record_query_profile(profile);
    }

    pub fn get_error_summary(&self) -> super::error_stats::ErrorSummary {
        self.error_stats.get_error_summary()
    }

    pub fn get_recent_errors(&self, limit: usize) -> Vec<super::error_stats::RecentError> {
        self.error_stats.get_recent_errors(limit)
    }

    pub fn record_query_metrics(&self, metrics: &QueryMetrics) {
        let mut last_metrics = self.last_query_metrics.write();
        *last_metrics = Some(metrics.clone());
        drop(last_metrics);

        // Record latency histogram
        {
            let mut histogram = self.query_latency_histogram.write();
            histogram.record_micros(metrics.total_time_us);
        }

        let updates = [
            (MetricType::QueryParseTimeUs, metrics.parse_time_us),
            (MetricType::QueryValidateTimeUs, metrics.validate_time_us),
            (MetricType::QueryPlanTimeUs, metrics.plan_time_us),
            (MetricType::QueryOptimizeTimeUs, metrics.optimize_time_us),
            (MetricType::QueryExecuteTimeUs, metrics.execute_time_us),
            (MetricType::QueryTotalTimeUs, metrics.total_time_us),
            (
                MetricType::QueryPlanNodeCount,
                metrics.plan_node_count as u64,
            ),
            (
                MetricType::QueryResultRowCount,
                metrics.result_row_count as u64,
            ),
        ];

        for (metric_type, value) in updates {
            let metric = self
                .metrics
                .entry(metric_type)
                .or_insert_with(|| Arc::new(MetricValue::new(0)));
            metric.set(value);
        }
    }

    /// Get latency percentiles (avg, p50, p95, p99) in microseconds
    pub fn get_latency_percentiles(&self) -> (u64, u64, u64, u64) {
        let histogram = self.query_latency_histogram.write();
        (
            histogram.avg(),
            histogram.p50(),
            histogram.p95(),
            histogram.p99(),
        )
    }

    /// Get search latency percentiles (avg, p50, p95, p99) in microseconds
    pub fn get_search_latency_percentiles(&self) -> (u64, u64, u64, u64) {
        let histogram = self.search_latency_histogram.write();
        (
            histogram.avg(),
            histogram.p50(),
            histogram.p95(),
            histogram.p99(),
        )
    }

    /// Get latency histogram report
    pub fn get_latency_report(&self) -> String {
        let histogram = self.query_latency_histogram.write();
        histogram.report()
    }

    /// Clear latency histogram
    pub fn clear_latency_histogram(&self) {
        let mut histogram = self.query_latency_histogram.write();
        histogram.clear();
    }

    pub fn get_last_query_metrics(&self) -> Option<QueryMetrics> {
        let last_metrics = self.last_query_metrics.write();
        last_metrics.clone()
    }

    pub fn get_query_metrics(&self) -> Option<QueryMetrics> {
        self.get_last_query_metrics()
    }

    /// Record aggregated query statistics
    pub fn record_aggregated_query(&self, profile: &QueryProfile, is_slow: bool) {
        self.aggregated_stats.record_query(profile, is_slow);
    }

    /// Get top N slow query patterns by average duration
    pub fn get_top_n_slow_query_patterns(
        &self,
        limit: usize,
    ) -> Vec<super::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_top_n_slow_queries(limit)
    }

    /// Get top N slow query patterns by total duration
    pub fn get_top_n_patterns_by_total_duration(
        &self,
        limit: usize,
    ) -> Vec<super::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_top_n_by_total_duration(limit)
    }

    /// Get top N slow query patterns by execution count
    pub fn get_top_n_patterns_by_execution_count(
        &self,
        limit: usize,
    ) -> Vec<super::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_top_n_by_execution_count(limit)
    }

    /// Get statistics for a specific query pattern
    pub fn get_pattern_stats(
        &self,
        normalized_query: &str,
    ) -> Option<super::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_pattern_stats(normalized_query)
    }

    /// Get all aggregated statistics
    pub fn get_all_aggregated_stats(&self) -> Vec<super::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_all_stats()
    }

    /// Get total number of queries processed
    pub fn get_total_aggregated_queries(&self) -> u64 {
        self.aggregated_stats.get_total_queries()
    }

    /// Get total number of slow queries
    pub fn get_total_aggregated_slow_queries(&self) -> u64 {
        self.aggregated_stats.get_total_slow_queries()
    }

    /// Get current queries per second
    pub fn get_current_qps(&self) -> u64 {
        self.aggregated_stats.get_current_qps()
    }

    /// Get number of query patterns
    pub fn get_pattern_count(&self) -> usize {
        self.aggregated_stats.get_pattern_count()
    }

    /// Clear all aggregated statistics
    pub fn clear_aggregated_stats(&self) {
        self.aggregated_stats.clear();
    }

    /// Cleanup expired aggregated statistics
    pub fn cleanup_aggregated_stats(&self) {
        self.aggregated_stats.cleanup();
    }

    // ============================================================================
    // Search Metrics
    // ============================================================================

    /// Record a search query operation
    pub fn record_search(&self, space_id: u64, index_name: &str, latency_ms: u64, success: bool) {
        let space_key = format!("space_{}", space_id);
        self.add_value(MetricType::NumSearchQueries);
        self.add_space_metric(&space_key, MetricType::NumSearchQueries);
        self.add_index_metric(index_name, MetricType::NumSearchQueries);

        if !success {
            self.add_value(MetricType::NumSearchErrors);
            self.add_space_metric(&space_key, MetricType::NumSearchErrors);
            self.add_index_metric(index_name, MetricType::NumSearchErrors);
        }

        self.add_value_with_amount(MetricType::SearchLatencyMs, latency_ms);
        self.add_space_metric_with_amount(&space_key, MetricType::SearchLatencyMs, latency_ms);
        self.add_index_metric_with_amount(index_name, MetricType::SearchLatencyMs, latency_ms);

        {
            let mut histogram = self.search_latency_histogram.write();
            histogram.record_micros(latency_ms * 1000);
        }
    }

    /// Record an index operation
    pub fn record_index_operation(
        &self,
        space_id: u64,
        index_name: &str,
        latency_ms: u64,
        success: bool,
    ) {
        let space_key = format!("space_{}", space_id);
        self.add_value(MetricType::NumIndexOperations);
        self.add_space_metric(&space_key, MetricType::NumIndexOperations);
        self.add_index_metric(index_name, MetricType::NumIndexOperations);

        if !success {
            self.add_value(MetricType::NumIndexErrors);
            self.add_space_metric(&space_key, MetricType::NumIndexErrors);
            self.add_index_metric(index_name, MetricType::NumIndexErrors);
        }

        self.add_value_with_amount(MetricType::IndexLatencyMs, latency_ms);
        self.add_space_metric_with_amount(&space_key, MetricType::IndexLatencyMs, latency_ms);
        self.add_index_metric_with_amount(index_name, MetricType::IndexLatencyMs, latency_ms);
    }

    /// Record a delete operation
    pub fn record_delete_operation(
        &self,
        space_id: u64,
        index_name: &str,
        latency_ms: u64,
        success: bool,
    ) {
        let space_key = format!("space_{}", space_id);
        self.add_value(MetricType::NumDeleteOperations);
        self.add_space_metric(&space_key, MetricType::NumDeleteOperations);
        self.add_index_metric(index_name, MetricType::NumDeleteOperations);

        if !success {
            self.add_value(MetricType::NumDeleteErrors);
            self.add_space_metric(&space_key, MetricType::NumDeleteErrors);
            self.add_index_metric(index_name, MetricType::NumDeleteErrors);
        }

        self.add_value_with_amount(MetricType::DeleteLatencyMs, latency_ms);
        self.add_space_metric_with_amount(&space_key, MetricType::DeleteLatencyMs, latency_ms);
        self.add_index_metric_with_amount(index_name, MetricType::DeleteLatencyMs, latency_ms);
    }

    /// Record search result count
    pub fn record_search_result_count(&self, space_id: u64, count: u64) {
        let space_key = format!("space_{}", space_id);
        self.add_value_with_amount(MetricType::SearchResultCount, count);
        self.add_space_metric_with_amount(&space_key, MetricType::SearchResultCount, count);
    }

    /// Record cache hit or miss
    pub fn record_cache_hit(&self, space_id: u64, hit: bool) {
        let space_key = format!("space_{}", space_id);
        if hit {
            self.add_value(MetricType::SearchCacheHitCount);
            self.add_space_metric(&space_key, MetricType::SearchCacheHitCount);
        } else {
            self.add_value(MetricType::SearchCacheMissCount);
            self.add_space_metric(&space_key, MetricType::SearchCacheMissCount);
        }
    }

    // ========== Storage Metrics ==========

    /// Record a storage read operation
    pub fn record_storage_read(&self, latency_us: u64, success: bool) {
        self.add_value(MetricType::StorageReadOps);
        self.add_value_with_amount(MetricType::StorageReadLatencyUs, latency_us);
        if !success {
            self.add_value(MetricType::StorageErrors);
        }
    }

    /// Record a storage write operation
    pub fn record_storage_write(&self, latency_us: u64, success: bool) {
        self.add_value(MetricType::StorageWriteOps);
        self.add_value_with_amount(MetricType::StorageWriteLatencyUs, latency_us);
        if !success {
            self.add_value(MetricType::StorageErrors);
        }
    }

    /// Record storage cache hit or miss
    pub fn record_storage_cache_hit(&self, hit: bool) {
        if hit {
            self.add_value(MetricType::StorageCacheHitCount);
        } else {
            self.add_value(MetricType::StorageCacheMissCount);
        }
    }

    // ========== Transaction Metrics ==========

    /// Record transaction begin
    pub fn record_txn_begin(&self) {
        self.add_value(MetricType::TxnBeginCount);
        self.add_value(MetricType::TxnActiveCount);
    }

    /// Record transaction commit
    pub fn record_txn_commit(&self) {
        self.add_value(MetricType::TxnCommitCount);
        self.dec_value(MetricType::TxnActiveCount);
    }

    /// Record transaction rollback
    pub fn record_txn_rollback(&self) {
        self.add_value(MetricType::TxnRollbackCount);
        self.dec_value(MetricType::TxnActiveCount);
    }

    /// Record transaction conflict
    pub fn record_txn_conflict(&self) {
        self.add_value(MetricType::TxnConflictCount);
    }

    pub fn record_sync_operation(&self, latency_ms: u64, success: bool) {
        self.add_value(MetricType::SyncOperations);
        self.add_value_with_amount(MetricType::SyncLatencyMs, latency_ms);
        if !success {
            self.add_value(MetricType::SyncErrors);
        }
    }

    pub fn record_sync_error(&self) {
        self.add_value(MetricType::SyncErrors);
    }

    pub fn set_sync_queue_depth(&self, depth: u64) {
        self.set_value(MetricType::SyncQueueDepth, depth);
    }

    pub fn record_index_scan(&self, latency_us: u64) {
        self.add_value(MetricType::IndexScanCount);
        self.add_value_with_amount(MetricType::IndexLookupLatencyUs, latency_us);
    }

    pub fn record_index_write(&self, latency_us: u64) {
        self.add_value(MetricType::IndexWriteOps);
        self.add_value_with_amount(MetricType::IndexWriteLatencyUs, latency_us);
    }

    pub fn set_index_memory_usage(&self, bytes: u64) {
        self.set_value(MetricType::IndexMemoryUsage, bytes);
    }

    // ========== CSR (Compressed Sparse Row) Metrics ==========

    /// Record a CSR edge insertion
    pub fn record_csr_insertion(&self) {
        self.add_value(MetricType::CsrInsertions);
    }

    /// Record a CSR edge deletion
    pub fn record_csr_deletion(&self) {
        self.add_value(MetricType::CsrDeletions);
    }

    /// Record a CSR overflow expansion (vertex capacity increase)
    pub fn record_csr_overflow_expansion(&self) {
        self.add_value(MetricType::CsrOverflowExpansions);
    }

    /// Record a CSR compaction operation
    /// edges_removed: number of deleted edges actually removed
    pub fn record_csr_compaction(&self, edges_removed: u64) {
        self.add_value(MetricType::CsrCompactions);
        self.add_value_with_amount(MetricType::CsrEdgesCompacted, edges_removed);
    }

    /// Record CSR memory allocation
    pub fn record_csr_allocation(&self, bytes: u64) {
        self.add_value_with_amount(MetricType::CsrBytesAllocated, bytes);
    }

    /// Set current CSR allocated bytes (snapshot of current state)
    pub fn set_csr_bytes_allocated(&self, bytes: u64) {
        self.set_value(MetricType::CsrBytesAllocated, bytes);
    }

    /// Record tombstone statistics for MVCC observability
    pub fn record_tombstone_stats(
        &self,
        count: u64,
        memory_bytes: u64,
        oldest_ts_min: Option<u32>,
        newest_ts_max: Option<u32>,
        active_snapshots: u64,
    ) {
        self.set_value(MetricType::TombstoneCount, count);
        self.set_value(MetricType::TombstoneMemoryBytes, memory_bytes);
        self.set_value(MetricType::TombstoneActiveSnapshots, active_snapshots);

        if let Some(ts) = oldest_ts_min {
            self.set_value(MetricType::TombstoneOldestTsMin, ts as u64);
        }
        if let Some(ts) = newest_ts_max {
            self.set_value(MetricType::TombstoneNewestTsMax, ts as u64);
        }
    }

    /// Increment tombstone garbage collection counter
    pub fn record_tombstone_gc(&self) {
        self.add_value(MetricType::TombstoneGCCount);
    }

    /// Record mutable CSR backpressure metrics
    pub fn record_mutable_csr_backpressure(&self, current_bytes: u64, peak_bytes: u64) {
        self.set_value(MetricType::MutableCsrBytes, current_bytes);
        // Update peak if current exceeds previous peak
        if let Some(prev_peak) = self.get_value(MetricType::MutableCsrPeakBytes) {
            if current_bytes > prev_peak {
                self.set_value(MetricType::MutableCsrPeakBytes, current_bytes);
            }
        } else {
            self.set_value(MetricType::MutableCsrPeakBytes, peak_bytes);
        }
    }

    /// Increment mutable CSR freeze counter
    pub fn record_mutable_csr_freeze(&self) {
        self.add_value(MetricType::MutableCsrFreezeCount);
    }
}

impl Default for StatsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_manager_creation() {
        let stats = StatsManager::new();
        assert_eq!(stats.get_value(MetricType::NumQueries), None);

        stats.add_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(1));
    }

    #[test]
    fn test_add_value() {
        let stats = StatsManager::new();
        stats.add_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(1));

        stats.add_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(2));
    }

    #[test]
    fn test_add_value_with_amount() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 5);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(5));

        stats.add_value_with_amount(MetricType::NumQueries, 3);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(8));
    }

    #[test]
    fn test_dec_value() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 10);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(10));

        stats.dec_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(9));

        stats.dec_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(8));
    }

    #[test]
    fn test_space_metrics() {
        let stats = StatsManager::new();
        stats.add_space_metric("test_space", MetricType::NumQueries);
        assert_eq!(
            stats.get_space_value("test_space", MetricType::NumQueries),
            Some(1)
        );

        stats.add_space_metric("test_space", MetricType::NumQueries);
        assert_eq!(
            stats.get_space_value("test_space", MetricType::NumQueries),
            Some(2)
        );

        stats.add_space_metric("other_space", MetricType::NumQueries);
        assert_eq!(
            stats.get_space_value("other_space", MetricType::NumQueries),
            Some(1)
        );
    }

    #[test]
    fn test_get_all_metrics() {
        let stats = StatsManager::new();
        stats.add_value(MetricType::NumQueries);
        stats.add_value(MetricType::NumActiveQueries);

        let all_metrics = stats.get_all_metrics();
        assert_eq!(all_metrics.get(&MetricType::NumQueries), Some(&1));
        assert_eq!(all_metrics.get(&MetricType::NumActiveQueries), Some(&1));
    }

    #[test]
    fn test_reset_metric() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 10);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(10));

        stats.reset_metric(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(0));
    }

    #[test]
    fn test_reset_all_metrics() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 10);
        stats.add_value_with_amount(MetricType::NumActiveQueries, 3);

        stats.reset_all_metrics();

        assert_eq!(stats.get_value(MetricType::NumQueries), Some(0));
        assert_eq!(stats.get_value(MetricType::NumActiveQueries), Some(0));
    }

    #[test]
    fn test_record_and_get_query_profile() {
        let stats = StatsManager::with_config(true, 10, 1_000_000);

        let mut profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        profile.total_duration_us = 500;
        profile.result_count = 10;

        stats.record_query_profile(profile.clone());

        assert_eq!(stats.query_cache_size(), 1);

        let recent = stats.get_recent_queries(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].session_id, 123);
    }

    #[test]
    fn test_get_slow_queries() {
        let stats = StatsManager::with_config(true, 10, 1_000_000);

        let mut slow_profile = QueryProfile::new(1, "MATCH (n) RETURN n".to_string());
        slow_profile.total_duration_us = 2_000_000; // 2000ms in microseconds
        stats.record_query_profile(slow_profile);

        let mut fast_profile = QueryProfile::new(2, "MATCH (n) RETURN n LIMIT 1".to_string());
        fast_profile.total_duration_us = 100_000; // 100ms in microseconds
        stats.record_query_profile(fast_profile);

        let slow_queries = stats.get_slow_queries(10);
        assert_eq!(slow_queries.len(), 1);
        assert_eq!(slow_queries[0].session_id, 1);
    }

    #[test]
    fn test_query_cache_size_limit() {
        let stats = StatsManager::with_config(true, 3, 1_000_000);

        for i in 0..5 {
            let profile = QueryProfile::new(i as i64, format!("Query {}", i));
            stats.record_query_profile(profile);
        }

        assert_eq!(stats.query_cache_size(), 3);

        let recent = stats.get_recent_queries(3);
        assert_eq!(recent[0].session_id, 4);
        assert_eq!(recent[2].session_id, 2);
    }

    #[test]
    fn test_disabled_monitoring() {
        let stats = StatsManager::with_config(false, 10, 1_000_000);

        let profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        stats.record_query_profile(profile);

        assert_eq!(stats.query_cache_size(), 0);
    }

    #[test]
    fn test_aggregated_stats_integration() {
        let stats = StatsManager::new();

        // Create test profiles
        let mut profile1 =
            QueryProfile::new(1, "MATCH (n:Person) WHERE n.id = 1 RETURN n".to_string());
        profile1.total_duration_us = 1000;

        let mut profile2 =
            QueryProfile::new(2, "MATCH (n:Person) WHERE n.id = 2 RETURN n".to_string());
        profile2.total_duration_us = 2000;

        // Record queries
        stats.record_aggregated_query(&profile1, false);
        stats.record_aggregated_query(&profile2, false);

        // Verify stats
        assert_eq!(stats.get_total_aggregated_queries(), 2);
        assert_eq!(stats.get_pattern_count(), 1); // Same pattern
        assert_eq!(stats.get_current_qps(), 2);
    }

    #[test]
    fn test_slow_query_aggregated_recording() {
        let stats = StatsManager::with_config(true, 10, 1_000_000);

        // Create a slow query profile
        let mut profile =
            QueryProfile::new(1, "MATCH (n:Person) WHERE n.id = 1 RETURN n".to_string());
        profile.total_duration_us = 2_000_000; // 2000ms

        // This should trigger aggregated recording via write_slow_query_log
        stats.record_query_profile(profile.clone());

        // Verify aggregated stats were recorded
        assert_eq!(stats.get_total_aggregated_queries(), 1);
        assert_eq!(stats.get_total_aggregated_slow_queries(), 1);
    }

    #[test]
    fn test_get_top_n_slow_query_patterns() {
        let stats = StatsManager::new();

        // Record multiple queries with different patterns
        for i in 0..10 {
            let mut profile =
                QueryProfile::new(i, format!("MATCH (n:Person) WHERE n.id = {} RETURN n", i));
            profile.total_duration_us = 1000 + (i * 100) as u64;
            stats.record_aggregated_query(&profile, false);
        }

        // Get top 5 patterns
        let top_patterns = stats.get_top_n_slow_query_patterns(5);
        assert_eq!(top_patterns.len(), 1); // All same pattern
        assert_eq!(top_patterns[0].execution_count, 10);
    }

    #[test]
    fn test_clear_aggregated_stats() {
        let stats = StatsManager::new();

        let profile = QueryProfile::new(1, "MATCH (n) RETURN n".to_string());
        stats.record_aggregated_query(&profile, false);

        assert_eq!(stats.get_total_aggregated_queries(), 1);

        stats.clear_aggregated_stats();

        assert_eq!(stats.get_total_aggregated_queries(), 0);
        assert_eq!(stats.get_pattern_count(), 0);
    }
}

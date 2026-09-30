//! Lightweight numeric query metrics and latency histograms.
use std::sync::Arc;

use crate::metrics::QueryMetrics;

use super::core::StatsManager;
use super::metric_type::MetricType;
use super::metric_value::MetricValue;

impl StatsManager {
    pub fn record_query_metrics(&self, metrics: &QueryMetrics) {
        // Per-instance history instead of single-slot overwrite: concurrent
        // queries append under the lock so no instance is lost.
        {
            let mut last_metrics = self.last_query_metrics.write();
            *last_metrics = Some(metrics.clone());
        }
        {
            let mut history = self.query_metrics_history.write();
            if history.len() >= 1000 {
                history.pop_front();
            }
            history.push_back(metrics.clone());
        }

        // Record latency histogram
        {
            let mut histogram = self.query_latency_histogram.write();
            histogram.record_micros(metrics.total_time_us);
        }

        // Cumulative sums plus a query count so means are computable;
        // never overwrite with a single query's values.
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
            metric.add(value);
        }
        self.add_value(MetricType::NumQueries);
    }

    /// Get latency percentiles (avg, p50, p95, p99) in microseconds
    pub fn get_latency_percentiles(&self) -> (u64, u64, u64, u64) {
        let histogram = self.query_latency_histogram.read();
        (
            histogram.avg(),
            histogram.p50(),
            histogram.p95(),
            histogram.p99(),
        )
    }

    /// Get search latency percentiles (avg, p50, p95, p99) in microseconds
    pub fn get_search_latency_percentiles(&self) -> (u64, u64, u64, u64) {
        let histogram = self.search_latency_histogram.read();
        (
            histogram.avg(),
            histogram.p50(),
            histogram.p95(),
            histogram.p99(),
        )
    }

    /// Get latency histogram report
    pub fn get_latency_report(&self) -> String {
        let histogram = self.query_latency_histogram.read();
        histogram.report()
    }

    /// Clear latency histogram
    pub fn clear_latency_histogram(&self) {
        let mut histogram = self.query_latency_histogram.write();
        histogram.clear();
    }

    pub fn get_last_query_metrics(&self) -> Option<QueryMetrics> {
        let last_metrics = self.last_query_metrics.read();
        last_metrics.clone()
    }

    /// Recent per-instance query metrics (up to 1000), newest last.
    pub fn get_query_metrics_history(&self) -> Vec<QueryMetrics> {
        self.query_metrics_history.read().iter().cloned().collect()
    }

    pub fn get_query_metrics(&self) -> Option<QueryMetrics> {
        self.get_last_query_metrics()
    }
}

//! Aggregated pattern delegation to the dedicated pattern manager.
use serde::{Deserialize, Serialize};

use crate::profile::QueryProfile;

use super::core::StatsManager;

/// Serializable pattern entry for handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPatternSnapshot {
    pub normalized_query: String,
    pub query_type: String,
    pub labels: Vec<String>,
    pub execution_count: u64,
    pub avg_duration_ms: f64,
    pub p95_duration_ms: f64,
    pub p99_duration_ms: f64,
    pub error_rate: f64,
    pub error_count: u64,
}

impl StatsManager {
    /// Record aggregated query statistics
    pub fn record_aggregated_query(&self, profile: &QueryProfile, is_slow: bool) {
        self.aggregated_stats.record_query(profile, is_slow);
    }

    /// Get top N slow query patterns by average duration
    pub fn get_top_n_slow_query_patterns(
        &self,
        limit: usize,
    ) -> Vec<crate::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_top_n_slow_queries(limit)
    }

    pub fn pattern_snapshot(&self, limit: usize) -> Vec<QueryPatternSnapshot> {
        self.aggregated_stats
            .get_top_n_slow_queries(limit)
            .into_iter()
            .map(|s| {
                let avg_duration_ms = s.avg_duration_ms();
                let p95_duration_ms = s.p95_duration_ms();
                let p99_duration_ms = s.p99_duration_ms();
                let error_rate = s.error_rate();
                let execution_count = s.execution_count;
                let error_count = s.error_count;
                let pattern = s.pattern;
                QueryPatternSnapshot {
                    normalized_query: pattern.normalized_query,
                    query_type: pattern.query_type,
                    labels: pattern.labels,
                    execution_count,
                    avg_duration_ms,
                    p95_duration_ms,
                    p99_duration_ms,
                    error_rate,
                    error_count,
                }
            })
            .collect()
    }

    /// Get top N slow query patterns by total duration
    pub fn get_top_n_patterns_by_total_duration(
        &self,
        limit: usize,
    ) -> Vec<crate::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_top_n_by_total_duration(limit)
    }

    /// Get top N slow query patterns by execution count
    pub fn get_top_n_patterns_by_execution_count(
        &self,
        limit: usize,
    ) -> Vec<crate::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_top_n_by_execution_count(limit)
    }

    /// Get statistics for a specific query pattern
    pub fn get_pattern_stats(
        &self,
        normalized_query: &str,
    ) -> Option<crate::aggregated_stats::AggregatedQueryStats> {
        self.aggregated_stats.get_pattern_stats(normalized_query)
    }

    /// Get all aggregated statistics
    pub fn get_all_aggregated_stats(&self) -> Vec<crate::aggregated_stats::AggregatedQueryStats> {
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
}

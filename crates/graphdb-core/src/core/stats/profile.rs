//! Querying image data and executing executor statistics
//!
//! Query profiles for detailed monitoring and analysis, with microsecond-level accuracy.

use std::time::Instant;

use super::error_stats::{ErrorInfo, ErrorType, QueryPhase};
use super::executor_stats::ExecutorStats;
use super::utils::micros_to_millis;

/// Statistics during the query execution phase (in microseconds)
#[derive(Debug, Clone, Default)]
pub struct StageMetrics {
    pub parse_us: u64,
    pub validate_us: u64,
    pub plan_us: u64,
    pub optimize_us: u64,
    pub execute_us: u64,
}

impl StageMetrics {
    pub fn from_query_metrics(metrics: &crate::core::stats::QueryMetrics) -> Self {
        Self {
            parse_us: metrics.parse_time_us,
            validate_us: metrics.validate_time_us,
            plan_us: metrics.plan_time_us,
            optimize_us: metrics.optimize_time_us,
            execute_us: metrics.execute_time_us,
        }
    }

    pub fn total_ms(&self) -> f64 {
        (self.parse_us + self.validate_us + self.plan_us + self.optimize_us + self.execute_us)
            as f64
            / 1000.0
    }

    pub fn parse_ms(&self) -> f64 {
        micros_to_millis(self.parse_us)
    }

    pub fn validate_ms(&self) -> f64 {
        micros_to_millis(self.validate_us)
    }

    pub fn plan_ms(&self) -> f64 {
        micros_to_millis(self.plan_us)
    }

    pub fn optimize_ms(&self) -> f64 {
        micros_to_millis(self.optimize_us)
    }

    pub fn execute_ms(&self) -> f64 {
        micros_to_millis(self.execute_us)
    }
}

/// Actuator statistics
#[derive(Debug, Clone)]
pub struct ExecutorStat {
    pub executor_type: String,
    pub executor_id: i64,
    pub stats: ExecutorStats,
}

impl ExecutorStat {
    pub fn from_executor(executor_type: String, executor_id: i64, stats: ExecutorStats) -> Self {
        Self {
            executor_type,
            executor_id,
            stats,
        }
    }

    pub fn duration_ms(&self) -> f64 {
        micros_to_millis(self.stats.exec_time_us)
    }

    pub fn rows_processed(&self) -> usize {
        self.stats.num_rows
    }

    pub fn memory_used(&self) -> usize {
        self.stats.memory_peak
    }
}

/// Query status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStatus {
    Success,
    Failed,
}

/// Query the image
///
/// Query profiles for detailed monitoring and analysis, with microsecond-level accuracy.
/// Differences from QueryMetrics:
/// QueryProfile: Provides detailed monitoring data for internal analysis and logging (in microseconds).
/// QueryMetrics: A lightweight component designed to provide results to the client in a very short time (within microseconds).
#[derive(Debug, Clone)]
pub struct QueryProfile {
    pub trace_id: String,
    pub session_id: i64,
    pub query_text: String,
    pub start_time: Instant,
    pub total_duration_us: u64,
    pub stages: StageMetrics,
    pub executor_stats: Vec<ExecutorStat>,
    pub result_count: usize,
    pub status: QueryStatus,
    pub error_message: Option<String>,
    pub error_info: Option<ErrorInfo>,
}

impl QueryProfile {
    pub fn new(session_id: i64, query_text: String) -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            session_id,
            query_text,
            start_time: Instant::now(),
            total_duration_us: 0,
            stages: StageMetrics::default(),
            executor_stats: Vec::new(),
            result_count: 0,
            status: QueryStatus::Success,
            error_message: None,
            error_info: None,
        }
    }

    pub fn mark_failed(&mut self, error: String) {
        self.status = QueryStatus::Failed;
        self.error_message = Some(error);
    }

    pub fn mark_failed_with_info(&mut self, error_info: ErrorInfo) {
        self.status = QueryStatus::Failed;
        self.error_message = Some(error_info.error_message.clone());
        self.error_info = Some(error_info);
    }

    pub fn error_type(&self) -> Option<ErrorType> {
        self.error_info.as_ref().map(|e| e.error_type)
    }

    pub fn error_phase(&self) -> Option<QueryPhase> {
        self.error_info.as_ref().map(|e| e.error_phase)
    }

    pub fn add_executor_stat(&mut self, stat: ExecutorStat) {
        self.executor_stats.push(stat);
    }

    pub fn add_executor_stats_from(
        &mut self,
        executor_type: String,
        executor_id: i64,
        stats: ExecutorStats,
    ) {
        let stat = ExecutorStat::from_executor(executor_type, executor_id, stats);
        self.executor_stats.push(stat);
    }

    pub fn set_stage_metrics(&mut self, metrics: crate::core::stats::QueryMetrics) {
        self.stages = StageMetrics::from_query_metrics(&metrics);
    }

    pub fn total_executor_time_us(&self) -> u64 {
        self.executor_stats
            .iter()
            .map(|s| s.stats.exec_time_us)
            .sum()
    }

    pub fn total_executor_time_ms(&self) -> f64 {
        micros_to_millis(self.total_executor_time_us())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::executor_stats::ExecutorStats;

    #[test]
    fn test_query_profile_creation() {
        let profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        assert_eq!(profile.session_id, 123);
        assert_eq!(profile.query_text, "MATCH (n) RETURN n");
        assert!(!profile.trace_id.is_empty());
        assert!(matches!(profile.status, QueryStatus::Success));
    }

    #[test]
    fn test_query_profile_mark_failed() {
        let mut profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        profile.mark_failed("Syntax error".to_string());
        assert!(matches!(profile.status, QueryStatus::Failed));
        assert_eq!(profile.error_message, Some("Syntax error".to_string()));
    }

    #[test]
    fn test_query_profile_add_executor_stat() {
        let mut profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        let stats = ExecutorStats {
            exec_time_us: 100_000,
            num_rows: 50,
            memory_peak: 1024,
            ..ExecutorStats::default()
        };
        let stat = ExecutorStat::from_executor("ScanVerticesExecutor".to_string(), 1, stats);
        profile.add_executor_stat(stat);
        assert_eq!(profile.executor_stats.len(), 1);
        assert_eq!(profile.total_executor_time_us(), 100_000);
        assert!((profile.total_executor_time_ms() - 100.0).abs() < 0.001);
    }

    #[test]
    fn test_stage_metrics_default() {
        let metrics = StageMetrics::default();
        assert_eq!(metrics.parse_us, 0);
        assert_eq!(metrics.execute_us, 0);
        assert!((metrics.total_ms() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_executor_stat_from_executor() {
        let stats = ExecutorStats {
            exec_time_us: 1500,
            num_rows: 100,
            memory_peak: 2048,
            ..ExecutorStats::default()
        };

        let stat = ExecutorStat::from_executor("TestExecutor".to_string(), 1, stats);

        assert!((stat.duration_ms() - 1.5).abs() < 0.001);
        assert_eq!(stat.rows_processed(), 100);
        assert_eq!(stat.memory_used(), 2048);
    }

    #[test]
    fn test_stage_metrics_from_query_metrics() {
        let metrics = crate::core::stats::QueryMetrics {
            parse_time_us: 100,
            execute_time_us: 500,
            ..crate::core::stats::QueryMetrics::default()
        };

        let stages = StageMetrics::from_query_metrics(&metrics);

        assert_eq!(stages.parse_us, 100);
        assert_eq!(stages.execute_us, 500);
        assert!((stages.parse_ms() - 0.1).abs() < 0.001);
        assert!((stages.execute_ms() - 0.5).abs() < 0.001);
    }
}

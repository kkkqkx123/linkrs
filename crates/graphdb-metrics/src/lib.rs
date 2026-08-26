//! Central metrics and observability primitives for GraphDB.
//!
//! Extracted from graphdb-core so any crate can depend on it without pulling
//! in the core data model. Hosts the unified [`StatsManager`] with its
//! [`MetricType`] registry, latency percentile histograms, error statistics,
//! aggregated query patterns and the slow-query logger.

pub mod aggregated_stats;
pub mod error_stats;
pub mod executor_stats;
pub mod latency_histogram;
pub mod manager;
pub mod metrics;
pub mod profile;
pub mod slow_query_logger;
pub mod utils;

// Re-export common types
pub use aggregated_stats::{AggregatedQueryStats, AggregatedStatsManager, QueryPattern};
pub use error_stats::{
    ErrorInfo, ErrorStatsManager, ErrorSummary, ErrorType, QueryPhase, RecentError,
};
pub use latency_histogram::LatencyHistogram;
pub use manager::{MetricType, MetricValue, OutboxState, StatsManager, TxnResourceMetrics};
pub use metrics::QueryMetrics;
pub use profile::{ExecutorStat, QueryProfile, QueryStatus, StageMetrics};
pub use slow_query_logger::{SlowQueryConfig, SlowQueryLogger};
pub use utils::{
    calculate_average, calculate_cache_hit_rate, duration_to_micros, format_duration,
    micros_to_millis, CacheStats, TimeConversion,
};

//! Statistical Information Module
//!
//! Provides query metrics, query portraits and error statistics.
//!
//! ## Module structure
//!
//! - `metrics`: lightweight query metrics (for return to client)
//! - `profile`: detailed query profile (for monitoring and analysis)
//! - `error_stats`: error statistics
//! - `manager`: unified manager
//! - `latency_histogram`: latency percentile calculations
//!
//! ## QueryMetrics vs QueryProfile
//!
//! ### QueryMetrics (lightweight)
//! - Purpose: Query metrics returned to the client
//! - Accuracy: microseconds (us)
//! - Content: execution time, number of nodes, number of results
//! - Usage scenarios: API response, client-side display
//!
//! ### QueryProfile (detailed)
//! - Purpose: Internal analysis and monitoring
//! - Accuracy: milliseconds (ms)
//! - Contents: execution time, actuator statistics, error messages, slow query logs
//! - Usage scenarios: performance analysis, problem diagnosis, monitoring alarms

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
pub use manager::{MetricType, MetricValue, StatsManager};
pub use metrics::QueryMetrics;
pub use profile::{ExecutorStat, QueryProfile, QueryStatus, StageMetrics};
pub use slow_query_logger::{SlowQueryConfig, SlowQueryLogger};
pub use utils::{
    calculate_average, calculate_cache_hit_rate, duration_to_micros, format_duration,
    micros_to_millis, CacheStats, TimeConversion,
};

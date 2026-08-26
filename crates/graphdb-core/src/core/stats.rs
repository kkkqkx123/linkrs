//! Compatibility re-export of the statistics subsystem.
//!
//! The implementation lives in the `graphdb-metrics` crate; both the
//! submodules and the flattened item names keep resolving through this
//! module, so existing `core::stats::*` paths are unaffected.

pub use graphdb_metrics::aggregated_stats;
pub use graphdb_metrics::error_stats;
pub use graphdb_metrics::executor_stats;
pub use graphdb_metrics::latency_histogram;
pub use graphdb_metrics::manager;
pub use graphdb_metrics::metrics;
pub use graphdb_metrics::profile;
pub use graphdb_metrics::slow_query_logger;
pub use graphdb_metrics::utils;

pub use graphdb_metrics::*;

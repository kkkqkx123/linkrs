//! Explain/Profile Executors
//!
//! This module provides executors for EXPLAIN and PROFILE statements.
//!
//! ## Components
//!
//! - `ExplainExecutor`: Handles EXPLAIN and EXPLAIN ANALYZE statements
//! - `ProfileExecutor`: Handles PROFILE statements with detailed statistics
//! - `ExecutionStatsContext`: Global context for managing execution statistics
//! - `format`: Utilities for formatting plan descriptions

pub mod execution_stats_context;
pub mod explain_executor;
pub mod format;
pub mod physical_plan_explain;
pub mod profile_executor;

// Re-export main types
pub use execution_stats_context::{
    ExecutionStatsContext, GlobalExecutionStats, NodeExecutionStats,
};
pub use explain_executor::{ExplainExecutor, ExplainMode};
pub use profile_executor::ProfileExecutor;

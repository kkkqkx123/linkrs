//! Explain/Profile Executors
//!
//! This module provides executors for EXPLAIN and PROFILE statements.
//!
//! ## Components
//!
//! - `ExecutionStatsContext`: Global context for managing execution statistics
//! - `format`: Utilities for formatting plan descriptions

pub mod execution_stats_context;
pub mod format;
pub mod physical_plan_explain;

// Re-export main types
pub use execution_stats_context::{
    ExecutionStatsContext, GlobalExecutionStats, NodeExecutionStats,
};

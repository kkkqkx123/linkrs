//! Unified Module for Basic Types of Actuators
//!
//! This module defines all the basic types related to actuators in a centralized manner, eliminating duplicate definitions and ensuring type consistency.
//!
//! Module structure:
//! - executor_stats.rs    - Executor statistics
//! - execution_result.rs  - Execution result type
//! - execution_context.rs - Execution context
//! - memory_budget.rs     - Memory budget and tracking
//! - traits.rs            - Search provider trait

pub mod execution_context;
pub mod execution_result;
pub mod executor_stats;
pub mod memory_budget;
pub mod traits;

pub use execution_context::{ExecutionContext, SearchContext};
pub use execution_result::{DBResult, ExecutionResult, IntoExecutionResult};
pub use executor_stats::ExecutorStats;
pub use memory_budget::{
    MemoryBudget, MemoryReservation, MemoryTracker, MemoryTrackerReservation, Spillable,
};
#[cfg(feature = "fulltext")]
pub use traits::FulltextProvider;
#[cfg(feature = "vector")]
pub use traits::VectorProvider;
pub use traits::{IndexInfo, SearchProvider, SearchProviderType};

pub use graphdb_core::types::EdgeDirection;

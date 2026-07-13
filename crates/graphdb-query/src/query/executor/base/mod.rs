//! Unified Module for Basic Types of Actuators
//!
//! This module defines all the basic types related to actuators in a centralized manner, eliminating duplicate definitions and ensuring type consistency.
//!
//! Module structure:
//! - executor_stats.rs    - Executor statistics
//! - execution_result.rs  - Execution result type
//! - execution_context.rs - Execution context
//! - executor_base.rs     - Basic executor implementation
//! - result_processor.rs  - Result processor
//! - config.rs            - Executor configuration structure

pub mod config;
pub mod execution_context;
pub mod execution_result;
pub mod executor_base;
pub mod executor_stats;
pub mod memory_budget;
pub mod result_processor;

pub use config::{
    AppendVerticesConfig, ApplyConfig, ExecutorConfig,
    IndexScanConfig, JoinConfig, JoinConfigWithDesc, LoopConfig,
    PatternApplyConfig, RollupApplyConfig,
};
pub use execution_context::ExecutionContext;
pub use execution_result::{DBResult, ExecutionResult, IntoExecutionResult};
pub use executor_base::{BaseExecutor, Executor, HasInput, HasStorage, StartExecutor};
pub use executor_stats::ExecutorStats;
pub use memory_budget::{
    MemoryBudget, MemoryReservation, MemoryTracker, MemoryTrackerReservation, Spillable,
};
pub use result_processor::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};

pub use crate::core::types::EdgeDirection;

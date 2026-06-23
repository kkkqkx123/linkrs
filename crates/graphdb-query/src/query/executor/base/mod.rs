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
//! - executor_enum.rs     - Executor enumeration

pub mod config;
pub mod execution_context;
pub mod execution_result;
pub mod executor_base;
pub mod executor_enum;
pub mod executor_stats;
pub mod manage_executor_enums;
pub mod result_processor;

pub use config::{
    AllPathsConfig, AppendVerticesConfig, BfsShortestConfig, ExecutorConfig, IndexScanConfig,
    JoinConfig, JoinConfigWithDesc, LoopConfig, MultiShortestPathConfig, PathConfig,
    PatternApplyConfig, RollupApplyConfig, ShortestPathConfig,
};
pub use execution_context::ExecutionContext;
pub use execution_result::{DBResult, ExecutionResult, IntoExecutionResult};
pub use executor_base::{
    BaseExecutor, ChainableExecutor, Executor, HasInput, HasStorage, InputExecutor, StartExecutor,
};
pub use executor_enum::ExecutorEnum;
pub use executor_stats::ExecutorStats;
#[cfg(feature = "fulltext-search")]
pub use manage_executor_enums::FulltextManageExecutor;
#[cfg(feature = "qdrant")]
pub use manage_executor_enums::VectorManageExecutor;
pub use manage_executor_enums::{
    EdgeManageExecutor, IndexManageExecutor, SpaceManageExecutor, TagManageExecutor,
    UserManageExecutor,
};
pub use result_processor::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};

pub use crate::core::types::EdgeDirection;

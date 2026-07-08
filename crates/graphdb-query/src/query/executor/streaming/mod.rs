//! Streaming execution engine for pull-based iterator model
//!
//! This module implements a pull-based chunked streaming executor system
//! that efficiently processes data through an operator pipeline.
//!
//! Key components:
//! - DataChunk: Fixed-size batches of rows
//! - StreamingExecutor: Enum-based pull operators
//! - PartitionView: Data partitioning for parallel execution
//! - PipelineScheduler: Task dependency and scheduling
//! - WorkerPool: Multi-threaded task execution
//! - StreamingExecutionEngine: Orchestration layer

pub mod base;
pub mod builder;
pub mod chunk;
pub mod decision;
pub mod engine;
pub mod executor;
pub mod factory;
pub mod partition;
pub mod scheduler;
pub mod worker;

pub use base::ExecutionMode;
pub use builder::StreamingExecutorBuilder;
pub use chunk::DataChunk;
pub use decision::{decide_execution_mode, ExecutionMode as StreamingMode, StreamingDecisionConfig};
pub use engine::StreamingExecutionEngine;
pub use executor::StreamingExecutor;
pub use factory::{StreamingQueryExecutor, convert_chunks_to_dataset, chunks_to_execution_result};
pub use partition::PartitionView;
pub use scheduler::{PipelineScheduler, SchedulerConfig, Task, TaskStatus};
pub use worker::{BackpressureControl, TaskResult, WorkerPool};

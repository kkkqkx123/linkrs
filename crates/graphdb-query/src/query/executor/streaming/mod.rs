//! Streaming execution engine for pull-based iterator model
//!
//! This module implements the new StreamingExecutor system that replaces
//! the push-based materialized execution with pull-based chunked execution.
//!
//! Phase 0: Basic infrastructure (DataChunk, StreamingExecutor)
//! Phase 1: Parallel execution framework (PartitionView, PipelineScheduler, WorkerPool)

pub mod chunk;
pub mod executor;
pub mod base;
pub mod partition;
pub mod scheduler;
pub mod worker;
pub mod engine;

pub use chunk::DataChunk;
pub use executor::StreamingExecutor;
pub use base::ExecutionMode;
pub use partition::PartitionView;
pub use scheduler::{PipelineScheduler, SchedulerConfig, Task, TaskStatus};
pub use worker::{WorkerPool, BackpressureControl, TaskResult};
pub use engine::{StreamingExecutionEngine, StreamingEngineConfig};

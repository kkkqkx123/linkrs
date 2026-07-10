//! Streaming execution engine for pull-based iterator model
//!
//! This module implements a pull-based chunked streaming executor system
//! that efficiently processes data through an operator pipeline.
//!
//! Key components:
//! - DataChunk: Fixed-size batches of rows
//! - StreamingExecutor: Enum-based pull operators
//! - StreamingExecutionEngine: Orchestration layer

pub mod base;
pub mod builder;
pub mod chunk;
pub mod engine;
pub mod executor;
pub mod factory;
pub mod partition;

pub use base::ExecutionMode;
pub use builder::StreamingExecutorBuilder;
pub use chunk::DataChunk;
pub use engine::StreamingExecutionEngine;
pub use executor::StreamingExecutor;
pub use factory::{chunks_to_execution_result, convert_chunks_to_dataset, StreamingQueryExecutor};
pub use partition::PartitionView;

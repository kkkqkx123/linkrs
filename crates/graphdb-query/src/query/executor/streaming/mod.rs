//! Streaming execution engine for pull-based iterator model
//!
//! This module implements a pull-based chunked streaming executor system
//! that efficiently processes data through an operator pipeline.
//!
//! Key components:
//! - DataChunk: Fixed-size batches of rows
//! - StreamingExecutor: Enum-based pull operators
//! - StreamingExecutionEngine: Orchestration layer
//! - ExecutionRuntime: Per-query runtime (cancel, memory, profile, resources)
//! - ResultStream: Streaming result handle

pub mod base;
pub mod builder;
pub mod chunk;
pub mod context;
pub mod coordinator;
pub mod driver;
pub mod engine;
pub mod executor;
pub mod factory;
pub mod helpers;
pub mod join_helpers;
mod physical_builder;
pub mod result_utils;
pub mod operator_base;
pub mod operators;
pub mod parallel_safety;
pub mod partition;
pub mod runtime;
pub mod slot;
pub mod stream;
pub mod stream_result;

pub use base::ExecutionMode;
pub use builder::StreamingExecutorBuilder;
pub use chunk::DataChunk;
pub use driver::ExecutorDriver;
pub use engine::StreamingExecutionEngine;
pub use executor::StreamingExecutor;
pub use factory::StreamingQueryExecutor;
pub use result_utils::{chunks_to_execution_result, convert_chunks_to_dataset};
pub use partition::PartitionView;
pub use runtime::{
    ExecutionRuntime, OperatorProfile, OperatorProfileKey, ProfileCollector, QueryFinishGuard,
    QueryIdentity, ResourceOwner,
};
pub use slot::{
    combine_layouts, combine_layouts_with_dedup, SlotId, SlotInfo, SlotLayout, SlotOrigin,
};
pub use stream::ResultStream;
pub use stream_result::StreamingQueryResult;

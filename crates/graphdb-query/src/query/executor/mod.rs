// Re-export all executor modules
pub mod base;
pub mod build_error;
pub mod explain;
pub mod expression;
pub mod streaming;
pub mod traversal;

// Re-export from the base module: The basic types are uniformly exported from the base module.
pub use base::{
    ExecutionContext, ExecutionResult, ExecutorStats, MemoryBudget, MemoryReservation,
    MemoryTracker, MemoryTrackerReservation, Spillable,
};

// Re-export streaming executors (Primary Execution Framework)
pub use streaming::{
    combine_layouts, DataChunk, ExecutionRuntime, OperatorProfile, PartitionView, ProfileCollector,
    QueryIdentity, ResourceOwner, ResultStream, SlotId, SlotInfo, SlotLayout,
    StreamingExecutionEngine, StreamingExecutor, StreamingQueryResult,
};

// Re-export explain types
pub use explain::{ExecutionStatsContext, NodeExecutionStats};

// Re-export core execution states
pub use crate::query::core::{ExecutorState, LoopExecutionState, QueryExecutionState, RowStatus};

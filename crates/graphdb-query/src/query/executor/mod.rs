// Re-export all executor modules
pub mod algorithms;
pub mod base;
pub mod build_error;
pub mod explain;
pub mod expression;
pub mod streaming;
pub mod traversal;

// Re-export from the base module: The basic types are uniformly exported from the base module.
pub use base::{
    BaseExecutor, ExecutionContext, ExecutionResult, Executor, ExecutorStats, HasInput, HasStorage,
    ResultProcessor, ResultProcessorContext, StartExecutor,
};

// Re-export streaming executors (Primary Execution Framework)
pub use streaming::{
    combine_layouts, DataChunk, ExecutionRuntime, OperatorProfile, PartitionView, ProfileCollector,
    QueryIdentity, ResourceOwner, ResultStream, SlotId, SlotInfo, SlotLayout,
    StreamingExecutionEngine, StreamingExecutor, StreamingQueryResult,
};

// Re-export explain/profile executors
pub use explain::{
    ExecutionStatsContext, ExplainExecutor, ExplainMode, NodeExecutionStats, ProfileExecutor,
};

// Re-export algorithm types
pub use algorithms::{
    AStar, AlgorithmContext, AlgorithmStats, BFSShortestExecutor, BidirectionalBFS, Dijkstra,
    MultiShortestPathExecutor, PathFindingAlgorithm, ShortestPathAlgorithm,
    ShortestPathAlgorithmType, SubgraphExecutor, TraversalAlgorithm,
};

// Re-export core execution states
pub use crate::query::core::{ExecutorState, LoopExecutionState, QueryExecutionState, RowStatus};

// Re-export all executor modules
pub mod admin;
pub mod base;
pub mod control_flow;
pub mod data_access;
pub mod data_modification;
pub mod explain;
pub mod expression;
pub mod graph_operations;
pub mod relational_algebra;
pub mod result_processing;
pub mod streaming;
pub mod utils;

// Re-export from the base module: The basic types are uniformly exported from the base module.
pub use base::{
    BaseExecutor, ExecutionContext, ExecutionResult, Executor,
    ExecutorStats, HasInput, HasStorage, ResultProcessor, ResultProcessorContext,
    StartExecutor,
};

// Re-export data access executors
pub use data_access::{
    AllPathsExecutor, GetEdgesExecutor, GetNeighborsExecutor, GetPropExecutor, GetVerticesExecutor,
    IndexScanExecutor, LookupIndexExecutor, ScanVerticesExecutor,
};

// Re-export result processing executors
pub use result_processing::{
    AggData, AggFunctionManager,
};

// Re-export relational algebra executors
pub use relational_algebra::{
    AggregateExecutor, AggregateFunctionSpec, CrossJoinExecutor, FilterExecutor,
    FullOuterJoinExecutor, GroupAggregateState, GroupByExecutor, HashInnerJoinExecutor,
    HashLeftJoinExecutor, HavingExecutor, InnerJoinExecutor, IntersectExecutor, LeftJoinExecutor,
    MinusExecutor, ProjectExecutor, ProjectionColumn, SetExecutor, UnionAllExecutor, UnionExecutor,
    WindowExecutor,
};

// Re-export transformations (Data conversion executors)
pub use result_processing::transformations::{
    AppendVerticesExecutor, AssignExecutor, PatternApplyExecutor, RollUpApplyExecutor,
    UnwindExecutor,
};

// Re-export core execution states
pub use crate::query::core::{ExecutorState, LoopExecutionState, QueryExecutionState, RowStatus};

// Re-export admin executors
pub use admin::{
    AlterEdgeExecutor, AlterTagExecutor, AlterUserExecutor, ChangePasswordExecutor,
    CreateEdgeExecutor, CreateEdgeIndexExecutor, CreateSpaceExecutor, CreateTagExecutor,
    CreateTagIndexExecutor, CreateUserExecutor, DescEdgeExecutor, DescEdgeIndexExecutor,
    DescSpaceExecutor, DescTagExecutor, DescTagIndexExecutor, DropEdgeExecutor,
    DropEdgeIndexExecutor, DropSpaceExecutor, DropTagExecutor, DropTagIndexExecutor,
    DropUserExecutor, RebuildEdgeIndexExecutor, RebuildTagIndexExecutor, ShowEdgeIndexesExecutor,
    ShowEdgesExecutor, ShowSpacesExecutor, ShowTagIndexesExecutor, ShowTagsExecutor,
};

// Legacy utility executors removed - use StreamingExecutor instead
// pub use utils::{ArgumentExecutor, DataCollectExecutor, PassThroughExecutor};

// Re-export streaming executors (Primary Execution Framework)
pub use streaming::{
    BackpressureControl, DataChunk, ExecutionMode, PartitionView, PipelineScheduler,
    SchedulerConfig, StreamingExecutionEngine, StreamingExecutor, StreamingExecutorBuilder, Task,
    TaskResult, TaskStatus, WorkerPool,
};

// Re-export graph traversal executors
pub use crate::query::executor::graph_operations::graph_traversal::algorithms::BFSShortestExecutor;

// Re-export explain/profile executors
pub use explain::{
    ExecutionStatsContext, ExplainExecutor, ExplainMode, InstrumentedExecutor,
    InstrumentedExecutorFactory, NodeExecutionStats, ProfileExecutor,
};


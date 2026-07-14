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

pub mod builder;
mod partition_builder;
pub mod chunk;
pub mod context;
pub mod coordinator;
pub mod engine;
pub mod executor;
pub mod factory;
pub mod helpers;
pub mod instance;
pub mod join_helpers;
pub mod operator_plan_builder;
pub mod operators;
pub mod parallel_safety;
pub mod partition;
pub mod plan;
pub mod pool;
pub mod parameters;
pub mod result_utils;
pub mod runtime;
pub mod slot;
pub mod spill;
pub mod state;
pub mod stream;
pub mod stream_result;
pub mod query_registry;
pub mod transaction_scope;

pub use builder::StreamingExecutorBuilder;
pub use chunk::DataChunk;
pub use context::BorrowedRowContext;
pub use engine::StreamingExecutionEngine;
pub use executor::StreamingExecutor;
pub use factory::StreamingQueryExecutor;
pub use operators::base::OperatorBase;
pub use operators::spec::{BlockingSpec, ExchangeSpec, JoinSpec, SourceSpec, UnarySpec};
pub use operators::state::{BlockingState, ExchangeState, JoinState, SourceState, UnaryState};
pub use partition::PartitionView;
pub use plan::node::PhysicalNode;
pub use result_utils::{chunks_to_execution_result, convert_chunks_to_dataset};
pub use runtime::{
    ExecutionRuntime, OperatorProfile, OperatorProfileKey, ProfileCollector, QueryFinishGuard,
    QueryIdentity, ResourceOwner,
};
pub use slot::{
    combine_layouts, combine_layouts_with_dedup, SlotId, SlotInfo, SlotLayout, SlotOrigin,
};
pub use stream::ResultStream;
pub use stream_result::StreamingQueryResult;

pub use plan::types::{
    CapabilitySet, FragmentGraph, FragmentId, FragmentKind, FragmentSpec, LogicalNodeId,
    OperatorKindSpec, OutputContract, PhysicalOperatorId, PhysicalOperatorIdAllocator,
    PhysicalOperatorSpec, PhysicalPlan, PlanCompatibility,
};
pub use plan::context::PhysicalPlanBuildContext;
pub use plan::validator::{PhysicalPlanValidator, ValidationResult, ValidationTier};

// ── Spill types ──
pub use spill::{RowBuffer, SpillConfig, SpilledFile, SpillManager, SpillReader, SpillWriter};

// ── P2: New types ──
pub use instance::{QueryBindings, QueryExecutionInstance, ResultSink};
pub use state::{
    GlobalState, GlobalStateArena, GlobalStateKey, LocalState, LocalStateArena, LocalStateKey,
    StateArenaSet, TaskId,
};
pub use query_registry::{
    CancelToken, QueryGuard, QueryId, QueryMetadata, QueryRegistry,
};
pub use transaction_scope::{
    CancelReason, SessionTransactionController, TransactionCommandResult, TransactionId,
    TransactionScope, TransactionState,
};

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
pub mod join_helpers;
pub mod operator_base;
pub mod operator_plan_builder;
pub mod operator_spec;
pub mod operator_state;
pub mod operators;
pub mod parallel_safety;
pub mod partition;
mod physical_builder;
pub mod physical_node;
pub mod physical_plan;
pub mod physical_plan_context;
pub mod physical_plan_validator;
pub mod physical_properties;
pub mod pool;
pub mod result_utils;
pub mod runtime;
pub mod slot;
pub mod stream;
pub mod stream_result;

pub use builder::StreamingExecutorBuilder;
pub use chunk::DataChunk;
pub use engine::StreamingExecutionEngine;
pub use executor::StreamingExecutor;
pub use factory::StreamingQueryExecutor;
pub use operator_spec::{BlockingSpec, ExchangeSpec, JoinSpec, SourceSpec, UnarySpec};
pub use operator_state::{BlockingState, ExchangeState, JoinState, SourceState, UnaryState};
pub use partition::PartitionView;
pub use physical_node::PhysicalNode;
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

pub use physical_plan::{
    CapabilitySet, FragmentGraph, FragmentId, FragmentKind, FragmentSpec, LogicalNodeId,
    OperatorKindSpec, OutputContract, PhysicalOperatorId, PhysicalOperatorIdAllocator,
    PhysicalOperatorSpec, PhysicalPlan, PlanCompatibility,
};
pub use physical_plan_context::PhysicalPlanBuildContext;
pub use physical_plan_validator::{PhysicalPlanValidator, ValidationResult, ValidationTier};

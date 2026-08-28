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

pub mod chunk;
pub mod context;
pub mod engine;
pub mod executor;

pub mod helpers;
pub mod instance;
pub mod interner;
pub mod join_helpers;
pub mod memory_pool;
pub mod operators;
pub mod parameters;
pub mod partition;
pub mod plan;
pub mod pool;
pub mod query_registry;
pub mod result_utils;
pub mod runtime;
pub mod slot;
pub mod spill;
pub mod state;
pub mod stream;
pub mod stream_result;
pub mod subquery;
pub mod transaction_scope;

pub use chunk::{ChunkView, DataChunk, RowPool};
pub use context::BorrowedRowContext;
pub use engine::StreamingExecutionEngine;
pub use executor::StreamingExecutor;

pub use operators::base::OperatorBase;
pub use operators::spec::{BlockingSpec, ExchangeSpec, JoinSpec, SourceSpec, UnarySpec};
pub use operators::state::{BlockingState, ExchangeState, JoinState, SourceState};
pub use partition::PartitionView;

pub use result_utils::{chunks_to_execution_result, convert_chunks_to_dataset};
pub use runtime::{
    ExecutionRuntime, OperatorProfile, OperatorProfileKey, ProfileBoard, ProfileCollector,
    ProfileEntry, QueryFinishGuard, QueryIdentity, ResourceOwner,
};
pub use slot::{
    combine_layouts, combine_layouts_with_dedup, SlotId, SlotInfo, SlotLayout, SlotOrigin,
};
pub use stream::ResultStream;
pub use stream_result::StreamingQueryResult;

pub use plan::context::PhysicalPlanBuildContext;
pub use plan::types::{
    CapabilitySet, FragmentGraph, FragmentId, FragmentKind, FragmentSpec, LogicalNodeId,
    OperatorKindSpec, OutputContract, PhysicalOperatorId, PhysicalOperatorIdAllocator,
    PhysicalOperatorSpec, PhysicalPlan, PlanCompatibility, PlanFingerprint, SortOrder,
};
pub use plan::validator::{PhysicalPlanValidator, ValidationResult, ValidationTier};

// ── Spill types ──
pub use spill::{
    cleanup_orphan_spill_dirs, hash_row_partition, DiskQuota, HashPartitionConfig,
    HashPartitionSpiller, RowBuffer, RunHeader, RunReader, RunWriter, SpillConfig, SpillManager,
    SpillReader, SpillWriter, SpilledFile, SpilledRun,
};

// ── New types ──
pub use instance::{QueryBindings, QueryExecutionInstance, ResultSink};
pub use query_registry::{CancelToken, QueryGuard, QueryId, QueryMetadata, QueryRegistry};
pub use state::{
    GlobalState, GlobalStateArena, GlobalStateKey, LocalState, LocalStateArena, LocalStateKey,
    StateArenaSet, TaskId,
};
pub use transaction_scope::{
    CancelReason, SessionTransactionController, TransactionCommandResult, TransactionId,
    TransactionScope, TransactionState,
};

// ── M4: Memory pool types ──
pub use memory_pool::{
    DatabaseMemoryPool, FragmentPool, MemoryPoolError, MemoryPoolReservation, OperatorPool,
    PoolHandle, PooledChunk, QueryPool, TaskPool,
};

//! QueryExecutionInstance: single-entry instantiation point for physical plans.
//!
//! The target production entry path:
//!
//! ```text
//! Arc<PhysicalPlan> + QueryBindings + ResultSink + Scheduler
//!     → QueryExecutionInstance::instantiate
//!     → ExecutionRuntime (created once, shared by engine/operators/handle)
//!     → operator tree (materialized per invocation)
//!     → delivery via ResultSink
//! ```
//!
//! Until the PhysicalPlan arena → StreamingExecutor bridge is built (M2),
//! `instantiate` accepts a [`PhysicalNode`] tree alongside the plan for
//! materialization.  Once the bridge exists, the `PhysicalNode` parameter
//! is removed and `instantiate` becomes the sole production path.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::engine::StreamingExecutionEngine;
use super::plan::materializer::PhysicalPlanMaterializer;
use super::plan::types::PhysicalPlan;
use super::plan::validator::PhysicalPlanValidator;
use super::pool::MorselWorkerPool;
use super::result_utils::convert_chunks_to_dataset;
use super::runtime::{ExecutionRuntime, QueryIdentity};
use super::stream_result::StreamingQueryResult;
use super::PhysicalNode;
use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::{ExecutionResult, MemoryBudget};
use crate::storage::StorageClient;

use super::transaction_scope::TransactionScope;

// ── QueryBindings ───────────────────────────────────────────────────────────

/// Per-execution bindings that parameterize a [`PhysicalPlan`].
///
/// Unlike the plan (immutable, cacheable), `QueryBindings` carries data that
/// changes per invocation: parameter values, storage handles, transaction
/// scope, memory budget, and delivery preferences.
///
/// Multiple concurrent executions of the same plan each have their own
/// `QueryBindings`, ensuring no mutable state is shared across instances.
#[derive(Clone)]
pub struct QueryBindings {
    /// Prepared-statement parameter values.
    pub parameters: Arc<HashMap<String, Value>>,
    /// Target space name.
    pub space_name: Option<String>,
    /// Storage client for this execution.
    pub storage: Option<Arc<RwLock<dyn StorageClient>>>,
    /// Per-query memory budget.
    pub memory_budget: MemoryBudget,
    /// Maximum intra-query worker threads (1 = serial only).
    pub max_workers: usize,
    /// Rows per output chunk.
    pub chunk_size: usize,
    /// Max buffered chunks per partition channel before back-pressure.
    pub max_buffered_chunks: usize,
    /// Server-assigned query ID (for KILL QUERY / metrics).
    pub query_id: u64,
    /// Transaction scope for this execution.
    pub transaction: TransactionScope,
    #[cfg(feature = "fulltext-search")]
    pub fulltext_manager: Option<Arc<crate::search::manager::FulltextIndexManager>>,
    #[cfg(feature = "qdrant")]
    pub vector_coordinator: Option<Arc<crate::sync::VectorSyncCoordinator>>,
}

impl QueryBindings {
    /// Build bindings from an [`ExecutionContext`] and a transaction scope.
    pub fn from_context(
        context: &crate::query::executor::base::ExecutionContext,
        transaction: TransactionScope,
    ) -> Self {
        Self {
            parameters: context.parameters.clone(),
            space_name: context.space_name.clone(),
            storage: context.storage.clone(),
            memory_budget: context.memory_budget.clone(),
            max_workers: context.max_workers,
            chunk_size: context.chunk_size,
            max_buffered_chunks: context.max_buffered_chunks,
            query_id: context.query_id,
            transaction,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: context.fulltext_manager.clone(),
            #[cfg(feature = "qdrant")]
            vector_coordinator: context.vector_coordinator.clone(),
        }
    }
}

// ── Delivery sink ───────────────────────────────────────────────────────────

/// Delivery mechanism for query results.
///
/// The sink is chosen at instantiation time.  Schema is always available
/// before the first data row.  Changing the sink does not rebuild the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultSink {
    /// Materialize all chunks into a single [`ExecutionResult`].
    Materialize,
    /// Stream chunks one-at-a-time through a thread-safe handle.
    Stream,
    /// Discard all output (for side-effect-only commands).
    Discard,
}

// ── QueryExecutionInstance ──────────────────────────────────────────────────

/// A single execution instance of a [`PhysicalPlan`].
///
/// Owns the runtime, root memory pool, operator tree, task group, and
/// delivery state for exactly one query invocation.  Multiple concurrent
/// executions of the same plan produce separate `QueryExecutionInstance`
/// values.
pub struct QueryExecutionInstance {
    plan: Arc<PhysicalPlan>,
    _bindings: QueryBindings,
    runtime: Arc<ExecutionRuntime>,
    engine: Option<StreamingExecutionEngine>,
    sink: ResultSink,
}

impl QueryExecutionInstance {
    /// Instantiate from a [`PhysicalPlan`] arena (production path).
    ///
    /// Uses [`PhysicalPlanMaterializer`] to convert the arena plan into an
    /// operator tree, then wraps it with runtime, engine, and sink.
    ///
    /// This is the sole production path.  The old [`PhysicalNode`]-based
    /// [`instantiate`](Self::instantiate) exists only for the transition.
    pub fn instantiate_plan(
        plan: Arc<PhysicalPlan>,
        bindings: QueryBindings,
        sink: ResultSink,
    ) -> Result<Self, QueryError> {
        // Phase 1: validate the plan (structural + binding).
        PhysicalPlanValidator::validate(&plan)?;

        // Phase 2: materialize arena plan → executor tree via materializer.
        let (executor, runtime) =
            PhysicalPlanMaterializer::materialize(&plan, &bindings)?;

        // Phase 3: set up the engine.
        let mut engine = StreamingExecutionEngine::new();
        engine.set_max_workers(bindings.max_workers);
        engine.set_max_buffered_chunks(bindings.max_buffered_chunks);
        engine.set_runtime(runtime.clone());
        engine.register_executor(0, executor);

        Ok(Self {
            plan,
            _bindings: bindings,
            runtime,
            engine: Some(engine),
            sink,
        })
    }

    /// Instantiate from a [`PhysicalNode`] tree (transition path).
    ///
    /// Takes an already-built `PhysicalNode` and wraps it with plan metadata,
    /// bindings, runtime, and sink.  This is the working path until the
    /// [`PhysicalPlan`] arena → operator bridge is ready.
    ///
    /// The `plan` argument is used for validation and metadata; the
    /// `physical_node` is materialized into the executable operator tree.
    pub fn instantiate(
        plan: Arc<PhysicalPlan>,
        physical_node: PhysicalNode,
        bindings: QueryBindings,
        sink: ResultSink,
        scheduler: Option<MorselWorkerPool>,
    ) -> Result<Self, QueryError> {
        // Phase 1: validate the plan (structural).
        PhysicalPlanValidator::validate(&plan)?;

        // Phase 2: create runtime from bindings.
        let runtime = Self::create_runtime(&bindings, scheduler)?;

        // Phase 3: materialize the operator tree from PhysicalNode.
        let executor = physical_node.materialize(
            Some(runtime.clone()),
            &bindings.memory_budget,
            bindings.chunk_size,
        );

        // Phase 4: set up the engine.
        let mut engine = StreamingExecutionEngine::new();
        engine.set_max_workers(bindings.max_workers);
        engine.set_max_buffered_chunks(bindings.max_buffered_chunks);
        engine.set_runtime(runtime.clone());
        engine.register_executor(0, executor);

        Ok(Self {
            plan,
            _bindings: bindings,
            runtime,
            engine: Some(engine),
            sink,
        })
    }

    /// Return a reference to the execution runtime.
    pub fn runtime(&self) -> &Arc<ExecutionRuntime> {
        &self.runtime
    }

    /// Return the output contract from the plan.
    pub fn output_contract(&self) -> &super::plan::types::OutputContract {
        &self.plan.output
    }

    /// Return the plan's fragment DAG.
    pub fn fragment_graph(&self) -> &super::plan::types::FragmentGraph {
        &self.plan.fragments
    }

    /// Execute and materialize all results.
    ///
    /// Valid only when the sink is [`ResultSink::Materialize`].
    /// Panics if the sink has a different variant.
    pub fn execute(&mut self) -> Result<ExecutionResult, QueryError> {
        assert!(
            matches!(self.sink, ResultSink::Materialize),
            "QueryExecutionInstance::execute requires Materialize sink"
        );

        let mut engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Engine already consumed".to_string()))?;
        let chunks = engine.execute()?;
        let dataset = convert_chunks_to_dataset(chunks, None)?;
        Ok(ExecutionResult::DataSet { data: dataset })
    }

    /// Convert to a streaming result handle.
    ///
    /// Valid only when the sink is [`ResultSink::Stream`].
    /// Panics if the sink has a different variant.
    pub fn into_stream(mut self) -> Result<StreamingQueryResult, QueryError> {
        assert!(
            matches!(self.sink, ResultSink::Stream),
            "QueryExecutionInstance::into_stream requires Stream sink"
        );

        let engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Engine already consumed".to_string()))?;
        let stream = engine.into_stream()?;
        Ok(StreamingQueryResult::new(stream, self.runtime.clone()))
    }

    /// Execute with a discard sink (for side-effect-only commands).
    pub fn execute_discard(&mut self) -> Result<(), QueryError> {
        assert!(
            matches!(self.sink, ResultSink::Discard),
            "QueryExecutionInstance::execute_discard requires Discard sink"
        );

        let mut engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Engine already consumed".to_string()))?;
        let _ = engine.execute()?;
        Ok(())
    }

    /// Cancel the running query.
    pub fn cancel(&self) {
        self.runtime.cancel();
    }

    // ── Private helpers ──

    fn create_runtime(
        bindings: &QueryBindings,
        scheduler: Option<MorselWorkerPool>,
    ) -> Result<Arc<ExecutionRuntime>, QueryError> {
        let runtime = ExecutionRuntime::new(
            QueryIdentity {
                query_id: bindings.query_id,
                session_id: None,
                space_name: bindings.space_name.clone(),
            },
            bindings.memory_budget.clone(),
            bindings.storage.clone(),
            #[cfg(feature = "fulltext-search")]
            bindings.fulltext_manager.clone(),
            #[cfg(feature = "qdrant")]
            bindings.vector_coordinator.clone(),
        );

        if let Some(pool) = scheduler {
            runtime.set_worker_pool(Some(pool));
        } else if bindings.max_workers > 1 {
            let pool = MorselWorkerPool::new(bindings.max_workers);
            runtime.set_worker_pool(Some(pool));
        }

        Ok(Arc::new(runtime))
    }
}

impl std::fmt::Debug for QueryExecutionInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryExecutionInstance")
            .field("plan_operators", &self.plan.operator_count())
            .field("fragments", &self.plan.fragment_count())
            .field("sink", &self.sink)
            .finish()
    }
}

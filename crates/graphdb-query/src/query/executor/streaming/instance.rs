//! QueryExecutionInstance: single-entry instantiation point for physical plans.
//!
//! The production entry path:
//!
//! ```text
//! Arc<PhysicalPlan> + QueryBindings + ResultSink
//!     → QueryExecutionInstance::instantiate_plan
//!     → ExecutionRuntime (created once, shared by engine/operators/handle)
//!     → operator tree (materialized from arena plan)
//!     → delivery via ResultSink
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::engine::StreamingExecutionEngine;
use super::plan::materializer::PhysicalPlanMaterializer;
use super::plan::types::PhysicalPlan;
use super::plan::validator::PhysicalPlanValidator;
use super::result_utils::convert_chunks_to_dataset;
use super::runtime::ExecutionRuntime;
use super::stream_result::StreamingQueryResult;
use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::{ExecutionResult, MemoryBudget};
use crate::storage::QueryStorage;
use crate::utils::Arena;

use super::parameters::{ParameterFrame, ParameterSchema};
use super::query_registry::{QueryGuard, QueryId, QueryMetadata, QueryRegistry};
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
///
/// M1.4: carries a [`ParameterFrame`] for slot-based parameter access at
/// execution time, built during validation from the plan's parameter schema.
#[derive(Clone)]
pub struct QueryBindings {
    /// Prepared-statement parameter values (name → value map for validation).
    pub parameters: Arc<HashMap<String, Value>>,
    /// M1.4: slot-indexed parameter frame for hot-path access.
    pub parameter_frame: Option<ParameterFrame>,
    /// Target space name.
    pub space_name: Option<String>,
    /// Storage client for this execution.
    pub storage: Option<Arc<RwLock<dyn QueryStorage>>>,
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
    /// Query text for diagnostics and logging.
    pub query_text: Option<String>,
    /// Session ID for the executing session.
    pub session_id: Option<String>,
    /// User name for the executing session.
    pub user_name: Option<String>,
    /// Transaction scope for this execution.
    pub transaction: TransactionScope,
    /// M6: Engine-level shared scheduler.  When set, all queries share the
    /// same worker pool instead of creating per-query threads.
    pub shared_scheduler: Option<Arc<super::pool::SharedScheduler>>,
    /// Number of partitions for partitioned execution. 0 = non-partitioned.
    pub partition_count: usize,
    /// Optional thread-safe bumpalo arena for executor temporary allocations.
    pub arena: Option<Arc<parking_lot::Mutex<Arena>>>,
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
            parameter_frame: None,
            space_name: context.space_name.clone(),
            storage: context.storage.clone(),
            memory_budget: context.memory_budget.clone(),
            max_workers: context.max_workers,
            chunk_size: context.chunk_size,
            max_buffered_chunks: context.max_buffered_chunks,
            query_id: context.query_id,
            query_text: None,
            session_id: None,
            user_name: None,
            transaction,
            shared_scheduler: context.shared_scheduler.clone(),
            partition_count: 0,
            arena: context.arena.clone(),
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: context.fulltext_manager.clone(),
            #[cfg(feature = "qdrant")]
            vector_coordinator: context.vector_coordinator.clone(),
        }
    }

    /// Build a [`ParameterFrame`] from the plan's parameter schema and the
    /// binding values.  Called after validation during materialization.
    ///
    /// M1.4: produces a slot-indexed frame that operators can read without
    /// string-based lookup.
    pub fn build_parameter_frame(&mut self, schema: &ParameterSchema) {
        let mut values = Vec::with_capacity(schema.params.len());
        for param in &schema.params {
            let value = self
                .parameters
                .get(&param.name)
                .cloned()
                .or_else(|| param.default.clone())
                .unwrap_or(crate::core::Value::Null(Default::default()));
            values.push(value);
        }
        self.parameter_frame = Some(ParameterFrame::new(values));
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
///
/// M2.8: optionally holds a [`QueryGuard`] that unregisters the query
/// from the [`QueryRegistry`] on drop, ensuring no leaked entries.
pub struct QueryExecutionInstance {
    plan: Arc<PhysicalPlan>,
    _bindings: QueryBindings,
    runtime: Arc<ExecutionRuntime>,
    engine: Option<StreamingExecutionEngine>,
    sink: ResultSink,
    /// M2.8: guard that unregisters from the query registry on drop.
    _registry_guard: Option<QueryGuard>,
}

impl QueryExecutionInstance {
    /// Instantiate from a [`PhysicalPlan`] arena (sole production path).
    ///
    /// Uses [`PhysicalPlanMaterializer`] to convert the arena plan into an
    /// operator tree, then wraps it with runtime, engine, and sink.
    ///
    /// M2.8: when a [`QueryRegistry`] is provided, the query is registered
    /// with a unique non-zero ID and the guard is stored in the instance.
    pub fn instantiate_plan(
        plan: Arc<PhysicalPlan>,
        bindings: QueryBindings,
        sink: ResultSink,
        registry: Option<Arc<QueryRegistry>>,
    ) -> Result<Self, QueryError> {
        // Phase 1: validate the plan (structural + binding).
        PhysicalPlanValidator::validate(&plan)?;

        // Phase 2: materialize arena plan → executor tree via materializer.
        let (executor, mut runtime) = PhysicalPlanMaterializer::materialize(&plan, &bindings)?;

        // Phase 3: register with query registry (M2.8).
        let registry_guard = registry.as_ref().map(|reg| {
            let session_id = bindings
                .session_id
                .as_ref()
                .and_then(|s| s.parse::<i64>().ok());
            let meta = QueryMetadata {
                query_id: QueryId(bindings.query_id.max(1)),
                session_id,
                user_name: bindings.user_name.clone(),
                space_name: bindings.space_name.clone(),
                query_text: bindings.query_text.clone(),
                start_time: std::time::Instant::now(),
            };
            let (qid, _token) = reg.register(meta);
            runtime.assign_query_id(qid.as_u64());
            // Arc::get_mut is safe here because runtime was just created
            // and has no other Arc references.
            if let Some(rt) = Arc::get_mut(&mut runtime) {
                rt.set_query_registry(reg.clone(), qid);
            }
            QueryGuard::new(reg.clone(), qid)
        });

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
            _registry_guard: registry_guard,
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
        let chunks = engine.execute_collected()?;
        let dataset =
            convert_chunks_to_dataset(chunks, Some(self.plan.output.output_layout.names()))?;
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
        let stream = engine.execute()?;
        Ok(StreamingQueryResult::new_with_schema(
            stream,
            self.runtime.clone(),
            self.plan.output.output_layout.names(),
        ))
    }

    /// Execute with a discard sink (for side-effect-only commands).
    ///
    /// Uses streaming internally to avoid materializing results that will be discarded.
    pub fn execute_discard(&mut self) -> Result<(), QueryError> {
        assert!(
            matches!(self.sink, ResultSink::Discard),
            "QueryExecutionInstance::execute_discard requires Discard sink"
        );

        let engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Engine already consumed".to_string()))?;
        let mut stream = engine.execute()?;
        while stream.next_chunk()?.is_some() {}
        stream.close()?;
        Ok(())
    }

    /// Cancel the running query.
    pub fn cancel(&self) {
        self.runtime.cancel();
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

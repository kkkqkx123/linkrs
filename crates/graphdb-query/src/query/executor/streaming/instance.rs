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
use crate::query::optimizer::stats::feedback::history::QueryFeedbackHistory;
use crate::query::optimizer::stats::feedback::query::{OperatorFeedback, QueryExecutionFeedback};
use crate::storage::QueryStorage;
use crate::utils::Arena;

use super::parameters::{ParameterFrame, ParameterSchema};
use super::query_registry::{CancelToken, QueryGuard, QueryId, QueryMetadata, QueryRegistry};
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
    /// Snapshot handle pinned by the bound storage (P2: storage boundary).
    ///
    /// Populated by the pipeline when the per-query storage is bound to a
    /// read/auto-commit operation context. `None` for unbound storage.
    pub bound_snapshot: Option<crate::storage::SnapshotHandle>,
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
    /// Request-scoped cancellation token.
    ///
    /// When present it is shared with the query registry and the execution
    /// runtime so `QueryContext::mark_killed`, KILL QUERY, and runtime cancel
    /// all flip the same underlying state.
    pub cancel_token: Option<CancelToken>,
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
    /// Shared query feedback history for collecting execution statistics.
    ///
    /// When set, the execution instance records estimated-vs-actual operator
    /// feedback here after execution completes (stats feedback loop, phase 1).
    /// Injected by the pipeline from the optimizer engine.
    pub feedback_history: Option<Arc<QueryFeedbackHistory>>,
    /// Shared cross-query policy for the typed columnar chunk layout.
    ///
    /// When set, the per-query columnar hit/miss counts are merged back into
    /// the policy when the query finishes (columnar auto-detection, phase 2).
    /// Injected by the pipeline from the optimizer engine.
    pub columnar_policy: Option<Arc<super::chunk::ColumnarPolicy>>,
    #[cfg(feature = "fulltext-search")]
    pub fulltext_manager: Option<Arc<crate::search::manager::FulltextIndexManager>>,
    #[cfg(feature = "qdrant")]
    pub vector_coordinator: Option<Arc<crate::sync::VectorSyncCoordinator>>,
}

impl std::fmt::Debug for QueryBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryBindings")
            .field("parameters", &self.parameters)
            .field("space_name", &self.space_name)
            .field("query_id", &self.query_id)
            .finish_non_exhaustive()
    }
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
            bound_snapshot: context.bound_snapshot,
            memory_budget: context.memory_budget.clone(),
            max_workers: context.max_workers,
            chunk_size: context.chunk_size,
            max_buffered_chunks: context.max_buffered_chunks,
            query_id: context.query_id,
            cancel_token: context.cancel_token.clone(),
            query_text: None,
            session_id: None,
            user_name: None,
            transaction,
            shared_scheduler: context.shared_scheduler.clone(),
            partition_count: 0,
            arena: context.arena.clone(),
            feedback_history: context.feedback_history.clone(),
            columnar_policy: context.columnar_policy.clone(),
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
        let (executor, runtime) = PhysicalPlanMaterializer::materialize(&plan, &bindings)?;

        // Phase 3: adopt the request-scoped cancellation token on the runtime
        // regardless of whether a registry is attached.  This is the
        // single-token convergence point: `QueryContext::mark_killed`, KILL
        // QUERY, and runtime cancel all flip the SAME underlying state, so
        // pipelines without a shared registry (pure streaming / embedded
        // paths) still honor mark_killed here.
        runtime.set_cancel_token(bindings.cancel_token.clone().unwrap_or_default());

        // Phase 4: register with the query registry (M2.8).
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
            // Prefer a request-scoped query id when present so EXPLAIN/PROFILE,
            // streaming, and materialized paths share one identity source;
            // otherwise let the registry allocate a unique id.
            //
            // The external bindings token (request-scoped QueryContext token)
            // is shared with the registry so mark_killed / KILL QUERY / runtime
            // cancel all flip the SAME underlying cancellation state.
            let (qid, token) = if bindings.query_id != 0 {
                reg.register_with_token(
                    QueryId(bindings.query_id),
                    meta,
                    bindings.cancel_token.clone(),
                )
            } else {
                reg.register_with_token(QueryId(0), meta, bindings.cancel_token.clone())
            };
            runtime.assign_query_id(qid.as_u64());
            runtime.set_query_registry(reg.clone(), qid);
            // Re-adopt the registry-canonical token (when no external
            // token was supplied the registry allocated its own), keeping
            // the registry entry and the runtime on one cancellation
            // source.
            runtime.set_cancel_token(token);
            QueryGuard::new(reg.clone(), qid)
        });

        // Phase 5: set up the engine.
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
        self.collect_execution_feedback();
        // Columnar auto-detection: merge this query's hit/miss counts into
        // the shared policy so later queries can adapt.
        self.runtime.flush_columnar_stats_to_policy();
        let dataset =
            convert_chunks_to_dataset(chunks, Some(self.plan.output.output_layout.names()))?;
        Ok(ExecutionResult::DataSet { data: dataset })
    }

    /// Collect estimated-vs-actual execution feedback into the shared
    /// [`QueryFeedbackHistory`] after execution completes.
    ///
    /// Compares the optimizer's per-operator row estimates (written into the
    /// physical operator specs as `estimated_cardinality`) with the runtime
    /// profile counters, and stores one [`QueryExecutionFeedback`] entry
    /// keyed by the plan fingerprint.  Filter operators additionally carry
    /// the normalized predicate key (`condition_key`) so phase 2 of the
    /// statistics feedback loop can correct the selectivity of the specific
    /// condition.
    fn collect_execution_feedback(&self) {
        let Some(history) = self.runtime().feedback_history.clone() else {
            return;
        };
        let profile = self.runtime().profile().flush_to_collector();
        let fingerprint = &self.plan.compatibility.fingerprint;
        let mut feedback =
            QueryExecutionFeedback::new(format!("v{}:{}", fingerprint.version, fingerprint.hash));
        feedback.space = self._bindings.space_name.clone();

        // Query-level estimate: the root operator's estimated cardinality.
        feedback.estimated_rows = self
            .plan
            .fragments
            .get(self.plan.root_fragment)
            .and_then(|fragment| self.plan.operator(fragment.root_operator))
            .and_then(|operator| operator.estimated_cardinality)
            .map(|rows| rows as u64)
            .unwrap_or(0);
        feedback.actual_rows = profile.total_rows;
        feedback.actual_time_us = profile.total_time_us;

        for (key, op_profile) in &profile.operators {
            let op_time_us =
                op_profile.open_time_us + op_profile.next_time_us + op_profile.close_time_us;
            // Track Apply vs SemiJoin rows and time so the decision feedback
            // loop can compare the measured cost of the nested-loop / hash
            // paths of subquery decorrelation.
            match op_profile.name.as_str() {
                "PatternApply" => {
                    feedback.apply_rows += op_profile.output_rows;
                    feedback.apply_time_us += op_time_us;
                }
                "SemiJoin" => {
                    feedback.join_rows += op_profile.output_rows;
                    feedback.join_time_us += op_time_us;
                }
                _ => {}
            }
            if let Some(operator) = self.plan.operator(key.physical_operator_id) {
                if let Some(estimated) = operator.estimated_cardinality {
                    // For filter operators, attach the normalized predicate
                    // key so the feedback loop (phase 2) can correct the
                    // selectivity of the specific condition; all other
                    // cardinality-estimated operators carry a shape key so
                    // the loop can correct their row counts.
                    let condition_key = match &operator.spec {
                        super::plan::types::OperatorKindSpec::Unary(
                            super::operators::spec::UnarySpec::Filter { predicate },
                        ) => Some(crate::query::optimizer::cost::selectivity::condition_key(
                            feedback.space.as_deref(),
                            predicate,
                        )),
                        _ => None,
                    };
                    let shape_key = if condition_key.is_some() {
                        None
                    } else {
                        crate::query::executor::streaming::operators::spec::
                            operator_cardinality_shape_key(feedback.space.as_deref(), &operator.spec)
                    };
                    feedback.add_operator_feedback(OperatorFeedback {
                        operator_id: key.physical_operator_id.0.to_string(),
                        operator_type: op_profile.name.clone(),
                        estimated_rows: estimated as u64,
                        actual_rows: op_profile.output_rows,
                        estimated_time_us: 0,
                        actual_time_us: op_time_us,
                        execution_loops: op_profile.advance_count.max(1),
                        condition_key,
                        shape_key,
                    });
                }
            }
        }

        history.add_feedback(feedback);
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
        let result = StreamingQueryResult::new_with_schema(
            stream,
            self.runtime.clone(),
            self.plan.output.output_layout.names(),
        );
        // Columnar auto-detection: merge this query's hit/miss counts into
        // the shared policy when the last streaming handle is dropped.
        let runtime = self.runtime.clone();
        result.set_on_drop(Box::new(move || runtime.flush_columnar_stats_to_policy()));
        Ok(result)
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
        self.runtime.flush_columnar_stats_to_policy();
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

//! Expression-level subquery execution.
//!
//! EXISTS / IN subqueries at non-conjunctive expression positions (WHERE
//! residual, RETURN, HAVING, WITH assignments, ...) are compiled at planning
//! time by `plan_expression_subqueries` into standalone sub-plans. Each
//! compiled subquery becomes a [`SubqueryRunnerSpec`] on the hosting
//! Filter/Project/Assign arena spec; the materializer instantiates a
//! per-operator [`SubqueryExecutor`] from those specs.
//!
//! Execution model:
//! - Every operator instance owns exactly one [`SubqueryExecutor`]; parallel
//!   partitions never share mutable runner state.
//! - A runner materializes its sub-plan **once** on first use and re-runs it
//!   per row via the reset protocol (`StreamingExecutor::reset`), never
//!   rebuilding the pipeline.
//! - Correlated runners receive the current row as a private correlation
//!   frame (`StreamingExecutor::inject_correlation_frame`); the frame is
//!   injected into the sub-plan's `Argument` source slot.
//! - Non-correlated runners evaluate once and cache the result (EXISTS →
//!   bool, IN → `HashSet<Value>` with NULLs removed).
//!
//! NULL semantics: a NULL left operand or NULL result values never
//! match, consistent with the conjunctive `keys_match` path.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::SubqueryBody;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::instance::QueryBindings;
use crate::query::executor::streaming::plan::materializer::PhysicalPlanMaterializer;
use crate::query::executor::streaming::plan::types::PhysicalPlan;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;
use parking_lot::Mutex;

/// Bundle of per-evaluation-environment state threaded into batch expression
/// evaluation. Replaces the old standalone `params` argument so
/// future operator kinds only need to add their facility to the env.
#[derive(Debug, Clone, Default)]
pub struct EvalEnv {
    /// Query parameter values (`@name` references).
    pub params: Option<Arc<HashMap<String, Value>>>,
    /// Session variable snapshot (`$name` references), captured once per
    /// statement at the API layer.
    pub session_variables: Option<Arc<HashMap<String, Value>>>,
    /// Expression-level subquery executor of the hosting operator, if any.
    pub subquery_executor: Option<Arc<SubqueryExecutor>>,
}

impl EvalEnv {
    /// Build an env carrying only parameter values.
    pub fn from_params(params: Option<Arc<HashMap<String, Value>>>) -> Self {
        Self {
            params,
            session_variables: None,
            subquery_executor: None,
        }
    }
}

/// Immutable subquery config carried by the arena spec.
///
/// Safe to share across parallel partitions: it holds only the compiled
/// sub-plan and the planning-time routing flag. The materializer derives a
/// fresh, per-operator [`SubqueryRunner`] from each spec so partitions never
/// share mutable runner state.
#[derive(Debug, Clone)]
pub struct SubqueryRunnerSpec {
    /// Stable identity assigned at planning time (`SubqueryBody.id`).
    pub id: u64,
    /// Standalone sub-plan (correlated = `Argument`-rooted subtree).
    pub plan: Arc<PhysicalPlan>,
    /// Whether the subquery references outer columns.
    pub correlated: bool,
}

/// Cached result of a non-correlated subquery, evaluated exactly once.
#[derive(Debug)]
enum SubqueryCache {
    /// EXISTS result: whether the subquery produced any row.
    Exists(bool),
    /// IN result set (NULL values removed at collection time).
    Contains(HashSet<Value>),
}

/// A compiled subquery with a reusable, resettable executor instance.
///
/// The executor is materialized once on first use and re-run per row via
/// `reset()`. Interior mutability via `parking_lot::Mutex` keeps the runner
/// `Sync` (executor trees are shipped to parallel partition workers); the
/// lock is never contended because each runner is owned by exactly one
/// operator instance and executed serially inside its `next()` call stack.
#[derive(Debug)]
pub struct SubqueryRunner {
    /// Stable identity assigned at planning time (`SubqueryBody.id`).
    pub id: u64,
    /// Standalone sub-plan (correlated = `Argument`-rooted subtree).
    pub plan: Arc<PhysicalPlan>,
    /// Whether the subquery references outer columns.
    pub correlated: bool,
    /// Materialized once on first use; re-run per row via `reset()`.
    executor: Mutex<Option<StreamingExecutor>>,
    /// Non-correlated results, evaluated once.
    cache: Mutex<Option<SubqueryCache>>,
    /// Whether the last reset used the close+open fallback (EXPLAIN
    /// `reset:fallback` audit).
    pub reset_fallback: Mutex<bool>,
}

impl SubqueryRunner {
    fn from_spec(spec: &SubqueryRunnerSpec) -> Self {
        Self {
            id: spec.id,
            plan: spec.plan.clone(),
            correlated: spec.correlated,
            executor: Mutex::new(None),
            cache: Mutex::new(None),
            reset_fallback: Mutex::new(false),
        }
    }

    /// Materialize the sub-plan once (lazily) and run `f` against the
    /// reused executor. For correlated subqueries the current row is injected
    /// as the correlation frame before each run.
    fn with_executor<F, T>(
        &self,
        runtime: &Arc<ExecutionRuntime>,
        bindings: &Arc<QueryBindings>,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
        f: F,
    ) -> Result<T, ExpressionError>
    where
        F: FnOnce(&mut StreamingExecutor) -> Result<T, QueryError>,
    {
        let mut slot = self.executor.lock();
        if slot.is_none() {
            let (mut exec, _) = PhysicalPlanMaterializer::materialize(&self.plan, bindings)
                .map_err(|e| {
                    ExpressionError::type_error(format!(
                        "Subquery plan materialization failed: {}",
                        e
                    ))
                })?;
            exec.set_chunk_size(bindings.chunk_size);
            exec.set_runtime(Some(runtime.clone()));
            exec.open()
                .map_err(|e| ExpressionError::type_error(format!("Subquery open failed: {}", e)))?;
            *slot = Some(exec);
        }
        let exec = slot.as_mut().ok_or_else(|| {
            ExpressionError::type_error("Subquery executor failed to materialize".to_string())
        })?;
        if self.correlated {
            exec.inject_correlation_frame(layout, row);
        }
        exec.reset()
            .map_err(|e| ExpressionError::type_error(format!("Subquery reset failed: {}", e)))?;
        if exec.base().reset_used_fallback {
            *self.reset_fallback.lock() = true;
        }
        f(exec)
            .map_err(|e| ExpressionError::type_error(format!("Subquery execution failed: {}", e)))
    }
}

/// Per-operator subquery execution facility.
///
/// Each Filter/Project/Assign operator instance holds exactly one executor
/// with its own runner instances; nothing is shared across partitions.
/// Execution is always synchronous and serial within the operator's own
/// `next()` call stack, so the runners need no cross-thread locks.
#[derive(Debug)]
pub struct SubqueryExecutor {
    /// Shared runtime used to re-materialize nested sub-plans (storage,
    /// cancellation, parameters).
    pub runtime: Arc<ExecutionRuntime>,
    /// Bindings used to materialize nested sub-plans (parameter-free).
    pub bindings: Arc<QueryBindings>,
    /// Compiled runners keyed by stable `SubqueryBody.id`.
    pub runners: HashMap<u64, SubqueryRunner>,
}

impl SubqueryExecutor {
    /// Build a per-operator executor from the immutable runner specs of an
    /// arena spec. Each call creates fresh runner state, so every operator
    /// instance (and every parallel partition) is isolated.
    pub fn from_specs(
        runtime: Arc<ExecutionRuntime>,
        bindings: Arc<QueryBindings>,
        specs: &[SubqueryRunnerSpec],
    ) -> Self {
        let runners = specs
            .iter()
            .map(|spec| (spec.id, SubqueryRunner::from_spec(spec)))
            .collect();
        Self {
            runtime,
            bindings,
            runners,
        }
    }

    /// Whether this executor hosts any compiled subqueries.
    pub fn is_empty(&self) -> bool {
        self.runners.is_empty()
    }

    /// EXISTS: whether the subquery produces at least one row.
    ///
    /// Non-correlated: evaluated once on the first call and cached.
    /// Correlated: re-executed per row with the current row as the frame.
    pub fn execute_exists(
        &self,
        body: &SubqueryBody,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
    ) -> Result<bool, ExpressionError> {
        let runner = self.runner(body)?;
        if !runner.correlated {
            let mut cache = runner.cache.lock();
            if let Some(SubqueryCache::Exists(value)) = &*cache {
                return Ok(*value);
            }
            let exists = self.run_exists(runner, layout, row)?;
            *cache = Some(SubqueryCache::Exists(exists));
            return Ok(exists);
        }
        self.run_exists(runner, layout, row)
    }

    /// IN: whether `value` occurs in the subquery result column.
    ///
    /// NULL never matches, mirroring the conjunctive `keys_match`
    /// path. Non-correlated: the result set is collected once (NULLs removed)
    /// into a `HashSet` and probed per row.
    pub fn execute_contains(
        &self,
        body: &SubqueryBody,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
        value: &Value,
    ) -> Result<bool, ExpressionError> {
        if value.is_null() {
            return Ok(false);
        }
        let runner = self.runner(body)?;
        if !runner.correlated {
            let mut cache = runner.cache.lock();
            if let Some(SubqueryCache::Contains(set)) = &*cache {
                return Ok(set.contains(value));
            }
            let set: HashSet<Value> = self
                .run_values(runner, layout, row)?
                .into_iter()
                .filter(|v| !v.is_null())
                .collect();
            let found = set.contains(value);
            *cache = Some(SubqueryCache::Contains(set));
            return Ok(found);
        }
        self.run_contains(runner, layout, row, value)
    }

    fn runner(&self, body: &SubqueryBody) -> Result<&SubqueryRunner, ExpressionError> {
        self.runners.get(&body.id).ok_or_else(|| {
            ExpressionError::type_error(
                "Subquery execution not supported in this context".to_string(),
            )
        })
    }

    /// Run the subquery and short-circuit on the first produced row.
    fn run_exists(
        &self,
        runner: &SubqueryRunner,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
    ) -> Result<bool, ExpressionError> {
        runner.with_executor(&self.runtime, &self.bindings, layout, row, |exec| loop {
            match exec.advance()? {
                Some(chunk) => {
                    if !chunk.rows.is_empty() {
                        return Ok(true);
                    }
                }
                None => return Ok(false),
            }
        })
    }

    /// Run the subquery and probe the produced values against `value`,
    /// short-circuiting on the first match (NULLs never match).
    fn run_contains(
        &self,
        runner: &SubqueryRunner,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
        value: &Value,
    ) -> Result<bool, ExpressionError> {
        runner.with_executor(&self.runtime, &self.bindings, layout, row, |exec| loop {
            match exec.advance()? {
                Some(chunk) => {
                    for chunk_row in &chunk.rows {
                        if let Some(candidate) = chunk_row.first() {
                            if !candidate.is_null() && candidate == value {
                                return Ok(true);
                            }
                        }
                    }
                }
                None => return Ok(false),
            }
        })
    }

    /// Run the subquery and collect the first column of every produced row.
    fn run_values(
        &self,
        runner: &SubqueryRunner,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
    ) -> Result<Vec<Value>, ExpressionError> {
        runner.with_executor(&self.runtime, &self.bindings, layout, row, |exec| {
            let mut values = Vec::new();
            while let Some(mut chunk) = exec.advance()? {
                chunk.materialize_selection();
                for chunk_row in chunk.rows {
                    if let Some(value) = chunk_row.first() {
                        values.push(value.clone());
                    }
                }
            }
            Ok(values)
        })
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::ContextualExpression;
    use crate::query::executor::base::{ExecutionContext, MemoryBudget};
    use crate::query::executor::streaming::plan::arena_builder::PhysicalPlanBuilder;
    use crate::query::executor::streaming::plan::context::PhysicalPlanBuildContext;
    use crate::query::executor::streaming::plan::validator::PhysicalPlanValidator;
    use crate::query::executor::streaming::runtime::QueryIdentity;
    use crate::query::executor::streaming::transaction_scope::TransactionScope;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::control_flow::ArgumentNode;
    use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode;
    use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::query::planning::plan::PlanNodeEnum;
    use std::collections::HashMap;

    fn build_plan(node: &PlanNodeEnum) -> Arc<PhysicalPlan> {
        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
        let plan = Arc::new(
            PhysicalPlanBuilder::build(node, &mut ctx, &exec_ctx).expect("plan should build"),
        );
        PhysicalPlanValidator::validate(&plan).expect("plan should validate");
        plan
    }

    fn contextual(expr: crate::core::Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    /// A subquery plan producing one row per element of `values` in column 0.
    fn values_subquery_plan(values: Vec<Value>) -> Arc<PhysicalPlan> {
        let start = PlanNodeEnum::Start(StartNode::new());
        let list = values
            .into_iter()
            .map(crate::core::Expression::Literal)
            .collect();
        let unwind = UnwindNode::new(start, "v", contextual(crate::core::Expression::list(list)))
            .expect("unwind should build");
        build_plan(&PlanNodeEnum::Unwind(unwind))
    }

    /// A correlated subquery plan that echoes the frame's `x` column.
    fn correlated_echo_plan() -> Arc<PhysicalPlan> {
        let mut argument = ArgumentNode::new(-2, "_arg");
        argument.set_col_names(vec!["x".to_string()]);
        let project = ProjectNode::new(
            PlanNodeEnum::Argument(argument),
            vec![crate::core::YieldColumn::new(
                contextual(crate::core::Expression::variable("x")),
                "x".to_string(),
            )],
        )
        .expect("project should build");
        build_plan(&PlanNodeEnum::Project(project))
    }

    fn test_executor(specs: Vec<SubqueryRunnerSpec>) -> SubqueryExecutor {
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        let bindings = Arc::new(QueryBindings {
            parameters: Arc::new(HashMap::new()),
            session_variables: Arc::new(HashMap::new()),
            parameter_frame: None,
            space_name: None,
            storage: None,
            bound_snapshot: None,
            memory_budget: MemoryBudget::new(1024 * 1024),
            max_workers: 1,
            chunk_size: 2048,
            max_buffered_chunks: 4,
            query_id: 1,
            cancel_token: None,
            session_id: None,
            user_name: None,
            query_text: None,
            transaction: TransactionScope::None,
            shared_scheduler: None,
            partition_count: 0,
            arena: None,
            feedback_history: None,
            columnar_policy: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
        });
        SubqueryExecutor::from_specs(runtime, bindings, &specs)
    }

    fn body(id: u64) -> SubqueryBody {
        SubqueryBody {
            id,
            patterns: Vec::new(),
            where_clause: None,
            return_expr: None,
        }
    }

    fn empty_layout() -> Arc<SlotLayout> {
        Arc::new(SlotLayout::new(vec![]))
    }

    #[test]
    fn non_correlated_exists_caches_and_reuses_executor() {
        let plan = values_subquery_plan(vec![Value::Int(1), Value::Int(2), Value::Int(1)]);
        let executor = test_executor(vec![SubqueryRunnerSpec {
            id: 7,
            plan,
            correlated: false,
        }]);
        let b = body(7);

        let first = executor
            .execute_exists(&b, empty_layout(), Vec::new())
            .expect("exists should run");
        assert!(first, "non-empty subquery exists == true");

        // The runner is materialized exactly once and the result cached.
        let runner = executor.runners.get(&7).expect("runner present");
        assert!(runner.executor.lock().is_some(), "materialized once");
        assert!(
            matches!(&*runner.cache.lock(), Some(SubqueryCache::Exists(true))),
            "EXISTS result cached"
        );

        // Second call hits the cache without re-running.
        let second = executor
            .execute_exists(&b, empty_layout(), Vec::new())
            .expect("cached exists");
        assert!(second);
    }

    #[test]
    fn non_correlated_in_uses_hash_set_cache() {
        let plan = values_subquery_plan(vec![Value::Int(1), Value::Int(2), Value::Int(1)]);
        let executor = test_executor(vec![SubqueryRunnerSpec {
            id: 8,
            plan,
            correlated: false,
        }]);
        let b = body(8);

        assert!(executor
            .execute_contains(&b, empty_layout(), Vec::new(), &Value::Int(1))
            .expect("contains 1"));
        assert!(!executor
            .execute_contains(&b, empty_layout(), Vec::new(), &Value::Int(3))
            .expect("not contains 3"));

        let runner = executor.runners.get(&8).expect("runner present");
        let cache = runner.cache.lock();
        let SubqueryCache::Contains(set) = cache.as_ref().expect("IN result cached") else {
            panic!("IN result must be cached as a HashSet");
        };
        assert_eq!(set.len(), 2, "duplicates and NULLs removed at collection");
    }

    #[test]
    fn null_never_matches_and_skips_the_cache() {
        let plan = values_subquery_plan(vec![
            Value::Int(1),
            Value::Null(crate::core::value::NullType::Null),
        ]);
        let executor = test_executor(vec![SubqueryRunnerSpec {
            id: 9,
            plan,
            correlated: false,
        }]);
        let b = body(9);

        // NULL left operand never matches and must not execute the
        // subquery (the cache stays empty).
        assert!(!executor
            .execute_contains(
                &b,
                empty_layout(),
                Vec::new(),
                &Value::Null(crate::core::value::NullType::Null),
            )
            .expect("null never matches"));
        assert!(executor.runners.get(&9).unwrap().cache.lock().is_none());

        // A non-NULL value matching the non-NULL result element still hits.
        assert!(executor
            .execute_contains(&b, empty_layout(), Vec::new(), &Value::Int(1))
            .expect("value found despite NULL in the result set"));
    }

    #[test]
    fn correlated_runner_reuses_executor_with_per_row_frames() {
        let plan = correlated_echo_plan();
        let executor = test_executor(vec![SubqueryRunnerSpec {
            id: 10,
            plan,
            correlated: true,
        }]);
        let b = body(10);
        let layout = Arc::new(SlotLayout::from_names(&["x".to_string()]));

        // First frame: x = 7.
        assert!(executor
            .execute_contains(&b, layout.clone(), vec![Value::Int(7)], &Value::Int(7),)
            .expect("x=7 contains 7"));
        // Same runner, new frame: x = 8 — the executor was reset, not
        // rebuilt, and the old frame is gone.
        assert!(!executor
            .execute_contains(&b, layout.clone(), vec![Value::Int(8)], &Value::Int(7))
            .expect("x=8 does not contain 7"));
        assert!(executor
            .execute_contains(&b, layout, vec![Value::Int(8)], &Value::Int(8))
            .expect("x=8 contains 8"));

        let runner = executor.runners.get(&10).expect("runner present");
        assert!(
            runner.executor.lock().is_some(),
            "correlated runner materializes once"
        );
        assert!(
            runner.cache.lock().is_none(),
            "correlated results are never cached"
        );
    }

    #[test]
    fn missing_runner_reports_precise_error() {
        let executor = test_executor(Vec::new());
        let err = executor
            .execute_exists(&body(99), empty_layout(), Vec::new())
            .expect_err("no runner for id 99");
        assert!(
            err.message.contains("not supported"),
            "expected the last-resort error, got: {}",
            err.message
        );
    }
}

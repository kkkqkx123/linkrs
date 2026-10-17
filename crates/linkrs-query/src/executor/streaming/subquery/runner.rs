//! Compiled subquery runners: specs, runner state and the reset protocol.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::executor::expression::ExpressionError;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::instance::QueryBindings;
use crate::executor::streaming::plan::materializer::PhysicalPlanMaterializer;
use crate::executor::streaming::plan::types::PhysicalPlan;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::error::QueryError;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::{Expression, Value};
use parking_lot::Mutex;

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
    /// Standalone sub-plan (correlated = `Argument`-rooted subtree; the
    /// Group-Join form = `Aggregate -> Project` build side).
    pub plan: Arc<PhysicalPlan>,
    /// Whether the subquery references outer columns.
    pub correlated: bool,
    /// Group-Join decorrelation info; when set the subquery is answered
    /// from a materialized key→aggregate table instead of re-execution.
    pub group_join: Option<GroupJoinSpec>,
}

/// Group-Join decorrelation info for a scalar aggregate subquery.
#[derive(Debug, Clone)]
pub struct GroupJoinSpec {
    /// Outer-side key expressions, evaluated against the hosting row's
    /// layout.
    pub hash_keys: Vec<Expression>,
    /// Leading group-key column count of each materialized sub-plan row;
    /// the aggregated value follows at that index.
    pub key_columns: usize,
    /// The single aggregate function computed by the sub-plan.
    pub function: AggregateFunction,
    /// DISTINCT flag of the aggregate.
    pub distinct: bool,
}

/// Cached result of a non-correlated subquery, evaluated exactly once.
#[derive(Debug)]
pub(crate) enum SubqueryCache {
    /// EXISTS result: whether the subquery produced any row.
    Exists(bool),
    /// IN result set (NULL values removed at collection time).
    Contains(HashSet<Value>),
    /// Scalar result: first result value, or NULL for empty results.
    Scalar(Value),
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
    /// Group-Join decorrelation info (see [`GroupJoinSpec`]).
    pub group_join: Option<GroupJoinSpec>,
    /// Materialized once on first use; re-run per row via `reset()`.
    pub(crate) executor: Mutex<Option<StreamingExecutor>>,
    /// Non-correlated results, evaluated once.
    pub(crate) cache: Mutex<Option<SubqueryCache>>,
    /// Group-Join lookup table: group key → aggregated value, built once
    /// from a single materialization of the sub-plan.
    pub(crate) group_table: Mutex<Option<HashMap<Vec<Value>, Value>>>,
    /// Whether the last reset used the close+open fallback (EXPLAIN
    /// `reset:fallback` audit).
    pub reset_fallback: Mutex<bool>,
}

impl SubqueryRunner {
    pub(super) fn from_spec(spec: &SubqueryRunnerSpec) -> Self {
        Self {
            id: spec.id,
            plan: spec.plan.clone(),
            correlated: spec.correlated,
            group_join: spec.group_join.clone(),
            executor: Mutex::new(None),
            cache: Mutex::new(None),
            group_table: Mutex::new(None),
            reset_fallback: Mutex::new(false),
        }
    }

    /// Materialize the sub-plan once (lazily) and run `f` against the
    /// reused executor. For correlated subqueries the current row is injected
    /// as the correlation frame before each run.
    pub(super) fn with_executor<F, T>(
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

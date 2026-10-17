//! Per-operator subquery execution facility.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::executor::expression::ExpressionError;
use crate::executor::streaming::context::BorrowedRowContext;
use crate::executor::streaming::instance::QueryBindings;
use crate::executor::streaming::plan::materializer::PhysicalPlanMaterializer;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::types::expr::SubqueryBody;
use linkrs_core::Value;

use super::runner::{GroupJoinSpec, SubqueryCache, SubqueryRunner, SubqueryRunnerSpec};

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
    /// Group-Join form: answered from the materialized key→aggregate table
    /// (the key is present iff at least one right row matched).
    pub fn execute_exists(
        &self,
        body: &SubqueryBody,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
    ) -> Result<bool, ExpressionError> {
        let runner = self.runner(body)?;
        if let Some(spec) = &runner.group_join {
            return self
                .run_group_join_lookup(runner, spec, layout, row)
                .map(|found| found.is_some());
        }
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
    /// into a `HashSet` and probed per row. Group-Join form: the lookup value
    /// is compared against the aggregated result of the row's key.
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
        if let Some(spec) = &runner.group_join {
            let found = self.run_group_join_lookup(runner, spec, layout, row)?;
            return Ok(matches!(found, Some(v) if !v.is_null() && &v == value));
        }
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

    /// Scalar subquery: the first result value, or NULL when the subquery
    /// returns no rows.
    ///
    /// Non-correlated: evaluated once on the first call and cached.
    /// Correlated: re-executed per row with the current row as the frame.
    /// Group-Join form: answered from the materialized key→aggregate table;
    /// a missing key yields NULL.
    pub fn execute_scalar(
        &self,
        body: &SubqueryBody,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
    ) -> Result<Value, ExpressionError> {
        let runner = self.runner(body)?;
        if let Some(spec) = &runner.group_join {
            let found = self.run_group_join_lookup(runner, spec, layout, row)?;
            return Ok(found.unwrap_or(Value::Null(linkrs_core::NullType::Null)));
        }
        if !runner.correlated {
            let mut cache = runner.cache.lock();
            if let Some(SubqueryCache::Scalar(value)) = &*cache {
                return Ok(value.clone());
            }
            let value = self.run_scalar(runner, layout, row)?;
            *cache = Some(SubqueryCache::Scalar(value.clone()));
            return Ok(value);
        }
        self.run_scalar(runner, layout, row)
    }

    /// Run the subquery and return its first result value (NULL when empty).
    fn run_scalar(
        &self,
        runner: &SubqueryRunner,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
    ) -> Result<Value, ExpressionError> {
        let values = self.run_values(runner, layout, row)?;
        Ok(values
            .into_iter()
            .next()
            .unwrap_or(Value::Null(linkrs_core::NullType::Null)))
    }

    fn runner(&self, body: &SubqueryBody) -> Result<&SubqueryRunner, ExpressionError> {
        self.runners.get(&body.id).ok_or_else(|| {
            ExpressionError::type_error(
                "Subquery execution not supported in this context".to_string(),
            )
        })
    }

    /// Group-Join lookup: evaluate the outer-side hash keys against the
    /// current row and probe the materialized key→aggregate table.
    ///
    /// A NULL key component never matches (same convention as the conjunctive
    /// `keys_match` path); a missing key yields `None` (EXISTS → false,
    /// IN → no match).
    fn run_group_join_lookup(
        &self,
        runner: &SubqueryRunner,
        spec: &GroupJoinSpec,
        layout: Arc<SlotLayout>,
        row: Vec<Value>,
    ) -> Result<Option<Value>, ExpressionError> {
        let mut ctx = BorrowedRowContext::new(&row, layout);
        let mut key = Vec::with_capacity(spec.hash_keys.len());
        for expr in &spec.hash_keys {
            let value = ExpressionEvaluator::evaluate(expr, &mut ctx)?;
            if value.is_null() {
                return Ok(None);
            }
            key.push(value);
        }
        self.ensure_group_table(runner, spec)?;
        let table = runner.group_table.lock();
        Ok(table.as_ref().and_then(|t| t.get(&key).cloned()))
    }

    /// Build the Group-Join table once: materialize the sub-plan
    /// (`Aggregate`-rooted build side), drain its rows and index them by the
    /// leading group-key columns.
    fn ensure_group_table(
        &self,
        runner: &SubqueryRunner,
        spec: &GroupJoinSpec,
    ) -> Result<(), ExpressionError> {
        if runner.group_table.lock().is_some() {
            return Ok(());
        }
        let (mut exec, _) = PhysicalPlanMaterializer::materialize(&runner.plan, &self.bindings)
            .map_err(|e| {
                ExpressionError::type_error(format!(
                    "Group-Join subquery plan materialization failed: {}",
                    e
                ))
            })?;
        exec.set_chunk_size(self.bindings.chunk_size);
        exec.set_runtime(Some(self.runtime.clone()));
        exec.open()
            .map_err(|e| ExpressionError::type_error(format!("Subquery open failed: {}", e)))?;

        let mut table: HashMap<Vec<Value>, Value> = HashMap::new();
        while let Some(mut chunk) = exec
            .advance()
            .map_err(|e| ExpressionError::type_error(format!("Subquery execution failed: {}", e)))?
        {
            chunk.normalize_for_opaque("Subquery");
            for row in chunk.rows {
                if row.len() <= spec.key_columns {
                    continue;
                }
                let key: Vec<Value> = row[..spec.key_columns].to_vec();
                // NULL group keys can never be matched by an outer row.
                if key.iter().any(Value::is_null) {
                    continue;
                }
                table.insert(key, row[spec.key_columns].clone());
            }
        }
        *runner.group_table.lock() = Some(table);
        Ok(())
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
                // Visible rows only: an attached selection may leave
                // non-visible physical rows behind.
                Some(chunk) => {
                    if chunk.visible_count() > 0 {
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
                    for chunk_row in chunk.visible_rows() {
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
                chunk.normalize_for_opaque("Subquery");
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

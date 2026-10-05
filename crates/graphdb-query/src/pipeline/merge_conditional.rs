//! Conditional MERGE orchestration.
//!
//! `MERGE` with `ON MATCH` / `ON CREATE` actions plans to a scalar
//! conditional (`LogicalSelectNode`), which the streaming assembler cannot
//! execute. This module implements the missing half at the pipeline layer:
//! probe the merge pattern against the transaction snapshot, then compile
//! and execute exactly one branch with the existing insert/update machinery.
//!
//! The probe and the chosen branch run under the same transaction scope, so
//! existence check and write observe a single snapshot. Branch bodies reuse
//! the planner helpers, so property mapping, map-overwrite, and empty-map
//! clearing behave exactly like standalone `INSERT` / `UPDATE`.

use super::prepared::PreparedRequest;
use super::QueryPipelineManager;
use crate::binder::bound::{BoundMergePattern, BoundStatement};
use crate::binder::expr_converter::bound_expr_to_contextual;
use crate::executor::base::ExecutionResult;
use crate::executor::expression::evaluation_context::default_context::DefaultExpressionContext;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::instance::ResultSink;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{ArgumentNode, PlanNodeEnum};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::logical_plan::LogicalPlan;
use crate::planning::plan::{ExecutionPlan, SubPlan};
use crate::planning::statements::dml::merge_planner::MergePlanner;
use crate::storage::QueryStorage;
use graphdb_core::error::{DBError, DBResult, QueryError};
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::Expression;
use graphdb_core::{DataSet, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Whether the bound statement is a node-pattern `MERGE` carrying actions.
///
/// Only this shape takes the conditional orchestration path: bare merges
/// keep the direct insert path, and edge merges with actions stay explicitly
/// rejected (endpoint resolution needs dataflow the scalar probe cannot see).
pub(crate) fn is_conditional_node_merge(bound: &BoundStatement) -> bool {
    match bound {
        BoundStatement::Merge(merge) => {
            matches!(merge.pattern, BoundMergePattern::Node(_))
                && (!merge.on_match.is_empty() || !merge.on_create.is_empty())
        }
        _ => false,
    }
}

/// Evaluate one expression to a constant value for pipeline bypasses.
///
/// Parameters (`@name`) and session variables (`$name`) resolve from the
/// request; anything needing row input (variables, properties, subqueries)
/// fails loudly so callers never silently substitute nulls.
pub(crate) fn eval_const_expression(
    expr: &Expression,
    parameters: &HashMap<String, Value>,
    session_variables: &HashMap<String, Value>,
) -> DBResult<Value> {
    let mut context = DefaultExpressionContext::new()
        .with_parameters(Arc::new(parameters.clone()))
        .with_session_variables(Arc::new(session_variables.clone()));
    ExpressionEvaluator::evaluate(expr, &mut context)
        .map_err(|error| DBError::from(QueryError::execution(error.to_string())))
}

/// Numeric-width-insensitive equality for merge probes.
///
/// Pattern literals (`Int`) routinely meet stored properties (`BigInt`)
/// from another declaration; width-strict comparison would miss matches the
/// user plainly expressed.
fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::SmallInt(a), Value::SmallInt(b)) => a == b,
        (Value::SmallInt(a), Value::Int(b)) => *a as i64 == *b as i64,
        (Value::SmallInt(a), Value::BigInt(b)) => *a as i64 == *b,
        (Value::Int(a), Value::SmallInt(b)) => *a as i64 == *b as i64,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Int(a), Value::BigInt(b)) => *a as i64 == *b,
        (Value::BigInt(a), Value::SmallInt(b)) => *a == *b as i64,
        (Value::BigInt(a), Value::Int(b)) => *a == *b as i64,
        (Value::BigInt(a), Value::BigInt(b)) => a == b,
        _ => left == right,
    }
}

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    /// Execute a node-pattern `MERGE` with `ON MATCH` / `ON CREATE` actions.
    ///
    /// Probe-then-branch inside one transaction scope: the pattern is
    /// matched by tag plus full property equality against the same snapshot
    /// the branch writes through, then the winning branch (`UPDATE` on the
    /// probed vertex, or `INSERT` plus `ON CREATE` update on the new vertex)
    /// is compiled and executed with the standard plan machinery.
    pub(crate) fn execute_conditional_merge(
        &mut self,
        request: &PreparedRequest,
    ) -> DBResult<ExecutionResult> {
        let merge = match request.bound_statement.as_ref() {
            Some(BoundStatement::Merge(merge)) => merge.clone(),
            _ => {
                return Err(DBError::from(QueryError::execution(
                    "Conditional merge requires a bound MERGE statement".to_string(),
                )));
            }
        };
        let vertex = match &merge.pattern {
            BoundMergePattern::Node(vertex) => vertex.clone(),
            BoundMergePattern::Edge { .. } => {
                return Err(DBError::from(QueryError::execution(
                    "MERGE edge pattern with ON MATCH / ON CREATE is not supported".to_string(),
                )));
            }
        };
        let tag = vertex
            .labels
            .first()
            .ok_or_else(|| {
                DBError::from(QueryError::execution(
                    "MERGE node pattern must have a label".to_string(),
                ))
            })?
            .clone();
        let query_context = request.query_context.clone();
        let space_name = query_context
            .space_name()
            .or_else(|| query_context.request_context().space_name.clone())
            .unwrap_or_else(|| "default".to_string());
        let request_context = query_context.request_context();
        let parameters = request_context.parameters.clone();
        let session_variables = request_context.session_variables.clone();

        let mut pattern_props = Vec::new();
        if let Some(props) = &vertex.properties {
            for (name, bound_expr) in props {
                let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
                let ctx_expr = bound_expr_to_contextual(bound_expr, &expr_ctx).map_err(|e| {
                    DBError::from(QueryError::execution(format!(
                        "MERGE pattern property '{name}' cannot be resolved: {e}"
                    )))
                })?;
                let expression = ctx_expr.get_expression().ok_or_else(|| {
                    DBError::from(QueryError::execution(format!(
                        "MERGE pattern property '{name}' has no evaluable expression"
                    )))
                })?;
                let value = eval_const_expression(&expression, &parameters, &session_variables)
                    .map_err(|e| {
                        DBError::from(QueryError::execution(format!(
                            "MERGE pattern property '{name}' must be a constant expression: {e}"
                        )))
                    })?;
                pattern_props.push((name.clone(), value));
            }
        }

        let exec_ctx = self.build_execution_context(&query_context);
        let storage = exec_ctx.storage.clone().ok_or_else(|| {
            DBError::from(QueryError::execution(
                "Conditional merge requires a storage binding".to_string(),
            ))
        })?;
        let matched = {
            let reader = storage.read();
            let candidates = reader
                .scan_vertices_by_tag(&space_name, &tag)
                .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;
            candidates
                .into_iter()
                .find(|vertex| {
                    pattern_props.iter().all(|(name, value)| {
                        vertex
                            .tag
                            .properties
                            .get(name.as_str())
                            .is_some_and(|actual| values_equal(actual, value))
                    })
                })
                .map(|vertex| vertex.vid)
        };

        let scope = request.transaction_scope.clone();
        let planner = MergePlanner::new();
        let merged = match matched {
            Some(vid) => {
                if merge.on_match.is_empty() {
                    0
                } else {
                    let branch_ctx = Arc::new(ExpressionAnalysisContext::new());
                    let root = MergePlanner::build_node_update_root(
                        &merge.on_match,
                        space_name.clone(),
                        tag.clone(),
                        Value::from(vid),
                        &branch_ctx,
                    )
                    .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;
                    self.execute_merge_branch(root, &query_context, &space_name, scope)?;
                    1
                }
            }
            None => {
                let branch_ctx = Arc::new(ExpressionAnalysisContext::new());
                let (insert_root, vid_expr) = planner
                    .build_node_insert_parts(&vertex, space_name.clone(), &branch_ctx)
                    .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;
                self.execute_merge_branch(insert_root, &query_context, &space_name, scope.clone())?;
                if !merge.on_create.is_empty() {
                    let vid = eval_merge_vid(&vid_expr, &parameters, &session_variables)?;
                    let update_ctx = Arc::new(ExpressionAnalysisContext::new());
                    let root = MergePlanner::build_node_update_root(
                        &merge.on_create,
                        space_name.clone(),
                        tag.clone(),
                        vid,
                        &update_ctx,
                    )
                    .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;
                    self.execute_merge_branch(root, &query_context, &space_name, scope)?;
                }
                1
            }
        };
        Ok(ExecutionResult::DataSet {
            data: DataSet::from_rows(
                vec![vec![Value::BigInt(merged)]],
                vec!["merged".to_string()],
            ),
        })
    }

    /// Compile and execute one merge branch root with the standard plan
    /// machinery (optimize, build, execute) under the merge scope.
    ///
    /// Branch plans intentionally bypass the plan cache: the winning branch
    /// depends on runtime probe state, so a cached plan for this text could
    /// serve the stale branch.
    fn execute_merge_branch(
        &mut self,
        branch_root: LogicalNodeEnum,
        query_context: &Arc<crate::QueryContext>,
        space_name: &str,
        scope: crate::executor::streaming::transaction_scope::TransactionScope,
    ) -> DBResult<()> {
        let mut sub_plan = SubPlan::from_logical_root(branch_root);
        sub_plan.set_tail(PlanNodeEnum::Argument(ArgumentNode::new(
            next_node_id(),
            "merge_branch",
        )));
        let root = sub_plan.root().clone();
        let mut execution_plan = ExecutionPlan::new(root);
        if let Some(logical_root) = sub_plan.logical_root().cloned() {
            execution_plan.set_logical_plan(LogicalPlan::new(logical_root));
        }
        let optimized = self.optimize_execution_plan(execution_plan, Some(space_name))?;
        let physical = self.build_physical_plan(&optimized, query_context)?;
        let result = self.execute_compiled_with_scope(
            physical,
            query_context.clone(),
            ResultSink::Materialize,
            scope,
        )?;
        match result {
            ExecutionResult::DataSet { .. } | ExecutionResult::Empty | ExecutionResult::Success => {
                Ok(())
            }
            ExecutionResult::Error(message) => Err(DBError::from(QueryError::execution(message))),
            ExecutionResult::SpaceSwitched(_)
            | ExecutionResult::ConfigUpdate { .. }
            | ExecutionResult::ShowConfigs { .. } => Err(DBError::from(QueryError::execution(
                "Merge branch produced an unexpected execution result".to_string(),
            ))),
        }
    }
}

/// Evaluate the vertex-id expression of a merge insert to the concrete id.
///
/// The expression is a plan-time literal (explicit pattern id or the
/// generated fallback), so evaluating it here yields exactly the id the
/// insert branch stored.
fn eval_merge_vid(
    vid_expr: &ContextualExpression,
    parameters: &HashMap<String, Value>,
    session_variables: &HashMap<String, Value>,
) -> DBResult<Value> {
    let expression = vid_expr.get_expression().ok_or_else(|| {
        DBError::from(QueryError::execution(
            "MERGE insert produced no evaluable vertex id".to_string(),
        ))
    })?;
    eval_const_expression(&expression, parameters, session_variables)
}

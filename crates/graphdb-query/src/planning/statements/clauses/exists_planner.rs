//! EXISTS / IN subquery planning for conjunctive WHERE positions.
//!
//! `WHERE EXISTS / NOT EXISTS / IN / NOT IN (subquery)` conjuncts are
//! planned as [`PatternApply`](crate::planning::plan::core::nodes::PatternApplyNode)
//! nodes over the input plan. Correlated equality keys are split per side:
//! `hash_keys` are evaluated against the outer (left) layout and `probe_keys`
//! against the subquery (right) layout, mirroring the `SemiJoinNode`
//! convention so the decorrelation pass can rewrite the apply into a
//! semi / anti join.
//!
//! Non-equi correlation (e.g. `p.age > t.age`) is planned as a
//! [`CorrelatedApplyNode`] that re-executes the right subtree per outer row
//! with the outer row bound as the correlation frame.

mod exists_types;
mod exists_utils;
mod wrap;

use std::collections::HashSet;
use std::sync::Arc;

use crate::binder::validation::ValidationInfo;
use crate::optimizer::cost_based::subquery_unnesting::SubqueryUnnestingOptimizer;
use crate::parser::ast::pattern::PatternUtils;
use crate::planning::planner::PlannerError;
use crate::planning::statements::pattern_planner::{self, PlanningContext};
use crate::planning::statements::plan_combiner;
use crate::QueryContext;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::types::{ContextualExpression, Expression};

pub use exists_types::{
    collect_expression_subqueries, extract_conjunctive_exists, is_trivially_true, ExistsSpec,
    PlannedGroupJoin, PlannedSubquery, SubqueryIdAllocator,
};
pub(crate) use exists_utils::to_contextual;

pub(crate) use exists_utils::{
    and_join, collect_and_conjuncts, extract_keys, split_correlated, wrap_filter,
    wrap_filter_with_subqueries, wrap_project_with_subqueries,
};

/// Unified planning entry for EXISTS / IN at any expression position
/// (WHERE residual, HAVING, RETURN, WITH assignments, ...).
pub fn plan_expression_subqueries(
    expr: Expression,
    qctx: &Arc<QueryContext>,
    space_id: u64,
    space_name: &str,
    outer_col_names: &[String],
    id_alloc: &mut SubqueryIdAllocator,
) -> Result<(Expression, Vec<PlannedSubquery>), PlannerError> {
    let mut expr = expr;
    let bodies = collect_expression_subqueries(&mut expr, id_alloc);
    let mut planned = Vec::with_capacity(bodies.len());
    for body in &bodies {
        match plan_scalar_subquery(body, qctx, space_id, space_name, outer_col_names, id_alloc) {
            Ok(subquery) => planned.push(subquery),
            Err(error) => {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "EXISTS/IN subquery cannot be planned in this position ({error}); \
                     move it to a conjunctive WHERE condition, e.g. \
                     `WHERE cond AND EXISTS {{ ... }}`"
                )))
            }
        }
    }
    Ok((expr, planned))
}

/// Convenience rejection entry for call sites whose hosting operator does
/// not yet support expression-level subqueries.
pub fn check_expression_subqueries(
    expr: &Expression,
    _qctx: &Arc<QueryContext>,
    _space_id: u64,
    _space_name: &str,
    _outer_col_names: &[String],
) -> Result<(), PlannerError> {
    let mut id_alloc = SubqueryIdAllocator::new();
    let mut cloned = expr.clone();
    let bodies = collect_expression_subqueries(&mut cloned, &mut id_alloc);
    if bodies.is_empty() {
        return Ok(());
    }
    Err(expression_subquery_position_error())
}

/// Compile every expression-level EXISTS / IN inside a contextual expression
/// into a standalone sub-plan.
pub fn plan_contextual_subqueries(
    ctx_expr: &mut ContextualExpression,
    qctx: &Arc<QueryContext>,
    space_id: u64,
    space_name: &str,
    outer_col_names: &[String],
    id_alloc: &mut SubqueryIdAllocator,
) -> Result<Vec<PlannedSubquery>, PlannerError> {
    let Some(expr_meta) = ctx_expr.expression() else {
        return Ok(Vec::new());
    };
    let (planned_expr, subqueries) = plan_expression_subqueries(
        expr_meta.inner().clone(),
        qctx,
        space_id,
        space_name,
        outer_col_names,
        id_alloc,
    )?;
    if subqueries.is_empty() {
        return Ok(Vec::new());
    }
    let ctx = ctx_expr.context();
    let new_id =
        ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(planned_expr));
    *ctx_expr = ContextualExpression::new(new_id, ctx.clone());
    Ok(subqueries)
}

/// The precise planning-time error for expression-level EXISTS / IN on
/// hosts that are not yet wired.
fn expression_subquery_position_error() -> PlannerError {
    PlannerError::PlanGenerationFailed(
        "EXISTS/IN subquery cannot be planned in this position \
         (expression-level subquery execution is not yet supported here); \
         move it to a conjunctive WHERE condition, e.g. \
         `WHERE cond AND EXISTS { ... }`"
            .to_string(),
    )
}

/// Compile a single expression-level EXISTS / IN body into a standalone
/// sub-plan.
fn plan_scalar_subquery(
    body: &graphdb_core::types::expr::SubqueryBody,
    qctx: &Arc<QueryContext>,
    space_id: u64,
    space_name: &str,
    outer_col_names: &[String],
    id_alloc: &mut SubqueryIdAllocator,
) -> Result<PlannedSubquery, PlannerError> {
    let mut patterns = Vec::with_capacity(body.patterns.len());
    for pattern_str in &body.patterns {
        let pattern = crate::parser::parsing::TraversalParser::new()
            .parse_pattern(&mut crate::parser::ParseContext::new(pattern_str))
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!(
                    "Invalid subquery pattern `{pattern_str}`: {e}"
                ))
            })?;
        patterns.push(pattern);
    }

    let inner_vars: HashSet<String> = patterns
        .iter()
        .flat_map(PatternUtils::find_variables)
        .collect();

    let mut conditions: Vec<Expression> = Vec::new();
    if let Some(where_expr) = &body.where_clause {
        collect_and_conjuncts(where_expr, &mut conditions);
    }

    let mut nested_specs = Vec::new();
    let mut flat_conditions = Vec::new();
    for condition in &conditions {
        flat_conditions.push(extract_conjunctive_exists(condition, &mut nested_specs));
    }

    let return_expr = body.return_expr.as_ref().map(|e| e.as_ref().clone());
    let return_correlated = return_expr
        .as_ref()
        .is_some_and(|e| e.get_variables().iter().any(|v| !inner_vars.contains(v)));

    let aggregate_return = match &return_expr {
        Some(Expression::Aggregate {
            func,
            args,
            distinct,
            filter: None,
        }) if args.len() == 1 && !contains_expression_subquery(&args[0]) => {
            Some((*func, args[0].clone(), *distinct))
        }
        _ => None,
    };

    let try_group_join =
        aggregate_return.is_some() && nested_specs.is_empty() && !return_correlated;
    let (hash_key_exprs, probe_key_exprs, residual_conditions) = if try_group_join {
        extract_keys(&flat_conditions, &inner_vars)?
    } else {
        (Vec::new(), Vec::new(), flat_conditions)
    };
    let (inner_residual, correlated_residual) = split_correlated(&residual_conditions, &inner_vars);
    let mut correlated_residual = correlated_residual;

    let expr_context = Arc::new(ExpressionAnalysisContext::new());
    let validation_info = ValidationInfo::new();
    let planning_ctx = PlanningContext {
        space_id,
        space_name,
        validation_info: &validation_info,
        qctx,
        enable_index_optimization: false,
        metadata_context: &None,
        expr_context: &Some(expr_context.clone()),
        where_expression: None,
    };

    let mut sub_plan = if patterns.is_empty() {
        pattern_planner::plan_node_pattern(space_id, space_name)?
    } else {
        let mut plan = pattern_planner::plan_path_pattern(&patterns[0], &planning_ctx)?;
        for pattern in patterns.iter().skip(1) {
            let path_plan = pattern_planner::plan_path_pattern(pattern, &planning_ctx)?;
            plan = plan_combiner::cross_join_plans(plan, path_plan)?;
        }
        plan
    };

    for nested in &nested_specs {
        let nested_outer = sub_plan
            .root()
            .as_ref()
            .map(|root| root.col_names().to_vec())
            .unwrap_or_default();
        let planned = plan_subquery(nested, qctx, space_id, space_name, &nested_outer)?;
        sub_plan = if let Some(condition) = &planned.mark_join_condition {
            wrap_mark_join(sub_plan, &planned, condition, nested.negated)?
        } else if planned.correlated {
            wrap_correlated_apply(sub_plan, &planned, nested.negated)?
        } else {
            wrap_pattern_apply(sub_plan, &planned, nested.negated)?
        };
    }

    if !inner_residual.is_empty() && !is_trivially_true(&and_join(&inner_residual)) {
        let (planned_residual, residual_subqueries) = plan_expression_subqueries(
            and_join(&inner_residual),
            qctx,
            space_id,
            space_name,
            outer_col_names,
            id_alloc,
        )?;
        sub_plan = wrap_filter_with_subqueries(
            sub_plan,
            to_contextual(planned_residual, &expr_context),
            residual_subqueries,
        )?;
    }

    let mut group_join = None;
    if let Some((func, agg_arg, distinct)) = aggregate_return.clone() {
        let eligible = !hash_key_exprs.is_empty()
            && correlated_residual.is_empty()
            && sub_plan
                .root()
                .as_ref()
                .is_some_and(SubqueryUnnestingOptimizer::is_mark_join_shape);
        if eligible {
            sub_plan = build_group_join_right_subtree(
                sub_plan,
                &probe_key_exprs,
                func,
                &agg_arg,
                distinct,
                &expr_context,
            )?;
            group_join = Some(PlannedGroupJoin {
                hash_keys: hash_key_exprs,
                key_columns: probe_key_exprs.len(),
                function: func,
                distinct,
            });
        } else if !hash_key_exprs.is_empty() {
            let mut restored: Vec<Expression> = hash_key_exprs
                .iter()
                .zip(probe_key_exprs.iter())
                .map(|(h, p)| Expression::binary(h.clone(), BinaryOperator::Equal, p.clone()))
                .collect();
            restored.extend(inner_residual.iter().cloned());
            restored.extend(correlated_residual.iter().cloned());
            let (inner_restored, correlated_restored) = split_correlated(&restored, &inner_vars);
            if !inner_restored.is_empty() && !is_trivially_true(&and_join(&inner_restored)) {
                sub_plan = wrap_filter(
                    sub_plan,
                    to_contextual(and_join(&inner_restored), &expr_context),
                )?;
            }
            correlated_residual = correlated_restored;
        }
    }

    let correlated = !correlated_residual.is_empty() || return_correlated;
    if correlated {
        sub_plan = build_correlated_right_subtree(sub_plan, &correlated_residual, outer_col_names)?;
    }

    if let Some(return_expr) = return_expr {
        if group_join.is_none() {
            let (planned_return, return_subqueries) = plan_expression_subqueries(
                return_expr,
                qctx,
                space_id,
                space_name,
                outer_col_names,
                id_alloc,
            )?;
            sub_plan = wrap_project_with_subqueries(
                sub_plan,
                to_contextual(planned_return, &expr_context),
                return_subqueries,
            )?;
        }
    }

    Ok(PlannedSubquery {
        id: body.id,
        plan: Box::new(sub_plan),
        hash_keys: Vec::new(),
        probe_keys: Vec::new(),
        correlated,
        mark_join_condition: None,
        group_join,
    })
}

/// Whether `expr` contains an expression-level EXISTS / IN anywhere.
fn contains_expression_subquery(expr: &Expression) -> bool {
    match expr {
        Expression::Exists { .. } | Expression::In { .. } | Expression::ScalarSubquery { .. } => {
            true
        }
        _ => expr
            .children()
            .iter()
            .any(|c| contains_expression_subquery(c)),
    }
}

/// Plan a single EXISTS / IN spec against the outer plan.
pub fn plan_subquery(
    spec: &ExistsSpec,
    qctx: &Arc<QueryContext>,
    space_id: u64,
    space_name: &str,
    outer_col_names: &[String],
) -> Result<PlannedSubquery, PlannerError> {
    let mut patterns = Vec::with_capacity(spec.body.patterns.len());
    for pattern_str in &spec.body.patterns {
        let pattern = crate::parser::parsing::TraversalParser::new()
            .parse_pattern(&mut crate::parser::ParseContext::new(pattern_str))
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!(
                    "Invalid subquery pattern `{pattern_str}`: {e}"
                ))
            })?;
        patterns.push(pattern);
    }

    let inner_vars: HashSet<String> = patterns
        .iter()
        .flat_map(PatternUtils::find_variables)
        .collect();

    let mut conditions: Vec<Expression> = Vec::new();
    if let Some(where_expr) = &spec.body.where_clause {
        collect_and_conjuncts(where_expr, &mut conditions);
    }
    let mut in_equality: Option<Expression> = None;
    if let Some(left_expr) = &spec.left_expr {
        let return_expr = spec.body.return_expr.as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "IN subquery requires a RETURN expression".to_string(),
            )
        })?;
        let equality = Expression::binary(
            left_expr.clone(),
            BinaryOperator::Equal,
            return_expr.as_ref().clone(),
        );
        in_equality = Some(equality.clone());
        conditions.push(equality);
    }

    let mut nested_specs = Vec::new();
    let mut flat_conditions = Vec::new();
    for condition in &conditions {
        flat_conditions.push(extract_conjunctive_exists(condition, &mut nested_specs));
    }

    let (hash_key_exprs, probe_key_exprs, residual_conditions) =
        extract_keys(&flat_conditions, &inner_vars)?;
    let (inner_residual, correlated_residual) = split_correlated(&residual_conditions, &inner_vars);

    let expr_context = Arc::new(ExpressionAnalysisContext::new());
    let validation_info = ValidationInfo::new();
    let planning_ctx = PlanningContext {
        space_id,
        space_name,
        validation_info: &validation_info,
        qctx,
        enable_index_optimization: false,
        metadata_context: &None,
        expr_context: &Some(expr_context.clone()),
        where_expression: None,
    };

    let mut sub_plan = if patterns.is_empty() {
        pattern_planner::plan_node_pattern(space_id, space_name)?
    } else {
        let mut plan = pattern_planner::plan_path_pattern(&patterns[0], &planning_ctx)?;
        for pattern in patterns.iter().skip(1) {
            let path_plan = pattern_planner::plan_path_pattern(pattern, &planning_ctx)?;
            plan = plan_combiner::cross_join_plans(plan, path_plan)?;
        }
        plan
    };

    for nested in &nested_specs {
        let nested_outer = sub_plan
            .root()
            .as_ref()
            .map(|root| root.col_names().to_vec())
            .unwrap_or_default();
        let planned = plan_subquery(nested, qctx, space_id, space_name, &nested_outer)?;
        sub_plan = if let Some(condition) = &planned.mark_join_condition {
            wrap_mark_join(sub_plan, &planned, condition, nested.negated)?
        } else if planned.correlated {
            wrap_correlated_apply(sub_plan, &planned, nested.negated)?
        } else {
            wrap_pattern_apply(sub_plan, &planned, nested.negated)?
        };
    }

    if !inner_residual.is_empty() && !is_trivially_true(&and_join(&inner_residual)) {
        let residual_expr = and_join(&inner_residual);
        sub_plan = wrap_filter(sub_plan, to_contextual(residual_expr, &expr_context))?;
    }

    let hash_keys = hash_key_exprs
        .into_iter()
        .map(|e| to_contextual(e, &expr_context))
        .collect();
    let probe_keys = probe_key_exprs
        .into_iter()
        .map(|e| to_contextual(e, &expr_context))
        .collect();

    let correlated = !correlated_residual.is_empty();
    let mut mark_join_condition = None;
    if correlated {
        let mark_joinable = sub_plan.root().as_ref().is_some_and(|root| {
            SubqueryUnnestingOptimizer::is_mark_join_shape(root)
                && !SubqueryUnnestingOptimizer::contains_aggregation(root)
        });
        if mark_joinable {
            let mut conditions = correlated_residual;
            if let Some(equality) = in_equality {
                conditions.push(equality);
            }
            mark_join_condition = Some(to_contextual(and_join(&conditions), &expr_context));
        } else {
            let mut correlated_conditions = correlated_residual;
            if let Some(equality) = in_equality {
                correlated_conditions.push(equality);
            }
            sub_plan =
                build_correlated_right_subtree(sub_plan, &correlated_conditions, outer_col_names)?;
        }
    }

    Ok(PlannedSubquery {
        id: spec.body.id,
        plan: Box::new(sub_plan),
        hash_keys: if correlated { Vec::new() } else { hash_keys },
        probe_keys: if correlated { Vec::new() } else { probe_keys },
        correlated: correlated && mark_join_condition.is_none(),
        mark_join_condition,
        group_join: None,
    })
}

pub use wrap::{
    build_correlated_right_subtree, build_group_join_right_subtree, wrap_correlated_apply,
    wrap_mark_join, wrap_pattern_apply,
};

#[cfg(test)]
mod tests;

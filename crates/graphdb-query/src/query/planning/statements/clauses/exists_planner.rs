//! EXISTS / IN subquery planning for conjunctive WHERE positions.
//!
//! `WHERE EXISTS / NOT EXISTS / IN / NOT IN (subquery)` conjuncts are
//! planned as [`PatternApply`](crate::query::planning::plan::core::nodes::PatternApplyNode)
//! nodes over the input plan. Correlated equality keys are split per side:
//! `hash_keys` are evaluated against the outer (left) layout and `probe_keys`
//! against the subquery (right) layout, mirroring the `SemiJoinNode`
//! convention so the decorrelation pass can rewrite the apply into a
//! semi / anti join.
//!
//! Non-equi correlation (e.g. `p.age > t.age`) is planned as a
//! [`CorrelatedApplyNode`] that re-executes the right subtree per outer row
//! with the outer row bound as the correlation frame.

use std::collections::HashSet;
use std::sync::Arc;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::core::types::expr::ExpressionMeta;
use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::core::types::{ContextualExpression, Expression};
use crate::query::binder::validation::ValidationInfo;
use crate::query::optimizer::cost_based::subquery_unnesting::SubqueryUnnestingOptimizer;
use crate::query::parser::ast::pattern::PatternUtils;
use crate::query::planning::plan::core::next_node_id;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::control_flow::ArgumentNode;
use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::CorrelatedApplyNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode;
use crate::query::planning::plan::core::nodes::join::join_node::SemiJoinNode;
use crate::query::planning::plan::core::nodes::join::CrossJoinNode;
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::query::planning::plan::logical::logical_nodes::graph_ops::LogicalCorrelatedApplyNode;
use crate::query::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode;
use crate::query::planning::plan::logical::logical_nodes::join::LogicalCrossJoinNode;
use crate::query::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode;
use crate::query::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
use crate::query::planning::plan::logical::logical_nodes::operation::LogicalProjectNode;
use crate::query::planning::plan::logical::LogicalNodeEnum;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::planning::statements::pattern_planner::{self, PlanningContext};
use crate::query::planning::statements::plan_combiner;
use crate::query::QueryContext;

/// An EXISTS / IN subquery at a conjunctive WHERE position.
#[derive(Debug, Clone)]
pub struct ExistsSpec {
    /// The subquery body (patterns + WHERE + optional RETURN).
    pub body: crate::core::types::expr::SubqueryBody,
    /// NOT EXISTS / NOT IN.
    pub negated: bool,
    /// IN's left-hand expression (`None` for EXISTS).
    pub left_expr: Option<Expression>,
}

/// A planned subquery: the subquery plan plus its correlated keys.
#[derive(Debug, Clone)]
pub struct PlannedSubquery {
    /// Stable identity (`SubqueryBody.id`) assigned by the id allocator; the
    /// runtime dispatches on it to find the compiled runner.
    /// The conjunctive path (`plan_subquery`) leaves it 0 — those subqueries
    /// never materialize runners.
    pub id: u64,
    /// The subquery plan (pattern scans + subquery-local filters, or the
    /// `Filter -> CrossJoin -> Argument` correlated right subtree).
    ///
    /// Boxed so plan nodes can carry `Vec<PlannedSubquery>` without an
    /// infinite-size type cycle (SubPlan roots embed `PlanNodeEnum` by value).
    pub plan: Box<SubPlan>,
    /// Outer-side (left layout) key expressions.
    pub hash_keys: Vec<ContextualExpression>,
    /// Subquery-side (right layout) key expressions.
    pub probe_keys: Vec<ContextualExpression>,
    /// True when the subquery is planned as a `CorrelatedApply` (per-row
    /// re-execution over an `Argument` frame) instead of a key-based
    /// `PatternApply`. This is a planning-time routing flag only.
    pub correlated: bool,
    /// When set, the non-equi correlated subquery is decorrelated as a
    /// Mark-Join (a `SemiJoin` carrying the correlated residual as its join
    /// condition) instead of a per-row `CorrelatedApply`. The caller wraps
    /// the join with [`wrap_mark_join`].
    pub mark_join_condition: Option<ContextualExpression>,
    /// When set, the correlated scalar aggregate subquery
    /// (`RETURN agg(...) WHERE corr = outer.x`) is decorrelated as a
    /// Group-Join: the right subtree pre-aggregates per probe key and the
    /// runtime backfills the outer row by hash-key lookup.
    pub group_join: Option<PlannedGroupJoin>,
}

/// Group-Join decorrelation info for a scalar aggregate correlated subquery.
///
/// The planned right subtree ends in `Project(probe keys) -> Aggregate`
/// whose output rows are `[group_key..., agg_value]`; the runtime
/// materializes them once into a `HashMap<Vec<Value>, Value>` keyed by the
/// group key and answers per-row lookups with the outer-side
/// [`PlannedGroupJoin::hash_keys`] expressions.
#[derive(Debug, Clone)]
pub struct PlannedGroupJoin {
    /// Outer-side key expressions, evaluated against the hosting row's
    /// layout (same convention as `PatternApply.hash_keys`).
    pub hash_keys: Vec<Expression>,
    /// Number of leading group-key columns in each materialized row; the
    /// aggregated value follows at that index.
    pub key_columns: usize,
    /// The single aggregate function computed by the right subtree.
    pub function: AggregateFunction,
    /// DISTINCT flag of the aggregate.
    pub distinct: bool,
}

/// Extracted keys and residual conditions of a subquery.
type ExtractedKeys = (Vec<Expression>, Vec<Expression>, Vec<Expression>);

/// Walk the AND-tree of `expr`, collect every conjunctive EXISTS/IN into
/// `specs`, and rebuild the condition with `true` substituted at the
/// extraction sites.
pub fn extract_conjunctive_exists(expr: &Expression, specs: &mut Vec<ExistsSpec>) -> Expression {
    match expr {
        Expression::Binary {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let left_res = extract_conjunctive_exists(left, specs);
            let right_res = extract_conjunctive_exists(right, specs);
            Expression::binary(left_res, BinaryOperator::And, right_res)
        }
        Expression::Exists { body } => {
            specs.push(ExistsSpec {
                body: body.as_ref().clone(),
                negated: false,
                left_expr: None,
            });
            Expression::literal(true)
        }
        Expression::In {
            expr,
            subquery,
            negated,
        } => {
            specs.push(ExistsSpec {
                body: subquery.as_ref().clone(),
                negated: *negated,
                left_expr: Some(expr.as_ref().clone()),
            });
            Expression::literal(true)
        }
        // `NOT EXISTS { … }` / `NOT (x IN { … })` parse as a `NOT` prefix.
        Expression::Unary {
            op: UnaryOperator::Not,
            operand,
        } => match operand.as_ref() {
            Expression::Exists { body } => {
                specs.push(ExistsSpec {
                    body: body.as_ref().clone(),
                    negated: true,
                    left_expr: None,
                });
                Expression::literal(true)
            }
            Expression::In {
                expr,
                subquery,
                negated,
            } => {
                specs.push(ExistsSpec {
                    body: subquery.as_ref().clone(),
                    negated: !*negated,
                    left_expr: Some(expr.as_ref().clone()),
                });
                Expression::literal(true)
            }
            _ => expr.clone(),
        },
        _ => expr.clone(),
    }
}

/// Whether the (residual) condition is a plain `true` AND-chain and thus
/// needs no filter node.
pub fn is_trivially_true(expr: &Expression) -> bool {
    match expr {
        Expression::Literal(crate::core::Value::Bool(true)) => true,
        Expression::Binary {
            left,
            op: BinaryOperator::And,
            right,
        } => is_trivially_true(left) && is_trivially_true(right),
        _ => false,
    }
}

/// Monotonically increasing allocator for stable `SubqueryBody.id`s within a
/// single query planning pass. Re-planning the same AST with a fresh
/// allocator re-assigns ids from 0.
#[derive(Debug, Default)]
pub struct SubqueryIdAllocator {
    next: u64,
}

impl SubqueryIdAllocator {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Allocate the next stable id.
    pub fn allocate(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }
}

/// Collect every EXISTS / IN subquery at expression level of `expr`,
/// assigning each body a stable id from `id_alloc` (in place).
///
/// The walker covers every nested container — binary/unary operators, CASE,
/// lists, maps, type casts, subscripts, ranges, paths, list comprehensions,
/// function/aggregate arguments, reduce, window functions, label-tag
/// properties, path builds — by descending through
/// [`Expression::children_mut`]. Subquery bodies are NOT descended into:
/// nested subqueries inside a body's WHERE / RETURN are compiled recursively
/// by the subquery planner rather than by this walker.
pub fn collect_expression_subqueries(
    expr: &mut Expression,
    id_alloc: &mut SubqueryIdAllocator,
) -> Vec<crate::core::types::expr::SubqueryBody> {
    let mut bodies = Vec::new();
    collect_expression_subqueries_inner(expr, id_alloc, &mut bodies);
    bodies
}

fn collect_expression_subqueries_inner(
    expr: &mut Expression,
    id_alloc: &mut SubqueryIdAllocator,
    out: &mut Vec<crate::core::types::expr::SubqueryBody>,
) {
    match expr {
        Expression::Exists { body } => {
            body.id = id_alloc.allocate();
            out.push(body.as_ref().clone());
        }
        Expression::In {
            expr: left,
            subquery,
            ..
        } => {
            subquery.id = id_alloc.allocate();
            out.push(subquery.as_ref().clone());
            // The IN left operand is an expression-level position itself.
            collect_expression_subqueries_inner(left, id_alloc, out);
        }
        // `Expression::children_mut` does not expose the aggregate filter.
        Expression::Aggregate { args, filter, .. } => {
            for arg in args.iter_mut() {
                collect_expression_subqueries_inner(arg, id_alloc, out);
            }
            if let Some(filter_expr) = filter {
                collect_expression_subqueries_inner(filter_expr, id_alloc, out);
            }
        }
        _ => {
            for child in expr.children_mut() {
                collect_expression_subqueries_inner(child, id_alloc, out);
            }
        }
    }
}

/// Unified planning entry for EXISTS / IN at any expression position
/// (WHERE residual, HAVING, RETURN, WITH assignments, ...).
///
/// Collects every EXISTS / IN in `expr`, assigns stable ids, compiles each
/// into a standalone sub-plan, and returns the rewritten expression (bodies
/// now carry their ids) plus the planned subqueries for the hosting
/// Filter/Project/Assign node. Compilation failure returns a precise
/// [`PlannerError`] naming the position and the supported alternative —
/// there is no "compiled but missing at runtime" path.
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
/// not yet support expression-level subqueries (ORDER BY / UNWIND / DML
/// value positions): any EXISTS / IN is refused at planning time with the
/// precise error instead of leaking to the runtime "not supported" path.
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
/// into a standalone sub-plan, re-registering the rewritten expression (bodies now carry
/// their stable ids) into its context so the plan node evaluates the
/// id-carrying form. Returns the compiled subqueries for the hosting node.
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
    let new_id = ctx.register_expression(ExpressionMeta::new(planned_expr));
    *ctx_expr = ContextualExpression::new(new_id, ctx.clone());
    Ok(subqueries)
}

/// The precise planning-time error for expression-level EXISTS / IN on
/// hosts that are not yet wired: the original Stage B error semantics.
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
///
/// The result differs from `plan_subquery` (conjunctive WHERE) in three
/// ways:
/// - no hash/probe key extraction: the subquery never participates in an
///   apply filter, the runtime tests result containment directly;
/// - IN needs no synthesized equality — the sub-plan only outputs the
///   RETURN expression column and the runtime probes it per row;
/// - a correlated subquery is re-rooted as
///   `Filter(correlated) -> CrossJoin(Argument(outer), plan)` so the runtime
///   can bind the hosting row as the correlation frame, and the optional
///   RETURN expression becomes a top `Project` node.
fn plan_scalar_subquery(
    body: &crate::core::types::expr::SubqueryBody,
    qctx: &Arc<QueryContext>,
    space_id: u64,
    space_name: &str,
    outer_col_names: &[String],
    id_alloc: &mut SubqueryIdAllocator,
) -> Result<PlannedSubquery, PlannerError> {
    // Parse the subquery patterns (stored as re-parseable strings).
    let mut patterns = Vec::with_capacity(body.patterns.len());
    for pattern_str in &body.patterns {
        let pattern = crate::query::parser::parsing::TraversalParser::new()
            .parse_pattern(&mut crate::query::parser::ParseContext::new(pattern_str))
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

    // Subquery-local conditions. Unlike the conjunctive path there is no
    // synthesized IN equality: containment is decided at runtime,
    // the sub-plan only has to produce the RETURN column.
    let mut conditions: Vec<Expression> = Vec::new();
    if let Some(where_expr) = &body.where_clause {
        collect_and_conjuncts(where_expr, &mut conditions);
    }

    // Nested EXISTS/IN inside the subquery's own WHERE are planned
    // recursively as further applies over the subquery base plan; the
    // residual (expression-level positions) is compiled recursively via
    // `plan_expression_subqueries` and attached to the sub-plan's own
    // Filter/Project nodes.
    let mut nested_specs = Vec::new();
    let mut flat_conditions = Vec::new();
    for condition in &conditions {
        flat_conditions.push(extract_conjunctive_exists(condition, &mut nested_specs));
    }

    // A RETURN expression referencing outer columns also makes the subquery
    // correlated: the projection must see the outer frame through the
    // CrossJoin below.
    let return_expr = body.return_expr.as_ref().map(|e| e.as_ref().clone());
    let return_correlated = return_expr
        .as_ref()
        .is_some_and(|e| e.get_variables().iter().any(|v| !inner_vars.contains(v)));

    // Scalar aggregate shape (`RETURN agg(arg)`): a Group-Join decorrelation
    // candidate. Only filter-free single-argument aggregates whose argument
    // references no outer variable and no nested subquery qualify.
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

    // When a scalar aggregate candidate is present, split the equi-key
    // conjuncts out of the conditions so the correlated part reduces to
    // pure hash/probe keys (the Group-Join precondition). Any other shape
    // keeps the conditions untouched below.
    let try_group_join =
        aggregate_return.is_some() && nested_specs.is_empty() && !return_correlated;
    let (hash_key_exprs, probe_key_exprs, residual_conditions) = if try_group_join {
        extract_keys(&flat_conditions, &inner_vars)?
    } else {
        (Vec::new(), Vec::new(), flat_conditions)
    };
    let (inner_residual, correlated_residual) = split_correlated(&residual_conditions, &inner_vars);
    let mut correlated_residual = correlated_residual;

    // Build the base subquery plan. Index selection is disabled for the
    // subquery: its tags are not part of the outer ValidationInfo, so scans
    // degrade to full scans (same convention as the conjunctive path).
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

    // Nested conjunctive EXISTS/IN wrap the subquery base plan.
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

    // Subquery-local residual filter; expression-level subqueries inside the
    // residual are compiled recursively and attached to this filter node.
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

    // Scalar aggregate Group-Join decorrelation: when the correlated part
    // reduced to pure equi keys and the right subtree is a simple
    // single-table shape, pre-aggregate per probe key instead of
    // re-executing the subquery per outer row. On any mismatch the
    // extracted equality conjuncts are restored so the CorrelatedApply /
    // plain-project fallback keeps every condition.
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

    // When the subquery references outer columns (non-equi correlation),
    // re-root it as Filter -> CrossJoin(Argument, plan) so the runtime can
    // bind the outer row as the correlation frame and re-execute per row.
    let correlated = !correlated_residual.is_empty() || return_correlated;
    if correlated {
        sub_plan = build_correlated_right_subtree(sub_plan, &correlated_residual, outer_col_names)?;
    }

    // The RETURN expression (if any) becomes a top projection; expression-level
    // subqueries inside it are compiled recursively and attached to the
    // project node. EXISTS without a RETURN expression needs no projection —
    // existence is decided by non-emptiness. The Group-Join form already
    // materialized the aggregate as the sub-plan root, so no projection is
    // wrapped on top.
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
        Expression::Exists { .. } | Expression::In { .. } => true,
        _ => expr
            .children()
            .iter()
            .any(|c| contains_expression_subquery(c)),
    }
}

/// Re-root the scalar aggregate right subtree as a Group-Join build side:
/// `Aggregate(group_by = probe keys, agg) -> Project(probe keys) -> plan`.
///
/// The probe key expressions are projected into dedicated `__gj_key_N`
/// columns first so the aggregate can group by name regardless of the
/// pattern plan's output layout. The aggregate output rows are
/// `[group_key..., agg_value]` positionally, which is what the runtime
/// Group-Join table build consumes.
fn build_group_join_right_subtree(
    sub_plan: SubPlan,
    probe_keys: &[Expression],
    agg_func: AggregateFunction,
    agg_arg: &Expression,
    distinct: bool,
    context: &Arc<ExpressionAnalysisContext>,
) -> Result<SubPlan, PlannerError> {
    let input_node = sub_plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The subquery plan has no root node".to_string())
    })?;

    let key_names: Vec<String> = (0..probe_keys.len())
        .map(|i| format!("__gj_key_{i}"))
        .collect();
    let columns: Vec<crate::core::YieldColumn> = probe_keys
        .iter()
        .zip(&key_names)
        .map(|(expr, name)| {
            crate::core::YieldColumn::new(to_contextual(expr.clone(), context), name.clone())
        })
        .collect();
    let project = ProjectNode::new(input_node, columns)?;

    let mut aggregate = AggregateNode::new(project.into_enum(), key_names.clone(), vec![agg_func])?;
    aggregate.set_aggregation_args(vec![vec![agg_arg.clone()]]);
    aggregate.set_aggregation_distinct(vec![distinct]);

    Ok(SubPlan {
        root: Some(aggregate.into_enum()),
        tail: sub_plan.tail,
        logical_root: None,
    })
}

/// Wrap `plan` with a filter node carrying expression-level subqueries
/// (physical node + logical mirror).
fn wrap_filter_with_subqueries(
    plan: SubPlan,
    condition: ContextualExpression,
    subqueries: Vec<PlannedSubquery>,
) -> Result<SubPlan, PlannerError> {
    let input_node = plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;
    let filter_node = FilterNode::new(input_node, condition.clone())?.with_subqueries(subqueries);

    let logical_root = plan.logical_root().cloned().map(|input| {
        LogicalNodeEnum::Filter(LogicalFilterNode {
            id: next_node_id(),
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            condition,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    });

    Ok(SubPlan {
        root: Some(filter_node.into_enum()),
        tail: plan.tail,
        logical_root,
    })
}

/// Wrap `plan` with a projection of a single expression column, carrying any
/// expression-level subqueries inside it (physical node + logical mirror).
fn wrap_project_with_subqueries(
    plan: SubPlan,
    expression: ContextualExpression,
    subqueries: Vec<PlannedSubquery>,
) -> Result<SubPlan, PlannerError> {
    let input_node = plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;
    let column =
        crate::core::YieldColumn::new(expression.clone(), expression.to_expression_string());
    let project_node =
        ProjectNode::new(input_node, vec![column.clone()])?.with_subqueries(subqueries);

    let logical_root = plan.logical_root().cloned().map(|input| {
        LogicalNodeEnum::Project(LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            columns: vec![column],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    });

    Ok(SubPlan {
        root: Some(project_node.into_enum()),
        tail: plan.tail,
        logical_root,
    })
}

/// Plan a single EXISTS / IN spec against the outer plan.
///
/// Returns the subquery plan (the right input of the apply) together with the
/// correlated keys. Callers wrap the outer plan with a `PatternApply` for
/// key-based correlation, or a `CorrelatedApply` when a residual condition
/// references outer variables and no equi keys exist.
pub fn plan_subquery(
    spec: &ExistsSpec,
    qctx: &Arc<QueryContext>,
    space_id: u64,
    space_name: &str,
    outer_col_names: &[String],
) -> Result<PlannedSubquery, PlannerError> {
    // Parse the subquery patterns (stored as re-parseable strings).
    let mut patterns = Vec::with_capacity(spec.body.patterns.len());
    for pattern_str in &spec.body.patterns {
        let pattern = crate::query::parser::parsing::TraversalParser::new()
            .parse_pattern(&mut crate::query::parser::ParseContext::new(pattern_str))
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

    // Subquery-local conjunctive conditions. For IN, synthesize the equality
    // `left_expr = return_expr` which participates in the key extraction.
    let mut conditions: Vec<Expression> = Vec::new();
    if let Some(where_expr) = &spec.body.where_clause {
        collect_and_conjuncts(where_expr, &mut conditions);
    }
    // The synthesized IN equality `left_expr = return_expr` participates in
    // key extraction; when the subquery ends up correlated it must be re-added
    // as a correlated filter condition so existence equals the IN test.
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

    // Nested EXISTS/IN inside the subquery's own WHERE are planned
    // recursively as further PatternApplies over the subquery base plan.
    let mut nested_specs = Vec::new();
    let mut flat_conditions = Vec::new();
    for condition in &conditions {
        flat_conditions.push(extract_conjunctive_exists(condition, &mut nested_specs));
    }

    let (hash_key_exprs, probe_key_exprs, residual_conditions) =
        extract_keys(&flat_conditions, &inner_vars)?;
    // Split residual conditions: subquery-local ones stay as a filter inside
    // the pattern plan; outer-correlated ones (no equi key) route to the
    // `CorrelatedApply` path.
    let (inner_residual, correlated_residual) = split_correlated(&residual_conditions, &inner_vars);

    // Build the base subquery plan. Index selection is disabled for the
    // subquery: its tags are not part of the outer ValidationInfo, so scans
    // degrade to full scans (see the design doc risk table).
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

    // Nested EXISTS/IN wrap the subquery base plan. Nested subqueries are
    // correlated against the current subquery's columns, so those become the
    // Argument layout of a nested CorrelatedApply when needed.
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

    // Subquery-local residual filter (references only subquery variables;
    // outer references are routed to the correlated path below). Skip the
    // trivially-true conjuncts left by nested EXISTS extraction.
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

    // When a residual condition references outer variables (non-equi
    // correlation), prefer the Mark-Join form: decorrelate into a SemiJoin
    // carrying the correlated residual as its join condition when the
    // subquery has a simple single-table shape. Only shapes that cannot be
    // decorrelated this way fall back to the per-row CorrelatedApply
    // (Filter -> CrossJoin(Argument, plan) re-executed per outer row).
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
        // The Mark-Join form keeps the extracted equi keys (mixed shapes);
        // the CorrelatedApply fallback clears them.
        hash_keys: if correlated { Vec::new() } else { hash_keys },
        probe_keys: if correlated { Vec::new() } else { probe_keys },
        // `correlated` marks the per-row CorrelatedApply routing; the
        // Mark-Join form is not a CorrelatedApply.
        correlated: correlated && mark_join_condition.is_none(),
        mark_join_condition,
        group_join: None,
    })
}

/// Wrap `left` with a `PatternApply` over the planned subquery.
///
/// Both the physical node and its logical mirror are built so `SubPlan`
/// stays consistent for the plan exit.
pub fn wrap_pattern_apply(
    left: SubPlan,
    planned: &PlannedSubquery,
    anti: bool,
) -> Result<SubPlan, PlannerError> {
    let left_root = left.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;
    let right_root = planned.plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The subquery plan has no root node".to_string())
    })?;

    let apply = PatternApplyNode::new(
        left_root,
        right_root,
        planned.hash_keys.clone(),
        planned.probe_keys.clone(),
        anti,
    )?;

    let logical_root = match (left.logical_root(), planned.plan.logical_root()) {
        (Some(left_logical), Some(right_logical)) => {
            Some(LogicalNodeEnum::PatternApply(LogicalPatternApplyNode {
                id: next_node_id(),
                left: Box::new(left_logical.clone()),
                right: Box::new(right_logical.clone()),
                hash_keys: planned.hash_keys.clone(),
                probe_keys: planned.probe_keys.clone(),
                deps: vec![left_logical.clone(), right_logical.clone()],
                is_anti_predicate: anti,
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            }))
        }
        _ => None,
    };

    Ok(SubPlan {
        root: Some(apply.into_enum()),
        tail: left.tail,
        logical_root,
    })
}

/// Wrap `left` with a `CorrelatedApply` over the planned correlated subquery.
///
/// Both the physical node and its logical mirror are built so `SubPlan`
/// stays consistent for the plan exit. The right subtree is self-contained
/// (rooted at an `Argument` source) and re-executed per outer row at runtime.
pub fn wrap_correlated_apply(
    left: SubPlan,
    planned: &PlannedSubquery,
    anti: bool,
) -> Result<SubPlan, PlannerError> {
    let left_root = left.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;
    let right_root = planned.plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The subquery plan has no root node".to_string())
    })?;

    let apply = CorrelatedApplyNode::new(left_root, right_root, anti)?;

    let logical_root = match (left.logical_root(), planned.plan.logical_root()) {
        (Some(left_logical), Some(right_logical)) => Some(LogicalNodeEnum::CorrelatedApply(
            LogicalCorrelatedApplyNode {
                id: next_node_id(),
                left: Box::new(left_logical.clone()),
                right: Box::new(right_logical.clone()),
                hash_keys: vec![],
                probe_keys: vec![],
                deps: vec![left_logical.clone(), right_logical.clone()],
                is_anti_predicate: anti,
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        )),
        _ => None,
    };

    Ok(SubPlan {
        root: Some(apply.into_enum()),
        tail: left.tail,
        logical_root,
    })
}

/// Wrap `left` with a Mark-Join (`SemiJoin` carrying the correlated residual
/// as its join condition) over the planned subquery.
///
/// The Mark-Join decorrelates a non-equi correlated EXISTS/IN: the right
/// side is the plain subquery plan (executed once, not per outer row), the
/// residual condition is evaluated over the combined left+right row, and
/// anti (NOT EXISTS) keeps left rows with no matching right row.
pub fn wrap_mark_join(
    left: SubPlan,
    planned: &PlannedSubquery,
    condition: &ContextualExpression,
    anti: bool,
) -> Result<SubPlan, PlannerError> {
    let left_root = left.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;
    let right_root = planned.plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The subquery plan has no root node".to_string())
    })?;

    let join = SemiJoinNode::new_with_condition(
        left_root,
        right_root,
        planned.hash_keys.clone(),
        planned.probe_keys.clone(),
        condition.clone(),
        anti,
    )?;

    let logical_root = match (left.logical_root(), planned.plan.logical_root()) {
        (Some(left_logical), Some(right_logical)) => {
            Some(LogicalNodeEnum::SemiJoin(LogicalSemiJoinNode {
                id: next_node_id(),
                left: Box::new(left_logical.clone()),
                right: Box::new(right_logical.clone()),
                hash_keys: planned.hash_keys.clone(),
                probe_keys: planned.probe_keys.clone(),
                deps: vec![left_logical.clone(), right_logical.clone()],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            }))
        }
        _ => None,
    };

    Ok(SubPlan {
        root: Some(join.into_enum()),
        tail: left.tail,
        logical_root,
    })
}
/// Re-root the subquery plan as the correlated right subtree:
/// `Filter(correlated) -> CrossJoin(Argument(col_names = outer), plan)`.
///
/// The `Argument` source carries only the outer layout; the outer row values
/// are injected per run into the executor's private correlation slot via
/// `StreamingExecutor::inject_correlation_frame`, and all
/// outer-correlated conditions live in the top `Filter` above the join.
fn build_correlated_right_subtree(
    sub_plan: SubPlan,
    correlated_residual: &[Expression],
    outer_col_names: &[String],
) -> Result<SubPlan, PlannerError> {
    let sub_root = sub_plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The subquery plan has no root node".to_string())
    })?;

    let expr_context = Arc::new(ExpressionAnalysisContext::new());
    let correlated_condition = to_contextual(and_join(correlated_residual), &expr_context);

    // Physical: Filter -> CrossJoin(Argument(col_names = outer), plan).
    let sub_tail = sub_plan.tail.clone();
    let sub_logical = sub_plan.logical_root().cloned();
    let mut argument = ArgumentNode::new(next_node_id(), "_correlated_apply");
    argument.set_col_names(outer_col_names.to_vec());
    let cross = CrossJoinNode::new(argument.into_enum(), sub_root)?;
    let filter = FilterNode::new(cross.into_enum(), correlated_condition.clone())?;
    let mut plan = SubPlan {
        root: Some(filter.into_enum()),
        tail: sub_tail,
        logical_root: None,
    };

    // Logical mirror: Filter -> CrossJoin(Argument, plan logical root).
    if let Some(sub_logical) = sub_logical {
        let logical_argument = LogicalNodeEnum::Argument(LogicalArgumentNode {
            id: next_node_id(),
            var: "_correlated_apply".to_string(),
            output_var: None,
            col_names: outer_col_names.to_vec(),
            column_types: vec![],
        });
        let logical_cross = LogicalNodeEnum::CrossJoin(LogicalCrossJoinNode {
            id: next_node_id(),
            left: Box::new(logical_argument.clone()),
            right: Box::new(sub_logical.clone()),
            hash_keys: vec![],
            probe_keys: vec![],
            deps: vec![logical_argument.clone(), sub_logical.clone()],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        let logical_filter = LogicalNodeEnum::Filter(LogicalFilterNode {
            id: next_node_id(),
            input: Some(Box::new(logical_cross.clone())),
            deps: vec![logical_cross],
            condition: correlated_condition,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        plan.logical_root = Some(logical_filter);
    }

    Ok(plan)
}

/// Wrap `plan` with a filter node (physical + logical mirror).
fn wrap_filter(plan: SubPlan, condition: ContextualExpression) -> Result<SubPlan, PlannerError> {
    let input_node = plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;
    let filter_node = FilterNode::new(input_node, condition.clone())?;

    let logical_root = plan.logical_root().cloned().map(|input| {
        LogicalNodeEnum::Filter(LogicalFilterNode {
            id: next_node_id(),
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            condition,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    });

    Ok(SubPlan {
        root: Some(filter_node.into_enum()),
        tail: plan.tail,
        logical_root,
    })
}

/// Split an AND-tree into conjuncts.
fn collect_and_conjuncts(expr: &Expression, out: &mut Vec<Expression>) {
    match expr {
        Expression::Binary {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            collect_and_conjuncts(left, out);
            collect_and_conjuncts(right, out);
        }
        _ => out.push(expr.clone()),
    }
}

/// Combine conjuncts into an AND-chain; `Literal(true)` when empty.
fn and_join(exprs: &[Expression]) -> Expression {
    let mut iter = exprs.iter();
    let Some(first) = iter.next() else {
        return Expression::literal(true);
    };
    let mut acc = first.clone();
    for expr in iter {
        acc = Expression::binary(acc, BinaryOperator::And, expr.clone());
    }
    acc
}

/// Register an expression into a fresh `ExpressionAnalysisContext`.
pub(crate) fn to_contextual(
    expr: Expression,
    context: &Arc<ExpressionAnalysisContext>,
) -> ContextualExpression {
    let id = context.register_expression(ExpressionMeta::new(expr));
    ContextualExpression::new(id, context.clone())
}

/// Extract equi-join keys from the subquery conditions.
///
/// A conjunct `a = b` becomes a key when one side references exactly one
/// subquery variable and the other side references none. Everything else
/// (including outer-correlated residual conditions) stays in `residual`; the
/// caller splits the residual into subquery-local and outer-correlated parts
/// via [`split_correlated`].
fn extract_keys(
    conditions: &[Expression],
    inner_vars: &HashSet<String>,
) -> Result<ExtractedKeys, PlannerError> {
    let mut hash_keys = Vec::new();
    let mut probe_keys = Vec::new();
    let mut residual = Vec::new();

    for condition in conditions {
        if let Expression::Binary {
            left,
            op: BinaryOperator::Equal,
            right,
        } = condition
        {
            let left_vars: HashSet<String> = left.get_variables().into_iter().collect();
            let right_vars: HashSet<String> = right.get_variables().into_iter().collect();
            let left_inner = left_vars.iter().filter(|v| inner_vars.contains(*v)).count();
            let right_inner = right_vars
                .iter()
                .filter(|v| inner_vars.contains(*v))
                .count();

            // Probe side = exactly one variable, which is a subquery variable;
            // hash side = no subquery variables (may reference outer vars or
            // be a constant).
            if left_vars.len() == 1 && left_inner == 1 && right_inner == 0 {
                hash_keys.push(right.as_ref().clone());
                probe_keys.push(left.as_ref().clone());
                continue;
            }
            if right_vars.len() == 1 && right_inner == 1 && left_inner == 0 {
                hash_keys.push(left.as_ref().clone());
                probe_keys.push(right.as_ref().clone());
                continue;
            }
        }
        residual.push(condition.clone());
    }

    Ok((hash_keys, probe_keys, residual))
}

/// Split residual conditions into subquery-local and outer-correlated parts.
///
/// A condition is correlated when it references any variable not bound by the
/// subquery patterns (`inner_vars`); those route to the `CorrelatedApply`
/// path. The remaining conditions reference only subquery variables and stay
/// as a subquery-local filter inside the pattern plan.
fn split_correlated(
    residual: &[Expression],
    inner_vars: &HashSet<String>,
) -> (Vec<Expression>, Vec<Expression>) {
    let mut inner_residual = Vec::new();
    let mut correlated_residual = Vec::new();
    for condition in residual {
        let cond_vars: HashSet<String> = condition.get_variables().into_iter().collect();
        if cond_vars.iter().any(|v| !inner_vars.contains(v)) {
            correlated_residual.push(condition.clone());
        } else {
            inner_residual.push(condition.clone());
        }
    }
    (inner_residual, correlated_residual)
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::Value;

    fn prop(var: &str, name: &str) -> Expression {
        Expression::property(Expression::variable(var), name)
    }

    fn eq(left: Expression, right: Expression) -> Expression {
        Expression::binary(left, BinaryOperator::Equal, right)
    }

    #[test]
    fn extracts_conjunctive_exists() {
        let cond = Expression::binary(
            eq(prop("t", "age"), Expression::literal(30)),
            BinaryOperator::And,
            Expression::Exists {
                body: Box::new(crate::core::types::expr::SubqueryBody {
                    patterns: vec!["(p:person)".to_string()],
                    where_clause: None,
                    return_expr: None,
                    id: 0,
                }),
            },
        );
        let mut specs = Vec::new();
        let residual = extract_conjunctive_exists(&cond, &mut specs);
        assert_eq!(specs.len(), 1);
        assert!(!specs[0].negated);
        assert_eq!(
            residual,
            Expression::binary(
                eq(prop("t", "age"), Expression::literal(30)),
                BinaryOperator::And,
                Expression::literal(true),
            )
        );
        assert!(!is_trivially_true(&residual));
    }

    #[test]
    fn extracts_negated_exists() {
        let cond = Expression::unary(
            UnaryOperator::Not,
            Expression::Exists {
                body: Box::new(crate::core::types::expr::SubqueryBody {
                    patterns: vec!["(p:person)".to_string()],
                    where_clause: None,
                    return_expr: None,
                    id: 0,
                }),
            },
        );
        let mut specs = Vec::new();
        let residual = extract_conjunctive_exists(&cond, &mut specs);
        assert_eq!(specs.len(), 1);
        assert!(specs[0].negated);
        assert!(is_trivially_true(&residual));
    }

    #[test]
    fn extracts_not_in_as_negated_spec() {
        // `x NOT IN { … }` parses as `NOT (x IN { … })`.
        let cond = Expression::unary(
            UnaryOperator::Not,
            Expression::in_subquery(
                Expression::variable("t"),
                crate::core::types::expr::SubqueryBody {
                    patterns: vec!["(p:person)".to_string()],
                    where_clause: None,
                    return_expr: Some(Box::new(prop("p", "name"))),
                    id: 0,
                },
                false,
            ),
        );
        let mut specs = Vec::new();
        let residual = extract_conjunctive_exists(&cond, &mut specs);
        assert_eq!(specs.len(), 1);
        assert!(specs[0].negated);
        assert_eq!(specs[0].left_expr, Some(Expression::variable("t")));
        assert!(is_trivially_true(&residual));
    }

    #[test]
    fn leaves_non_conjunctive_exists_untouched() {
        // EXISTS under OR must not be extracted.
        let inner = Expression::Exists {
            body: Box::new(crate::core::types::expr::SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: None,
                return_expr: None,
                id: 0,
            }),
        };
        let cond = Expression::binary(
            eq(prop("t", "age"), Expression::literal(30)),
            BinaryOperator::Or,
            inner.clone(),
        );
        let mut specs = Vec::new();
        let residual = extract_conjunctive_exists(&cond, &mut specs);
        assert!(specs.is_empty());
        assert_eq!(residual, cond);
    }

    #[test]
    fn extracts_keys_from_equality() {
        let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
        let conditions = vec![eq(prop("p", "name"), prop("t", "name"))];
        let (hash, probe, residual) = extract_keys(&conditions, &inner).expect("keys");
        assert_eq!(hash, vec![prop("t", "name")]);
        assert_eq!(probe, vec![prop("p", "name")]);
        assert!(residual.is_empty());
    }

    #[test]
    fn keeps_inner_only_condition_as_residual() {
        let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
        let conditions = vec![Expression::binary(
            prop("p", "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(30),
        )];
        let (hash, probe, residual) = extract_keys(&conditions, &inner).expect("keys");
        assert!(hash.is_empty());
        assert!(probe.is_empty());
        assert_eq!(residual.len(), 1);
    }

    #[test]
    fn keeps_outer_reference_in_correlated_residual() {
        let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
        let condition = Expression::binary(
            prop("p", "age"),
            BinaryOperator::GreaterThan,
            prop("t", "age"),
        );
        let conditions = vec![condition.clone()];
        let (hash, probe, residual) = extract_keys(&conditions, &inner).expect("keys");
        assert!(hash.is_empty());
        assert!(probe.is_empty());
        assert_eq!(residual, vec![condition]);
    }

    #[test]
    fn split_correlated_separates_inner_and_outer_residuals() {
        let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
        let inner_only = Expression::binary(
            prop("p", "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(30),
        );
        let correlated = Expression::binary(
            prop("p", "age"),
            BinaryOperator::GreaterThan,
            prop("t", "age"),
        );
        let (inner_residual, correlated_residual) =
            split_correlated(&[inner_only.clone(), correlated.clone()], &inner);
        assert_eq!(inner_residual, vec![inner_only]);
        assert_eq!(correlated_residual, vec![correlated]);
    }

    #[test]
    fn in_synthesizes_equality_key() {
        let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
        let conditions = vec![eq(
            Expression::variable("t"),
            Expression::property(Expression::variable("p"), "name"),
        )];
        let (hash, probe, _) = extract_keys(&conditions, &inner).expect("keys");
        assert_eq!(hash, vec![Expression::variable("t")]);
        assert_eq!(probe, vec![prop("p", "name")]);
    }

    #[test]
    fn plans_correlated_exists_as_mark_join() {
        use crate::core::types::expr::SubqueryBody;

        let spec = ExistsSpec {
            body: SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: Some(Box::new(Expression::binary(
                    prop("p", "age"),
                    BinaryOperator::GreaterThan,
                    prop("t", "age"),
                ))),
                return_expr: None,
                id: 0,
            },
            negated: false,
            left_expr: None,
        };

        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let outer_col_names = vec!["t".to_string(), "t.name".to_string()];
        let planned = plan_subquery(&spec, &qctx, 1, "default", &outer_col_names)
            .expect("correlated subquery should plan");

        // A simple single-table subquery with a non-equi correlated residual
        // routes to the Mark-Join form (SemiJoin with a residual condition),
        // not the per-row CorrelatedApply.
        assert!(!planned.correlated, "Mark-Join is not a CorrelatedApply");
        assert!(planned.hash_keys.is_empty());
        assert!(planned.probe_keys.is_empty());
        let condition = planned
            .mark_join_condition
            .as_ref()
            .expect("non-equi residual becomes the Mark-Join condition")
            .get_expression()
            .expect("condition registered");
        // The condition references the outer variable `t` (left side) and the
        // inner variable `p` (right side).
        let cond_vars: HashSet<String> = condition.get_variables().into_iter().collect();
        assert!(
            cond_vars.contains("t"),
            "Mark-Join condition must reference the outer variable, got vars {:?}",
            cond_vars
        );
        assert!(
            cond_vars.contains("p"),
            "Mark-Join condition must reference the inner variable, got vars {:?}",
            cond_vars
        );
    }

    // ── scalar aggregate Group-Join planning ────────────────────

    fn test_qctx() -> Arc<QueryContext> {
        Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )))
    }

    #[test]
    fn plans_correlated_scalar_aggregate_as_group_join() {
        use crate::core::types::expr::SubqueryBody;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        // `t.city IN { (p:person) WHERE p.city = t.city RETURN max(p.age) }`
        // at an expression position: the correlated part reduces to an equi
        // key and the right subtree is a simple single-table shape, so the
        // subquery decorrelates into a Group-Join build side (Aggregate over
        // the probe key).
        let body = SubqueryBody {
            patterns: vec!["(p:person)".to_string()],
            where_clause: Some(Box::new(eq(prop("p", "city"), prop("t", "city")))),
            return_expr: Some(Box::new(Expression::Aggregate {
                func: crate::core::types::operators::AggregateFunction::Max,
                args: vec![prop("p", "age")],
                distinct: false,
                filter: None,
            })),
            id: 0,
        };
        let expr = Expression::in_subquery(prop("t", "city"), body, false);

        let mut id_alloc = SubqueryIdAllocator::new();
        let (_, planned) = plan_expression_subqueries(
            expr,
            &test_qctx(),
            1,
            "default",
            &["t".to_string()],
            &mut id_alloc,
        )
        .expect("scalar aggregate subquery should plan");
        assert_eq!(planned.len(), 1);
        let planned = &planned[0];

        let gj = planned.group_join.as_ref().expect("Group-Join planned");
        assert!(!planned.correlated, "Group-Join is not a CorrelatedApply");
        assert!(planned.mark_join_condition.is_none());
        assert_eq!(gj.key_columns, 1);
        assert_eq!(
            gj.function,
            crate::core::types::operators::AggregateFunction::Max
        );
        assert!(!gj.distinct);
        // The outer-side hash key references the outer variable `t`.
        assert_eq!(gj.hash_keys.len(), 1);
        assert_eq!(gj.hash_keys[0].get_variables(), vec!["t".to_string()]);

        // Right subtree root = Aggregate -> Project -> pattern plan.
        let root = planned.plan.root().as_ref().expect("right subtree root");
        let PlanNodeEnum::Aggregate(aggregate) = root else {
            panic!("expected Aggregate root, got {}", root.type_name());
        };
        assert_eq!(aggregate.group_keys(), &["__gj_key_0".to_string()]);
        assert_eq!(aggregate.aggregation_functions().len(), 1);
        assert_eq!(
            aggregate.aggregation_args()[0],
            vec![prop("p", "age")],
            "aggregate argument preserved"
        );
    }

    #[test]
    fn non_equi_scalar_aggregate_keeps_correlated_apply_fallback() {
        use crate::core::types::expr::SubqueryBody;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        // Non-equi correlation cannot reduce to equi keys: the extracted-key
        // attempt is restored and the per-row CorrelatedApply fallback kept.
        let body = SubqueryBody {
            patterns: vec!["(p:person)".to_string()],
            where_clause: Some(Box::new(Expression::binary(
                prop("p", "age"),
                BinaryOperator::GreaterThan,
                prop("t", "age"),
            ))),
            return_expr: Some(Box::new(Expression::Aggregate {
                func: crate::core::types::operators::AggregateFunction::Count,
                args: vec![Expression::Literal(Value::string("*"))],
                distinct: false,
                filter: None,
            })),
            id: 0,
        };
        let expr = Expression::exists(body);

        let mut id_alloc = SubqueryIdAllocator::new();
        let (_, planned) = plan_expression_subqueries(
            expr,
            &test_qctx(),
            1,
            "default",
            &["t".to_string()],
            &mut id_alloc,
        )
        .expect("non-equi aggregate subquery should plan");
        assert_eq!(planned.len(), 1);
        let planned = &planned[0];
        assert!(planned.group_join.is_none(), "fallback to CorrelatedApply");
        assert!(planned.correlated, "CorrelatedApply routing flag set");
        // The correlated condition survived the restore: the right subtree is
        // Project(return) -> Filter -> CrossJoin(Argument, plan).
        let root = planned.plan.root().as_ref().expect("right subtree root");
        assert_eq!(root.type_name(), "Project", "RETURN projection on top");
        let PlanNodeEnum::Project(project) = root else {
            panic!("expected Project root");
        };
        let input = project.dependencies().first().expect("project input");
        assert_eq!(input.type_name(), "Filter", "correlated filter below");
    }

    #[test]
    fn filtered_aggregate_return_skips_group_join() {
        use crate::core::types::expr::SubqueryBody;

        // An aggregate with a FILTER clause is outside the Group-Join shape:
        // the RETURN expression keeps its plain projection.
        let body = SubqueryBody {
            patterns: vec!["(p:person)".to_string()],
            where_clause: None,
            return_expr: Some(Box::new(Expression::Aggregate {
                func: crate::core::types::operators::AggregateFunction::Count,
                args: vec![prop("p", "age")],
                distinct: false,
                filter: Some(Box::new(Expression::binary(
                    prop("p", "age"),
                    BinaryOperator::GreaterThan,
                    Expression::literal(30),
                ))),
            })),
            id: 0,
        };
        let expr = Expression::exists(body);

        let mut id_alloc = SubqueryIdAllocator::new();
        let (_, planned) = plan_expression_subqueries(
            expr,
            &test_qctx(),
            1,
            "default",
            &["t".to_string()],
            &mut id_alloc,
        )
        .expect("filtered aggregate subquery should plan");
        assert_eq!(planned.len(), 1);
        assert!(planned[0].group_join.is_none());
        let root = planned[0].plan.root().as_ref().expect("right subtree root");
        assert_eq!(root.type_name(), "Project", "plain RETURN projection");
    }

    #[test]
    fn plans_complex_correlated_exists_as_correlated_apply() {
        use crate::core::types::expr::SubqueryBody;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        // A multi-pattern (cross-joined) correlated subquery is not a simple
        // single-table shape, so it keeps the per-row CorrelatedApply
        // fallback instead of the Mark-Join form.
        let spec = ExistsSpec {
            body: SubqueryBody {
                patterns: vec!["(p:person)".to_string(), "(q:person)".to_string()],
                where_clause: Some(Box::new(Expression::binary(
                    prop("p", "age"),
                    BinaryOperator::GreaterThan,
                    prop("t", "age"),
                ))),
                return_expr: None,
                id: 0,
            },
            negated: false,
            left_expr: None,
        };

        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let outer_col_names = vec!["t".to_string(), "t.name".to_string()];
        let planned = plan_subquery(&spec, &qctx, 1, "default", &outer_col_names)
            .expect("correlated subquery should plan");

        assert!(
            planned.correlated,
            "cross-joined subquery routes to CorrelatedApply"
        );
        assert!(planned.mark_join_condition.is_none());

        // Right subtree root = Filter over CrossJoin(Argument, pattern plan).
        let root = planned
            .plan
            .root()
            .as_ref()
            .expect("right subtree has a root");
        let PlanNodeEnum::Filter(filter_node) = root else {
            panic!("expected Filter root, got {}", root.type_name());
        };
        let Some(PlanNodeEnum::CrossJoin(cross_node)) = filter_node.dependencies().first() else {
            panic!("expected CrossJoin below the correlated Filter");
        };
        let PlanNodeEnum::Argument(argument) = cross_node.left_input() else {
            panic!("expected Argument as the cross join left input");
        };
        assert_eq!(
            argument.col_names(),
            outer_col_names.as_slice(),
            "Argument col_names mirror the outer layout"
        );
        assert!(
            !matches!(cross_node.right_input(), PlanNodeEnum::Argument(_)),
            "right input of the cross join is the subquery pattern plan"
        );
    }

    #[test]
    fn nested_correlated_exists_wraps_inner_mark_join() {
        use crate::core::types::expr::SubqueryBody;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        // Outer EXISTS over `(p:person)` whose WHERE contains a nested EXISTS
        // correlated against `p` (`q.age > p.age`). The nested subquery is a
        // simple single-table shape, so it becomes a Mark-Join (SemiJoin).
        let inner_body = SubqueryBody {
            patterns: vec!["(q:person)".to_string()],
            where_clause: Some(Box::new(Expression::binary(
                prop("q", "age"),
                BinaryOperator::GreaterThan,
                prop("p", "age"),
            ))),
            return_expr: None,
            id: 0,
        };
        let outer_spec = ExistsSpec {
            body: SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: Some(Box::new(Expression::Exists {
                    body: Box::new(inner_body),
                })),
                return_expr: None,
                id: 0,
            },
            negated: false,
            left_expr: None,
        };

        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let planned = plan_subquery(&outer_spec, &qctx, 1, "default", &["t".to_string()])
            .expect("outer EXISTS should plan");
        // The outer subquery itself references no outer variables.
        assert!(!planned.correlated);
        // The nested correlated EXISTS wraps the subquery base plan as a
        // Mark-Join (SemiJoin with a residual condition).
        assert!(
            matches!(
                planned.plan.root().as_ref(),
                Some(PlanNodeEnum::SemiJoin(_))
            ),
            "nested correlated EXISTS must be planned as a Mark-Join SemiJoin"
        );
    }

    #[test]
    fn in_with_correlated_where_keeps_synthesized_equality_in_mark_join_condition() {
        use crate::core::types::expr::SubqueryBody;

        let spec = ExistsSpec {
            body: SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: Some(Box::new(Expression::binary(
                    prop("p", "age"),
                    BinaryOperator::GreaterThan,
                    prop("t", "age"),
                ))),
                return_expr: Some(Box::new(prop("p", "age"))),
                id: 0,
            },
            negated: false,
            left_expr: Some(prop("t", "age")),
        };

        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let planned = plan_subquery(&spec, &qctx, 1, "default", &["t".to_string()])
            .expect("correlated IN should plan");
        // Simple single-table shape: the correlated IN routes to Mark-Join.
        assert!(!planned.correlated);
        // The Mark-Join condition must include the synthesized
        // `t.age = p.age` equality (IN semantics) alongside `p.age > t.age`.
        let condition = planned
            .mark_join_condition
            .as_ref()
            .expect("Mark-Join condition registered")
            .get_expression()
            .expect("condition resolved");
        let conjuncts = {
            let mut out = Vec::new();
            collect_and_conjuncts(&condition, &mut out);
            out
        };
        assert_eq!(conjuncts.len(), 2);
        assert!(
            conjuncts.contains(&eq(prop("t", "age"), prop("p", "age"))),
            "IN synthesized equality joins the Mark-Join condition"
        );
    }

    #[test]
    fn literal_true_is_trivially_true() {
        assert!(is_trivially_true(&Expression::literal(true)));
        assert!(is_trivially_true(&Expression::binary(
            Expression::literal(true),
            BinaryOperator::And,
            Expression::literal(true),
        )));
        assert!(!is_trivially_true(&Expression::literal(Value::Int(1))));
    }

    // ── expression-level subquery collection ───────────────────

    fn body() -> crate::core::types::expr::SubqueryBody {
        crate::core::types::expr::SubqueryBody {
            id: 0,
            patterns: vec!["(p:person)".to_string()],
            where_clause: None,
            return_expr: None,
        }
    }

    fn exists() -> Expression {
        Expression::exists(body())
    }

    fn in_subq(left: Expression) -> Expression {
        Expression::in_subquery(left, body(), false)
    }

    /// Assert that `expr` contains exactly `expected` expression-level
    /// subqueries and that every body received a unique, monotonically
    /// allocated id.
    fn assert_collected(mut expr: Expression, expected: usize) {
        let mut alloc = SubqueryIdAllocator::new();
        let bodies = collect_expression_subqueries(&mut expr, &mut alloc);
        assert_eq!(bodies.len(), expected, "collected subqueries");
        let mut ids: Vec<u64> = bodies.iter().map(|b| b.id).collect();
        ids.sort_unstable();
        let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "ids must be unique");
        if !ids.is_empty() {
            assert_eq!(ids[0], 0, "ids start at 0");
            for w in ids.windows(2) {
                assert_eq!(w[1], w[0] + 1, "ids are contiguous");
            }
        }
        // Bodies must carry their assigned ids in the mutated expression.
        // Traverse manually: `Expression::find_all` goes through
        // `children()`, which hides the aggregate filter.
        fn count_subqueries(expr: &Expression) -> usize {
            let own = matches!(expr, Expression::Exists { .. } | Expression::In { .. }) as usize;
            let children = match expr {
                Expression::Aggregate { args, filter, .. } => {
                    let mut c: Vec<&Expression> = args.iter().collect();
                    if let Some(f) = filter {
                        c.push(f.as_ref());
                    }
                    c
                }
                Expression::Exists { body } => {
                    let mut c: Vec<&Expression> = Vec::new();
                    if let Some(w) = &body.where_clause {
                        c.push(w);
                    }
                    if let Some(r) = &body.return_expr {
                        c.push(r);
                    }
                    c
                }
                _ => expr.children(),
            };
            own + children.iter().map(|c| count_subqueries(c)).sum::<usize>()
        }
        assert_eq!(
            count_subqueries(&expr),
            expected,
            "expression tree retains subqueries"
        );
    }

    #[test]
    fn collects_subquery_at_top_level() {
        assert_collected(exists(), 1);
        assert_collected(in_subq(Expression::variable("t")), 1);
    }

    #[test]
    fn collects_binary_left_and_right() {
        let expr = Expression::binary(
            exists(),
            BinaryOperator::Or,
            in_subq(Expression::variable("t")),
        );
        assert_collected(expr, 2);
    }

    #[test]
    fn collects_unary_operand() {
        assert_collected(Expression::unary(UnaryOperator::Not, exists()), 1);
    }

    #[test]
    fn collects_inside_case() {
        let expr = Expression::Case {
            test_expr: Some(Box::new(exists())),
            conditions: vec![(exists(), exists())],
            default: Some(Box::new(in_subq(Expression::variable("t")))),
        };
        assert_collected(expr, 4);
    }

    #[test]
    fn collects_inside_list_and_map() {
        assert_collected(
            Expression::List(vec![exists(), in_subq(Expression::variable("t"))]),
            2,
        );
        assert_collected(
            Expression::Map(vec![
                ("a".to_string(), exists()),
                ("b".to_string(), in_subq(Expression::variable("t"))),
            ]),
            2,
        );
    }

    #[test]
    fn collects_inside_type_cast_subscript_range_path() {
        assert_collected(
            Expression::cast(exists(), crate::core::types::DataType::Bool),
            1,
        );
        assert_collected(Expression::subscript(exists(), Expression::literal(0)), 1);
        assert_collected(
            Expression::Range {
                collection: Box::new(exists()),
                start: Some(Box::new(in_subq(Expression::variable("t")))),
                end: None,
            },
            2,
        );
        assert_collected(Expression::Path(vec![exists()]), 1);
    }

    #[test]
    fn collects_inside_list_comprehension() {
        let expr = Expression::ListComprehension {
            variable: "x".to_string(),
            source: Box::new(exists()),
            filter: Some(Box::new(in_subq(Expression::variable("t")))),
            map: Some(Box::new(Expression::function(
                "upper".to_string(),
                vec![exists()],
            ))),
        };
        assert_collected(expr, 3);
    }

    #[test]
    fn collects_inside_function_and_aggregate_args() {
        assert_collected(
            Expression::function(
                "f".to_string(),
                vec![exists(), in_subq(Expression::variable("t"))],
            ),
            2,
        );
        let agg = Expression::Aggregate {
            func: crate::core::types::operators::AggregateFunction::Count,
            args: vec![exists()],
            distinct: false,
            filter: Some(Box::new(in_subq(Expression::variable("t")))),
        };
        assert_collected(agg, 2);
    }

    #[test]
    fn collects_inside_reduce_and_window_function() {
        assert_collected(
            Expression::reduce(
                "acc",
                exists(),
                "x",
                in_subq(Expression::variable("t")),
                Expression::variable("acc"),
            ),
            2,
        );
        let window = Expression::WindowFunction {
            name: "row_number".to_string(),
            args: vec![exists()],
            over_partition_by: vec![in_subq(Expression::variable("t"))],
            over_order_by: vec![],
            over_order_desc: vec![],
        };
        assert_collected(window, 2);
    }

    #[test]
    fn collects_inside_label_tag_property_and_path_build() {
        assert_collected(
            Expression::LabelTagProperty {
                tag: Box::new(exists()),
                property: "name".to_string(),
            },
            1,
        );
        assert_collected(
            Expression::PathBuild(vec![exists(), in_subq(Expression::variable("t"))]),
            2,
        );
    }

    #[test]
    fn collects_in_left_operand_of_in() {
        let expr = Expression::in_subquery(exists(), body(), false);
        assert_collected(expr, 2);
    }

    #[test]
    fn does_not_descend_into_subquery_bodies() {
        // A subquery whose WHERE / RETURN themselves contain EXISTS / IN must
        // not be collected at expression level: those are compiled
        // recursively by the subquery planner.
        let inner = crate::core::types::expr::SubqueryBody {
            id: 0,
            patterns: vec!["(p:person)".to_string()],
            where_clause: Some(Box::new(Expression::binary(
                Expression::variable("p"),
                BinaryOperator::Or,
                exists(),
            ))),
            return_expr: Some(Box::new(in_subq(Expression::variable("p")))),
        };
        let mut alloc = SubqueryIdAllocator::new();
        let mut expr = Expression::exists(inner);
        let bodies = collect_expression_subqueries(&mut expr, &mut alloc);
        assert_eq!(bodies.len(), 1, "only the outer subquery is collected");
    }

    #[test]
    fn ids_are_reassigned_on_replanning() {
        let mut first_alloc = SubqueryIdAllocator::new();
        let mut second_alloc = SubqueryIdAllocator::new();
        let mut expr_a = Expression::binary(exists(), BinaryOperator::Or, exists());
        let mut expr_b = Expression::binary(exists(), BinaryOperator::Or, exists());
        let ids_a: Vec<u64> = collect_expression_subqueries(&mut expr_a, &mut first_alloc)
            .iter()
            .map(|b| b.id)
            .collect();
        let ids_b: Vec<u64> = collect_expression_subqueries(&mut expr_b, &mut second_alloc)
            .iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(ids_a, vec![0, 1]);
        assert_eq!(ids_b, vec![0, 1], "fresh planning re-assigns ids from 0");
    }

    #[test]
    fn plan_expression_subqueries_rejects_and_accepts() {
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));
        let mut alloc = SubqueryIdAllocator::new();
        let outer = vec!["t".to_string()];

        // No expression-level subquery: unchanged expression, no runners.
        let expr = Expression::binary(
            prop("t", "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(30),
        );
        let (out, planned) =
            plan_expression_subqueries(expr.clone(), &qctx, 1, "default", &outer, &mut alloc)
                .expect("plain expression passes");
        assert_eq!(out, expr);
        assert!(planned.is_empty());

        // EXISTS / IN at expression level: compiled into
        // standalone sub-plans with stable ids. The rewritten expression
        // carries the ids; the runners match by id.
        for expr in [exists(), in_subq(Expression::variable("t"))] {
            let (rewritten, planned) =
                plan_expression_subqueries(expr, &qctx, 1, "default", &outer, &mut alloc)
                    .expect("expression-level subquery compiles");
            assert_eq!(planned.len(), 1, "one subquery compiled");
            let body_id = match &rewritten {
                Expression::Exists { body } | Expression::In { subquery: body, .. } => body.id,
                other => panic!("expected a top-level subquery expression, got {:?}", other),
            };
            assert_eq!(planned[0].id, body_id, "runner id matches the body id");
            assert!(!planned[0].correlated, "no outer references in test body");
            assert!(planned[0].plan.root().is_some(), "sub-plan has a root");
        }

        // Ids are re-allocated per planning pass and unique within a pass.
        let (_, planned) =
            plan_expression_subqueries(exists(), &qctx, 1, "default", &outer, &mut alloc)
                .expect("re-plan succeeds");
        assert_eq!(planned.len(), 1);
    }
}

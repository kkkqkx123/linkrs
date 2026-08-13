//! EXISTS / IN subquery planning for conjunctive WHERE positions (P1).
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
//! with the outer row bound as the correlation frame (P2).

use std::collections::HashSet;
use std::sync::Arc;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::core::types::expr::ExpressionMeta;
use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::types::{ContextualExpression, Expression};
use crate::query::binder::validation::ValidationInfo;
use crate::query::parser::ast::pattern::PatternUtils;
use crate::query::planning::plan::core::next_node_id;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::control_flow::ArgumentNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::CorrelatedApplyNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode;
use crate::query::planning::plan::core::nodes::join::CrossJoinNode;
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::query::planning::plan::logical::logical_nodes::graph_ops::LogicalCorrelatedApplyNode;
use crate::query::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode;
use crate::query::planning::plan::logical::logical_nodes::join::LogicalCrossJoinNode;
use crate::query::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
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
    /// The subquery plan (pattern scans + subquery-local filters, or the
    /// `Filter -> CrossJoin -> Argument` correlated right subtree).
    pub plan: SubPlan,
    /// Outer-side (left layout) key expressions.
    pub hash_keys: Vec<ContextualExpression>,
    /// Subquery-side (right layout) key expressions.
    pub probe_keys: Vec<ContextualExpression>,
    /// True when the subquery is planned as a `CorrelatedApply` (per-row
    /// re-execution over an `Argument` frame) instead of a key-based
    /// `PatternApply`. This is a planning-time routing flag only.
    pub correlated: bool,
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
    // P2 `CorrelatedApply` path.
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
        sub_plan = if planned.correlated {
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
    // correlation), the subquery cannot be decorrelated via hash/probe keys.
    // Re-root the right subtree as Filter -> CrossJoin(Argument, plan) so the
    // CorrelatedApply operator can bind the outer row as the correlation frame
    // and re-execute the subtree per row. For IN, the synthesized equality is
    // folded into the correlated filter so existence equals the IN test.
    let correlated = !correlated_residual.is_empty();
    if correlated {
        let mut correlated_conditions = correlated_residual;
        if let Some(equality) = in_equality {
            correlated_conditions.push(equality);
        }
        sub_plan =
            build_correlated_right_subtree(sub_plan, &correlated_conditions, outer_col_names)?;
    }

    Ok(PlannedSubquery {
        plan: sub_plan,
        hash_keys: if correlated { Vec::new() } else { hash_keys },
        probe_keys: if correlated { Vec::new() } else { probe_keys },
        correlated,
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

/// Re-root the subquery plan as the correlated right subtree:
/// `Filter(correlated) -> CrossJoin(Argument(col_names = outer), plan)`.
///
/// The `Argument` source carries only the outer layout; the outer row values
/// are injected at runtime via `ExecutionRuntime::set_correlation_frame`, and
/// all outer-correlated conditions live in the top `Filter` above the join.
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
/// subquery patterns (`inner_vars`); those route to the P2 `CorrelatedApply`
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
                    is_correlated: false,
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
                    is_correlated: false,
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
                    is_correlated: false,
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
                is_correlated: false,
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
    fn plans_correlated_exists_as_correlated_apply() {
        use crate::core::types::expr::SubqueryBody;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        let spec = ExistsSpec {
            body: SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: Some(Box::new(Expression::binary(
                    prop("p", "age"),
                    BinaryOperator::GreaterThan,
                    prop("t", "age"),
                ))),
                return_expr: None,
                is_correlated: true,
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
            "non-equi correlation routes to CorrelatedApply"
        );
        assert!(planned.hash_keys.is_empty());
        assert!(planned.probe_keys.is_empty());

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
        // The correlated Filter references the outer variable `t`.
        let correlated_cond = filter_node
            .condition()
            .get_expression()
            .expect("correlated condition registered");
        let cond_vars: HashSet<String> = correlated_cond.get_variables().into_iter().collect();
        assert!(
            cond_vars.contains("t"),
            "correlated filter must reference the outer variable, got vars {:?}",
            cond_vars
        );
    }

    #[test]
    fn nested_correlated_exists_wraps_inner_correlated_apply() {
        use crate::core::types::expr::SubqueryBody;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        // Outer EXISTS over `(p:person)` whose WHERE contains a nested EXISTS
        // correlated against `p` (`q.age > p.age`).
        let inner_body = SubqueryBody {
            patterns: vec!["(q:person)".to_string()],
            where_clause: Some(Box::new(Expression::binary(
                prop("q", "age"),
                BinaryOperator::GreaterThan,
                prop("p", "age"),
            ))),
            return_expr: None,
            is_correlated: true,
        };
        let outer_spec = ExistsSpec {
            body: SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: Some(Box::new(Expression::Exists {
                    body: Box::new(inner_body),
                })),
                return_expr: None,
                is_correlated: true,
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
        // The nested correlated EXISTS wraps the subquery base plan.
        assert!(
            matches!(
                planned.plan.root().as_ref(),
                Some(PlanNodeEnum::CorrelatedApply(_))
            ),
            "nested correlated EXISTS must be planned as a CorrelatedApply"
        );
    }

    #[test]
    fn in_with_correlated_where_keeps_synthesized_equality_in_filter() {
        use crate::core::types::expr::SubqueryBody;
        use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

        let spec = ExistsSpec {
            body: SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: Some(Box::new(Expression::binary(
                    prop("p", "age"),
                    BinaryOperator::GreaterThan,
                    prop("t", "age"),
                ))),
                return_expr: Some(Box::new(prop("p", "age"))),
                is_correlated: true,
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
        assert!(planned.correlated);
        // The correlated Filter condition must include the synthesized
        // `t.age = p.age` equality (IN semantics) alongside `p.age > t.age`.
        let root = planned.plan.root().as_ref().expect("right subtree root");
        let PlanNodeEnum::Filter(filter_node) = root else {
            panic!("expected Filter root, got {}", root.type_name());
        };
        let condition = filter_node
            .condition()
            .get_expression()
            .expect("correlated condition registered");
        let conjuncts = {
            let mut out = Vec::new();
            collect_and_conjuncts(&condition, &mut out);
            out
        };
        assert_eq!(conjuncts.len(), 2);
        assert!(
            conjuncts.contains(&eq(prop("t", "age"), prop("p", "age"))),
            "IN synthesized equality joins the correlated filter"
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
}

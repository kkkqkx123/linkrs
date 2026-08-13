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
//! Non-equi correlation (e.g. `p.age > t.age`) is rejected with a precise
//! error; the per-row re-execution fallback is a deferred P2 item.

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
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode;
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode;
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
    /// The subquery plan (pattern scans + subquery-local filters).
    pub plan: SubPlan,
    /// Outer-side (left layout) key expressions.
    pub hash_keys: Vec<ContextualExpression>,
    /// Subquery-side (right layout) key expressions.
    pub probe_keys: Vec<ContextualExpression>,
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
/// correlated keys. Callers wrap the outer plan with a `PatternApply`.
pub fn plan_subquery(
    spec: &ExistsSpec,
    qctx: &Arc<QueryContext>,
    space_id: u64,
    space_name: &str,
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
    if let Some(left_expr) = &spec.left_expr {
        let return_expr = spec.body.return_expr.as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "IN subquery requires a RETURN expression".to_string(),
            )
        })?;
        conditions.push(Expression::binary(
            left_expr.clone(),
            BinaryOperator::Equal,
            return_expr.as_ref().clone(),
        ));
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

    // Nested EXISTS/IN wrap the subquery base plan.
    for nested in &nested_specs {
        let planned = plan_subquery(nested, qctx, space_id, space_name)?;
        sub_plan = wrap_pattern_apply(sub_plan, &planned, nested.negated)?;
    }

    // Subquery-local residual filter (references only subquery variables;
    // outer references were rejected by key extraction).
    if !residual_conditions.is_empty() {
        let residual_expr = and_join(&residual_conditions);
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

    Ok(PlannedSubquery {
        plan: sub_plan,
        hash_keys,
        probe_keys,
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
/// subquery variable and the other side references none. Conditions that
/// still reference outer variables (non-equi correlation) are rejected for
/// now (P2). Everything else stays as a subquery-local residual filter.
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
            let right_inner = right_vars.iter().filter(|v| inner_vars.contains(*v)).count();

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

    // Any residual condition that references outer variables is a correlated
    // predicate that cannot be evaluated inside the subquery plan.
    for condition in &residual {
        let cond_vars: HashSet<String> = condition.get_variables().into_iter().collect();
        let outer: Vec<&String> = cond_vars.iter().filter(|v| !inner_vars.contains(*v)).collect();
        if !outer.is_empty() {
            return Err(PlannerError::UnsupportedOperation(format!(
                "Correlated subquery condition `{}` references outer variable(s) {:?}; \
                 only equality correlation is supported (P2)",
                condition.to_expression_string(),
                outer
            )));
        }
    }

    Ok((hash_keys, probe_keys, residual))
}

#[cfg(test)]
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
    fn rejects_outer_reference_in_residual() {
        let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
        let conditions = vec![Expression::binary(
            prop("p", "age"),
            BinaryOperator::GreaterThan,
            prop("t", "age"),
        )];
        let result = extract_keys(&conditions, &inner);
        assert!(matches!(result, Err(PlannerError::UnsupportedOperation(_))));
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

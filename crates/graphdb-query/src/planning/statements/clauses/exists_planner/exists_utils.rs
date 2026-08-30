use std::collections::HashSet;
use std::sync::Arc;

use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::types::{ContextualExpression, Expression};

use super::exists_types::ExtractedKeys;

/// Register an expression into a fresh `ExpressionAnalysisContext`.
pub(crate) fn to_contextual(
    expr: Expression,
    context: &Arc<ExpressionAnalysisContext>,
) -> ContextualExpression {
    let id = context.register_expression(ExpressionMeta::new(expr));
    ContextualExpression::new(id, context.clone())
}

/// Split an AND-tree into conjuncts.
pub(crate) fn collect_and_conjuncts(expr: &Expression, out: &mut Vec<Expression>) {
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
pub(crate) fn and_join(exprs: &[Expression]) -> Expression {
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

/// Extract equi-join keys from the subquery conditions.
pub(crate) fn extract_keys(
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
pub(crate) fn split_correlated(
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

/// Wrap `plan` with a filter node (physical + logical mirror).
pub(crate) fn wrap_filter(
    plan: SubPlan,
    condition: ContextualExpression,
) -> Result<SubPlan, PlannerError> {
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

/// Wrap `plan` with a filter node carrying expression-level subqueries
/// (physical node + logical mirror).
pub(crate) fn wrap_filter_with_subqueries(
    plan: SubPlan,
    condition: ContextualExpression,
    subqueries: Vec<super::exists_types::PlannedSubquery>,
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
pub(crate) fn wrap_project_with_subqueries(
    plan: SubPlan,
    expression: ContextualExpression,
    subqueries: Vec<super::exists_types::PlannedSubquery>,
) -> Result<SubPlan, PlannerError> {
    let input_node = plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
    })?;
    let column =
        graphdb_core::YieldColumn::new(expression.clone(), expression.to_expression_string());
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

use std::sync::Arc;

use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::control_flow::ArgumentNode;
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::CorrelatedApplyNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode;
use crate::planning::plan::core::nodes::join::join_node::SemiJoinNode;
use crate::planning::plan::core::nodes::join::CrossJoinNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalCorrelatedApplyNode;
use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode;
use crate::planning::plan::logical::logical_nodes::join::LogicalCrossJoinNode;
use crate::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalAggregateNode, LogicalProjectNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::operators::AggregateFunction;
use graphdb_core::types::{ContextualExpression, Expression};

use super::{and_join, to_contextual, PlannedSubquery};

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
                join_condition: Some(condition.clone()),
                anti,
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

pub fn build_group_join_right_subtree(
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
    let columns: Vec<graphdb_core::YieldColumn> = probe_keys
        .iter()
        .zip(&key_names)
        .map(|(expr, name)| {
            graphdb_core::YieldColumn::new(to_contextual(expr.clone(), context), name.clone())
        })
        .collect();
    let project = crate::planning::plan::core::nodes::operation::project_node::ProjectNode::new(
        input_node,
        columns.clone(),
    )?;
    let project_col_names = project.col_names().to_vec();

    let mut aggregate = AggregateNode::new(project.into_enum(), key_names.clone(), vec![agg_func])?;
    aggregate.set_aggregation_args(vec![vec![agg_arg.clone()]]);
    aggregate.set_aggregation_distinct(vec![distinct]);
    let group_key_exprs: Vec<ContextualExpression> = key_names
        .iter()
        .map(|name| to_contextual(Expression::Variable(name.clone()), context))
        .collect();
    aggregate.set_group_key_exprs(group_key_exprs.clone());

    let logical_root = sub_plan.logical_root().cloned().map(|input| {
        let logical_project = LogicalNodeEnum::Project(LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(input)),
            columns: columns.clone(),
            subqueries: Vec::new(),
            has_folded_expressions: false,
            output_var: None,
            col_names: project_col_names.clone(),
            column_types: vec![],
        });
        LogicalNodeEnum::Aggregate(LogicalAggregateNode {
            id: next_node_id(),
            input: Some(Box::new(logical_project)),
            group_key_exprs: group_key_exprs.clone(),
            aggregation_functions: vec![agg_func],
            aggregation_args: aggregate.aggregation_args().to_vec(),
            aggregation_distinct: vec![distinct],
            aggregation_filters: vec![None],
            grouping_sets: vec![],
            output_var: None,
            col_names: aggregate.col_names().to_vec(),
            column_types: vec![],
        })
    });

    Ok(SubPlan {
        root: Some(aggregate.into_enum()),
        tail: sub_plan.tail,
        logical_root,
    })
}

pub fn build_correlated_right_subtree(
    sub_plan: SubPlan,
    correlated_residual: &[Expression],
    outer_col_names: &[String],
) -> Result<SubPlan, PlannerError> {
    let sub_root = sub_plan.root().clone().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The subquery plan has no root node".to_string())
    })?;

    let expr_context = Arc::new(ExpressionAnalysisContext::new());
    let correlated_condition = to_contextual(and_join(correlated_residual), &expr_context);

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
            left: Box::new(logical_argument),
            right: Box::new(sub_logical),
            hash_keys: vec![],
            probe_keys: vec![],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        let logical_filter = LogicalNodeEnum::Filter(LogicalFilterNode {
            id: next_node_id(),
            input: Some(Box::new(logical_cross)),
            condition: correlated_condition,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        plan.logical_root = Some(logical_filter);
    }

    Ok(plan)
}

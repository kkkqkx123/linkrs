use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::operation::sort_node::SortItem;
use crate::planning::plan::core::nodes::{CrossJoinNode, LeftJoinNode, UnionNode};
use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnionNode;
use crate::planning::plan::logical::logical_nodes::join::{
    LogicalCrossJoinNode, LogicalLeftJoinNode,
};
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalDedupNode, LogicalFilterNode, LogicalLimitNode, LogicalProjectNode, LogicalSortNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::YieldColumn;

/// Wrap the attached logical mirror (if any) of `input` with `wrap`.
///
/// Clause planners stack a node on top of the physical root; the mirror is
/// wrapped in lock-step so the compiler keeps consuming a native logical
/// tree.
pub(crate) fn wrap_logical(
    input: &SubPlan,
    wrap: impl FnOnce(LogicalNodeEnum) -> LogicalNodeEnum,
) -> Option<LogicalNodeEnum> {
    input.logical_root().cloned().map(wrap)
}

/// Seed a native logical tree for a standalone plan over a single empty row.
pub(crate) fn logical_start_root() -> LogicalNodeEnum {
    LogicalNodeEnum::Start(LogicalStartNode::new())
}

/// Seed a native logical tree for a standalone plan fed by an argument.
pub(crate) fn logical_argument_root(
    var: &str,
    col_names: Vec<String>,
    output_var: Option<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Argument(LogicalArgumentNode {
        id: next_node_id(),
        var: var.to_string(),
        output_var,
        col_names,
        column_types: vec![],
    })
}

/// Stack a projection mirror on top of a standalone logical tree.
pub(crate) fn wrap_logical_project(
    input: LogicalNodeEnum,
    columns: Vec<YieldColumn>,
    col_names: Vec<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Project(LogicalProjectNode {
        id: next_node_id(),
        input: Some(Box::new(input.clone())),
        deps: vec![input],
        columns,
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

/// Stack a filter mirror on top of a standalone logical tree.
pub(crate) fn wrap_logical_filter(
    input: LogicalNodeEnum,
    condition: ContextualExpression,
    col_names: Vec<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Filter(LogicalFilterNode {
        id: next_node_id(),
        input: Some(Box::new(input.clone())),
        deps: vec![input],
        condition,
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

/// Stack a dedup mirror on top of a standalone logical tree.
pub(crate) fn wrap_logical_dedup(
    input: LogicalNodeEnum,
    col_names: Vec<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Dedup(LogicalDedupNode {
        id: next_node_id(),
        input: Some(Box::new(input.clone())),
        deps: vec![input],
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

/// Stack a sort mirror on top of a standalone logical tree.
pub(crate) fn wrap_logical_sort(
    input: LogicalNodeEnum,
    sort_items: Vec<SortItem>,
    col_names: Vec<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Sort(LogicalSortNode {
        id: next_node_id(),
        input: Some(Box::new(input.clone())),
        deps: vec![input],
        sort_items,
        limit: None,
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

/// Stack a limit/offset mirror on top of a standalone logical tree.
pub(crate) fn wrap_logical_limit(
    input: LogicalNodeEnum,
    offset: i64,
    count: i64,
    col_names: Vec<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Limit(LogicalLimitNode {
        id: next_node_id(),
        input: Some(Box::new(input.clone())),
        deps: vec![input],
        offset,
        count,
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

pub fn cross_join_plans(left: SubPlan, right: SubPlan) -> Result<SubPlan, PlannerError> {
    let left_root = match left.root {
        Some(ref r) => r,
        None => return Ok(right),
    };

    let right_root = match right.root {
        Some(ref r) => r,
        None => return Ok(left),
    };

    let (right_root, needs_id_update) = if let Some(expand_all) = right_root.as_expand_all() {
        if expand_all.get_input_var().is_none() {
            let marker_var = "__CROSSJOIN_ID_MARKER__".to_string();
            let mut new_expand = expand_all.clone();
            new_expand.set_input_var(marker_var);
            (new_expand.into_enum(), true)
        } else {
            (right_root.clone(), false)
        }
    } else {
        (right_root.clone(), false)
    };

    let mut join_node = CrossJoinNode::new(left_root.clone(), right_root.clone())
        .map_err(|e| PlannerError::JoinFailed(format!("Cross-connection failed: {}", e)))?;

    if needs_id_update {
        let join_id = join_node.id();
        let actual_var = format!("left_{}", join_id);

        if let Some(expand_all) = join_node.right_input().as_expand_all() {
            let mut new_expand = expand_all.clone();
            new_expand.set_input_var(actual_var);
            join_node = CrossJoinNode::new(left_root.clone(), new_expand.into_enum())
                .map_err(|e| PlannerError::JoinFailed(format!("Cross-connection failed: {}", e)))?;
        }
    }

    let output_var = if let Some(expand_all) = join_node.right_input().as_expand_all() {
        expand_all.get_input_var().map(|v| v.to_string())
    } else {
        None
    };

    if let Some(var) = output_var {
        join_node.set_output_var(var);
    }

    // Logical mirror: replicate the same structure on the logical roots —
    // an input-less expand on the right side receives the join id as its
    // input variable (matching the physical marker wiring).
    let logical_root = {
        let left_logical = left.logical_root().cloned();
        let right_logical = right.logical_root().cloned();
        match (left_logical, right_logical) {
            (Some(left), Some(mut right)) => {
                if let LogicalNodeEnum::ExpandAll(expand) = &mut right {
                    if expand.input_var.is_none() {
                        if let Some(var) = join_node
                            .right_input()
                            .as_expand_all()
                            .and_then(|e| e.get_input_var())
                        {
                            expand.input_var = Some(var.to_string());
                        }
                    }
                }
                Some(LogicalNodeEnum::CrossJoin(LogicalCrossJoinNode {
                    id: next_node_id(),
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                    hash_keys: vec![],
                    probe_keys: vec![],
                    deps: vec![left, right],
                    output_var: join_node.output_var().map(|s| s.to_string()),
                    col_names: vec![],
                    column_types: vec![],
                }))
            }
            _ => None,
        }
    };

    Ok(SubPlan {
        root: Some(join_node.into_enum()),
        tail: left.tail.or(right.tail),
        logical_root,
    })
}

pub fn connect_node_to_edge_expansion(
    node_plan: SubPlan,
    edge_plan: SubPlan,
    node_alias: &str,
) -> Result<SubPlan, PlannerError> {
    let node_root = node_plan
        .root
        .as_ref()
        .ok_or_else(|| PlannerError::PlanGenerationFailed("Node plan has no root".to_string()))?;

    let edge_root = edge_plan
        .root
        .as_ref()
        .ok_or_else(|| PlannerError::PlanGenerationFailed("Edge plan has no root".to_string()))?;

    if let Some(expand_all) = edge_root.as_expand_all() {
        let mut new_expand = expand_all.clone();
        new_expand.set_input_var(node_alias.to_string());
        new_expand.set_output_var(format!("expand_{}", new_expand.id()));

        // Capture the logical mirrors before the physical tails move.
        let node_logical = node_plan.logical_root().cloned();
        let edge_logical = edge_plan.logical_root().cloned();

        // Structurally close the plan: the node root becomes the expand
        // node's input inside the SubPlan itself.
        let mut connected = SubPlan::connect_upstream(
            SubPlan::from_single_node(new_expand.into_enum()),
            SubPlan::from_single_node(node_root.clone()),
        )?;
        connected.tail = node_plan.tail.or(edge_plan.tail);

        // Logical mirror: the expand node depends on the node root (the
        // converter wires deps into physical inputs).
        if let (Some(node_logical), Some(edge_logical)) = (node_logical, edge_logical) {
            if let LogicalNodeEnum::ExpandAll(mut expand) = edge_logical {
                expand.input_var = Some(node_alias.to_string());
                expand.output_var = Some(format!("expand_{}", expand.id()));
                expand.deps = vec![node_logical.clone()];
                connected.logical_root = Some(LogicalNodeEnum::ExpandAll(expand));
            } else {
                connected.logical_root = None;
            }
        }
        Ok(connected)
    } else {
        cross_join_plans(node_plan, edge_plan)
    }
}

pub fn join_edge_expansions(
    left_plan: SubPlan,
    right_plan: SubPlan,
    left_dst_alias: &str,
) -> Result<SubPlan, PlannerError> {
    let left_root = left_plan
        .root
        .as_ref()
        .ok_or_else(|| PlannerError::PlanGenerationFailed("Left plan has no root".to_string()))?;

    let right_root = right_plan
        .root
        .as_ref()
        .ok_or_else(|| PlannerError::PlanGenerationFailed("Right plan has no root".to_string()))?;

    if let Some(expand_all) = right_root.as_expand_all() {
        let mut new_expand = expand_all.clone();
        new_expand.set_input_var(left_dst_alias.to_string());
        new_expand.set_output_var(format!("expand_{}", new_expand.id()));

        // Capture the logical mirrors before the physical tails move.
        let left_logical = left_plan.logical_root().cloned();
        let right_logical = right_plan.logical_root().cloned();

        // Structurally close the plan: the left root becomes the expand
        // node's input inside the SubPlan itself.
        let mut connected = SubPlan::connect_upstream(
            SubPlan::from_single_node(new_expand.into_enum()),
            SubPlan::from_single_node(left_root.clone()),
        )?;
        connected.tail = left_plan.tail.or(right_plan.tail);

        // Logical mirror: the expand node depends on the left root.
        if let (Some(left_logical), Some(right_logical)) = (left_logical, right_logical) {
            if let LogicalNodeEnum::ExpandAll(mut expand) = right_logical {
                expand.input_var = Some(left_dst_alias.to_string());
                expand.output_var = Some(format!("expand_{}", expand.id()));
                expand.deps = vec![left_logical.clone()];
                connected.logical_root = Some(LogicalNodeEnum::ExpandAll(expand));
            } else {
                connected.logical_root = None;
            }
        }
        Ok(connected)
    } else {
        cross_join_plans(left_plan, right_plan)
    }
}

pub fn left_join_plans(left: SubPlan, right: SubPlan) -> Result<SubPlan, PlannerError> {
    let left_root = match left.root {
        Some(ref r) => r,
        None => return Ok(right),
    };

    let right_root = match right.root {
        Some(ref r) => r,
        None => return Ok(left),
    };

    let join_node = LeftJoinNode::new(left_root.clone(), right_root.clone(), vec![], vec![])
        .map_err(|e| PlannerError::JoinFailed(format!("Left connection failed: {}", e)))?;

    let logical_root = match (left.logical_root().cloned(), right.logical_root().cloned()) {
        (Some(left), Some(right)) => Some(LogicalNodeEnum::LeftJoin(LogicalLeftJoinNode {
            id: next_node_id(),
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
            hash_keys: vec![],
            probe_keys: vec![],
            deps: vec![left, right],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })),
        _ => None,
    };

    Ok(SubPlan {
        root: Some(join_node.into_enum()),
        tail: left.tail.or(right.tail),
        logical_root,
    })
}

pub fn union_plans(left: SubPlan, right: SubPlan) -> Result<SubPlan, PlannerError> {
    let left_root = match left.root {
        Some(ref r) => r,
        None => return Ok(right),
    };

    let right_root = match right.root {
        Some(ref r) => r,
        None => return Ok(left),
    };

    let union_node = UnionNode::new(left_root.clone(), right_root.clone(), true).map_err(|e| {
        PlannerError::PlanGenerationFailed(format!("Concatenation operation failed: {}", e))
    })?;

    // Logical mirror: the left chain is the single input, the right branch
    // rides in deps[1] (matching the physical converter).
    let logical_root = match (left.logical_root().cloned(), right.logical_root().cloned()) {
        (Some(left), Some(right)) => Some(LogicalNodeEnum::Union(LogicalUnionNode {
            id: next_node_id(),
            input: Some(Box::new(left.clone())),
            deps: vec![left, right],
            distinct: true,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })),
        _ => None,
    };

    Ok(SubPlan {
        root: Some(union_node.into_enum()),
        tail: left.tail.or(right.tail),
        logical_root,
    })
}

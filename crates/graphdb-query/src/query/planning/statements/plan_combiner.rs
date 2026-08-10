use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::{CrossJoinNode, LeftJoinNode, UnionNode};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;

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

    Ok(SubPlan {
        root: Some(join_node.into_enum()),
        tail: left.tail.or(right.tail),
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

        // Structurally close the plan: the node root becomes the expand
        // node's input inside the SubPlan itself.
        let mut connected = SubPlan::connect_upstream(
            SubPlan::from_single_node(new_expand.into_enum()),
            SubPlan::from_single_node(node_root.clone()),
        )?;
        connected.tail = node_plan.tail.or(edge_plan.tail);
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

        // Structurally close the plan: the left root becomes the expand
        // node's input inside the SubPlan itself.
        let mut connected = SubPlan::connect_upstream(
            SubPlan::from_single_node(new_expand.into_enum()),
            SubPlan::from_single_node(left_root.clone()),
        )?;
        connected.tail = left_plan.tail.or(right_plan.tail);
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

    Ok(SubPlan {
        root: Some(join_node.into_enum()),
        tail: left.tail.or(right.tail),
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

    Ok(SubPlan {
        root: Some(union_node.into_enum()),
        tail: left.tail.or(right.tail),
    })
}

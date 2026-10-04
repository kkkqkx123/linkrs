use super::*;

pub(super) fn convert_inner_join(
    n: crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let hash_keys = n.hash_keys;
    let probe_keys = n.probe_keys;
    let mut node = crate::planning::plan::core::nodes::join::join_node::InnerJoinNode::new(
        left, right, hash_keys, probe_keys,
    )
    .expect("Failed to construct InnerJoinNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::InnerJoin(node)
}

pub(super) fn convert_left_join(
    n: crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let hash_keys = n.hash_keys;
    let probe_keys = n.probe_keys;
    let mut node = crate::planning::plan::core::nodes::join::join_node::LeftJoinNode::new(
        left, right, hash_keys, probe_keys,
    )
    .expect("Failed to construct LeftJoinNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::LeftJoin(node)
}

pub(super) fn convert_right_join(
    n: crate::planning::plan::logical::logical_nodes::join::LogicalRightJoinNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node = crate::planning::plan::core::nodes::join::join_node::RightJoinNode::new(
        left,
        right,
        n.hash_keys,
        n.probe_keys,
    )
    .expect("Failed to construct RightJoinNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::RightJoin(node)
}

pub(super) fn convert_cross_join(
    n: crate::planning::plan::logical::logical_nodes::join::LogicalCrossJoinNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node =
        crate::planning::plan::core::nodes::join::join_node::CrossJoinNode::new(left, right)
            .expect("Failed to construct CrossJoinNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::CrossJoin(node)
}

pub(super) fn convert_full_outer_join(
    n: crate::planning::plan::logical::logical_nodes::join::LogicalFullOuterJoinNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node = crate::planning::plan::core::nodes::join::join_node::FullOuterJoinNode::new(
        left,
        right,
        n.hash_keys,
        n.probe_keys,
    )
    .expect("Failed to construct FullOuterJoinNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::FullOuterJoin(node)
}

pub(super) fn convert_semi_join(
    n: crate::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let node = match &n.join_condition {
        Some(condition) => {
            crate::planning::plan::core::nodes::join::join_node::SemiJoinNode::new_with_condition(
                left,
                right,
                n.hash_keys,
                n.probe_keys,
                condition.clone(),
                n.anti,
            )
            .expect("Failed to construct SemiJoinNode")
        }
        None => crate::planning::plan::core::nodes::join::join_node::SemiJoinNode::new(
            left,
            right,
            n.hash_keys,
            n.probe_keys,
            n.anti,
        )
        .expect("Failed to construct SemiJoinNode"),
    };
    let mut node = node;
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::SemiJoin(node)
}

/// Lower an N-way WCO intersect to the dedicated physical node.
///
/// The probe side converts to `input`/`deps[0]` and each build side to
/// `deps[1..]`; the streaming `WcoIntersectOperator` resolves the bound
/// and intersect columns by variable name at execution time. Build sides
/// must therefore carry both their bound variable and the intersect
/// variable (the join-order DP selects endpoint-covering build plans);
/// the assembler reports a build error otherwise instead of silently
/// producing wrong rows.
pub(super) fn lower_wco_intersect(
    n: crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode,
) -> PlanNodeEnum {
    use crate::planning::plan::core::nodes::join::wco_intersect_node::WcoIntersectNode;
    let crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode {
        deps,
        intersect_key,
        bound_keys,
        output_var,
        col_names,
        column_types,
        ..
    } = n;
    let mut inputs = deps.into_iter();
    let probe = inputs.next().expect("WCO intersect needs a probe side");
    let probe_physical = convert_logical_to_physical(probe);
    let builds: Vec<PlanNodeEnum> = inputs.map(convert_logical_to_physical).collect();
    let mut node = WcoIntersectNode::new(probe_physical, builds, intersect_key, bound_keys)
        .expect("Failed to construct WcoIntersectNode");
    if let Some(var) = output_var {
        node.set_output_var(var);
    }
    node.set_col_names(col_names);
    node.set_column_types(column_types);
    PlanNodeEnum::WcoIntersect(node)
}

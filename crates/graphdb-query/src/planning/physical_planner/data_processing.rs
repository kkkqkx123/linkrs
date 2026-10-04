use super::*;

pub(super) fn convert_data_collect(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalDataCollectNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("DataCollectNode missing input"));
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::DataCollectNode::new(
                input, &n.collect_kind,
            ).expect("Failed to construct DataCollectNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::DataCollect(node)
}

pub(super) fn convert_remove(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalRemoveNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("RemoveNode missing input"));
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::RemoveNode::new(
                input, n.remove_items,
            ).expect("Failed to construct RemoveNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::Remove(node)
}

pub(super) fn convert_pattern_apply(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(n.left_input().clone());
    let right_input = convert_logical_to_physical(n.right_input().clone());
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode::new(
                input, right_input, n.hash_keys().to_vec(), n.probe_keys().to_vec(),
                n.is_anti_predicate,
            ).expect("Failed to construct PatternApplyNode");
    if let Some(var) = n.output_var() {
        node.set_output_var(var.to_string());
    }
    node.set_col_names(n.col_names().to_vec());
    PlanNodeEnum::PatternApply(node)
}

pub(super) fn convert_correlated_apply(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalCorrelatedApplyNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(n.left_input().clone());
    let right_input = convert_logical_to_physical(n.right_input().clone());
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::CorrelatedApplyNode::new(
                input, right_input, n.is_anti_predicate,
            ).expect("Failed to construct CorrelatedApplyNode");
    if let Some(var) = n.output_var() {
        node.set_output_var(var.to_string());
    }
    node.set_col_names(n.col_names().to_vec());
    PlanNodeEnum::CorrelatedApply(node)
}

pub(super) fn convert_roll_up_apply(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalRollUpApplyNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("RollUpApplyNode missing input"));
    let fallback =
        crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new().into_enum();
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::RollUpApplyNode::new(
                input, fallback, n.compare_cols, n.collect_col.clone(),
            ).expect("Failed to construct RollUpApplyNode");
    if let Some(var) = n.left_input_var {
        node.set_left_input_var(var);
    }
    if let Some(var) = n.right_input_var {
        node.set_right_input_var(var);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::RollUpApply(node)
}

pub(super) fn convert_union(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnionNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(n.deps[0].clone());
    let union_input = convert_logical_to_physical(n.deps[1].clone());
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode::new(
                input, union_input, n.distinct,
            ).expect("Failed to construct UnionNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Union(node)
}

pub(super) fn convert_minus(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalMinusNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(n.deps[0].clone());
    let minus_input = convert_logical_to_physical(n.deps[1].clone());
    let mut node =
        crate::planning::plan::core::nodes::graph_operations::set_operations_node::MinusNode::new(
            input,
            minus_input,
        )
        .expect("Failed to construct MinusNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Minus(node)
}

pub(super) fn convert_intersect(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalIntersectNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(n.deps[0].clone());
    let intersect_input = convert_logical_to_physical(n.deps[1].clone());
    let mut node = crate::planning::plan::core::nodes::graph_operations::set_operations_node::IntersectNode::new(
                input, intersect_input,
            ).expect("Failed to construct IntersectNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Intersect(node)
}

pub(super) fn convert_unwind(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("UnwindNode missing input"));
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode::new(
                input, &n.alias, n.list_expression,
            ).expect("Failed to construct UnwindNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Unwind(node)
}

pub(super) fn convert_materialize(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalMaterializeNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("MaterializeNode missing input"));
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::MaterializeNode::new(
                input,
            ).expect("Failed to construct MaterializeNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Materialize(node)
}

pub(super) fn convert_assign(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("AssignNode missing input"));
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::AssignNode::new(
                input, n.assignments,
            ).expect("Failed to construct AssignNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Assign(node)
}

pub(super) fn convert_apply(
    n: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalApplyNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical((*n.left_input()).clone());
    let right = convert_logical_to_physical((*n.right_input()).clone());
    let mut node = crate::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyNode::new(
                left, right, n.correlated_cols().to_vec(), *n.apply_kind(),
            ).expect("Failed to construct ApplyNode");
    if let Some(var) = n.output_var().map(|s| s.to_string()) {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names().to_vec());
    PlanNodeEnum::Apply(node)
}

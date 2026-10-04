use super::*;

pub(super) fn convert_expand(
    n: crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandNode,
) -> PlanNodeEnum {
    let deps: Vec<PlanNodeEnum> = n
        .deps
        .into_iter()
        .map(convert_logical_to_physical)
        .collect();
    let mut node = crate::planning::plan::core::nodes::traversal::traversal_node::ExpandNode::new(
        n.space_id,
        n.edge_types,
        n.direction,
    );
    if let Some(expr) = n.filter {
        node.set_filter(expr);
    }
    if let Some(tag) = n.dst_tag {
        node.set_dst_tag(tag);
    }
    for dep in deps {
        node.add_input(dep);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Expand(node)
}

pub(super) fn convert_expand_all(
    n: crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode,
) -> PlanNodeEnum {
    let deps: Vec<PlanNodeEnum> = n
        .deps
        .into_iter()
        .map(convert_logical_to_physical)
        .collect();
    let mut node =
        crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode::new(
            n.space_id,
            n.edge_types,
            &n.direction,
        );
    node.set_any_edge_type(n.any_edge_type);
    if let Some(limit) = n.step_limit {
        node.set_step_limit(limit);
    }
    if let Some(limits) = n.step_limits {
        node.set_step_limits(limits);
    }
    node.set_join_input(n.join_input);
    node.set_sample(n.sample);
    node.set_edge_props(n.edge_props);
    node.set_vertex_props(n.vertex_props);
    if let Some(expr) = n.filter {
        node.set_filter(expr);
    }
    if !n.src_vids.is_empty() {
        node.set_src_vids(n.src_vids);
    }
    node.set_include_empty_paths(n.include_empty_paths);
    node.set_path_semantic(n.path_semantic);
    if let Some(var) = n.input_var {
        node.set_input_var(var);
    }
    if let Some(tag) = n.dst_tag {
        node.set_dst_tag(tag);
    }
    for dep in deps {
        node.add_input(dep);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::ExpandAll(node)
}

pub(super) fn convert_traverse(
    n: crate::planning::plan::logical::logical_nodes::traversal::LogicalTraverseNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("TraverseNode missing input"));
    let mut node = crate::planning::plan::core::nodes::traversal::traversal_node::TraverseNode::new(
        n.space_id,
        &n.start_vids,
        n.min_steps,
        n.max_steps,
    );
    if let Some(end) = n.end_vids {
        node.set_end_vids(&end);
    }
    node.set_edge_types(n.edge_types);
    node.set_direction(n.direction);
    node.set_path_semantic(n.path_semantic);
    if let Some(expr) = n.e_filter {
        node.set_e_filter(expr);
    }
    if let Some(expr) = n.v_filter {
        node.set_v_filter(expr);
    }
    if let Some(expr) = n.first_step_filter {
        node.set_first_step_filter(expr);
    }
    if let Some(tag) = n.dst_tag {
        node.set_dst_tag(tag);
    }
    node.set_input(input);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Traverse(node)
}

pub(super) fn convert_append_vertices(
    n: crate::planning::plan::logical::logical_nodes::traversal::LogicalAppendVerticesNode,
) -> PlanNodeEnum {
    let deps: Vec<PlanNodeEnum> = n
        .deps
        .into_iter()
        .map(convert_logical_to_physical)
        .collect();
    let mut node =
        crate::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode::new(
            n.space_id,
            &n.vertex_tag,
        );
    node.set_vertex_props(n.vertex_props);
    if let Some(expr) = n.filter {
        node.set_filter(expr);
    }
    if let Some(expr) = n.src_expression {
        node.set_src_expression(expr);
    }
    if let Some(alias) = n.node_alias {
        node.set_node_alias(alias);
    }
    for dep in deps {
        node.add_input(dep);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::AppendVertices(node)
}

pub(super) fn convert_bi_expand(
    n: crate::planning::plan::logical::logical_nodes::traversal::LogicalBiExpandNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node = crate::planning::plan::core::nodes::traversal::traversal_node::BiExpandNode::new(
        left,
        right,
        n.space_id,
        n.left_direction,
        n.right_direction,
        n.edge_types,
        n.max_hops,
    );
    if let Some(var) = n.meeting_point_var {
        node.set_meeting_point_var(var);
    }
    if let Some(tag) = n.dst_tag {
        node.set_dst_tag(tag);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::BiExpand(node)
}

pub(super) fn convert_bi_traverse(
    n: crate::planning::plan::logical::logical_nodes::traversal::LogicalBiTraverseNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    use crate::planning::plan::core::nodes::traversal::traversal_node::BiTraverseNodeParams;
    let params = BiTraverseNodeParams {
        left_input: left,
        right_input: right,
        space_id: n.space_id,
        left_src_var: n.left_src_var,
        right_src_var: n.right_src_var,
        edge_types: n.edge_types,
        left_direction: n.left_direction,
        right_direction: n.right_direction,
        min_hops: n.min_hops,
        max_hops: n.max_hops,
        path_var: n.path_var,
    };
    let mut node =
        crate::planning::plan::core::nodes::traversal::traversal_node::BiTraverseNode::new(params);
    if let Some(alias) = n.edge_alias {
        node.set_edge_alias(alias);
    }
    if let Some(alias) = n.vertex_alias {
        node.set_vertex_alias(alias);
    }
    if let Some(tag) = n.dst_tag {
        node.set_dst_tag(tag);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::BiTraverse(node)
}

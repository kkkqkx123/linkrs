use super::*;

pub(super) fn convert_multi_shortest_path(
    n: crate::planning::plan::logical::logical_nodes::algorithm::LogicalMultiShortestPathNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node =
        crate::planning::plan::core::nodes::traversal::path_algorithms::MultiShortestPathNode::new(
            left, right, n.steps,
        );
    node.set_left_vid_var(&n.left_vid_var);
    node.set_right_vid_var(&n.right_vid_var);
    node.set_edge_types(n.edge_types);
    node.set_direction(n.direction);
    node.set_target_vertex_ids(n.target_vertex_ids);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::MultiShortestPath(node)
}

pub(super) fn convert_b_f_s_shortest(
    n: crate::planning::plan::logical::logical_nodes::algorithm::LogicalBFSShortestNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node =
        crate::planning::plan::core::nodes::traversal::path_algorithms::BFSShortestNode::new(
            left,
            right,
            n.space_id,
            n.steps,
            n.edge_types,
            n.with_cycle,
        );
    if n.with_loop {
        node.set_loop(true);
    }
    if n.reverse {
        node.set_reverse(true);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::BFSShortest(node)
}

pub(super) fn convert_all_paths(
    n: crate::planning::plan::logical::logical_nodes::algorithm::LogicalAllPathsNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node =
        crate::planning::plan::core::nodes::traversal::path_algorithms::AllPathsNode::new(
            left,
            right,
            n.space_id,
            n.steps,
            n.edge_types,
            n.min_hop,
            n.max_hop,
            n.acyclic,
        );
    node.set_start_vertex_ids(n.start_vertex_ids);
    node.set_end_vertex_ids(n.end_vertex_ids);
    node.set_direction(n.direction);
    if n.limit != 0 {
        node.set_limit(n.limit);
    }
    if n.offset != 0 {
        node.set_offset(n.offset);
    }
    if let Some(filter) = n.filter {
        node.set_filter(filter);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::AllPaths(node)
}

pub(super) fn convert_shortest_path(
    n: crate::planning::plan::logical::logical_nodes::algorithm::LogicalShortestPathNode,
) -> PlanNodeEnum {
    let left = convert_logical_to_physical(*n.left);
    let right = convert_logical_to_physical(*n.right);
    let mut node =
        crate::planning::plan::core::nodes::traversal::path_algorithms::ShortestPathNode::new(
            left,
            right,
            n.space_id,
            n.edge_types,
            n.max_step,
        );
    node.set_start_vertex_ids(n.start_vertex_ids);
    node.set_end_vertex_ids(n.end_vertex_ids);
    if let Some(expr) = n.weight_expression {
        node.set_weight_expression(expr);
    }
    if let Some(expr) = n.heuristic_expression {
        node.set_heuristic_expression(expr);
    }
    if n.no_reverse {
        node.set_no_reverse(true);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::ShortestPath(node)
}

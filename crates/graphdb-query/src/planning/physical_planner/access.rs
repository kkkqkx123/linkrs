use super::*;

pub(super) fn convert_start(
    n: crate::planning::plan::logical::logical_nodes::access::LogicalStartNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Start(node)
}

pub(super) fn convert_get_vertices(
    n: crate::planning::plan::logical::logical_nodes::access::LogicalGetVerticesNode,
) -> PlanNodeEnum {
    let deps: Vec<PlanNodeEnum> = n
        .deps
        .into_iter()
        .map(convert_logical_to_physical)
        .collect();
    let mut node =
        crate::planning::plan::core::nodes::access::graph_scan_node::GetVerticesNode::new(
            n.space_id,
            &n.space_name,
            &n.src_vids,
        );
    node.set_deps(deps);
    node.set_tag_props(n.tag_props);
    if let Some(tag) = n.tag {
        node.set_tag(tag);
    }
    if let Some(expr) = n.expression {
        node.set_filter(expr);
    }
    node.set_dedup(n.dedup);
    if let Some(limit) = n.limit {
        node.set_limit(limit);
    }
    node.set_projected_properties(n.projected_properties);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::GetVertices(node)
}

pub(super) fn convert_get_edges(
    n: crate::planning::plan::logical::logical_nodes::access::LogicalGetEdgesNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::access::graph_scan_node::GetEdgesNode::new(
        n.space_id,
        &n.src,
        &n.edge_type,
        &n.rank,
        &n.dst,
    );
    if let Some(expr) = n.expression {
        node.set_filter(expr);
    }
    if let Some(limit) = n.limit {
        node.set_limit(limit);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::GetEdges(node)
}

pub(super) fn convert_get_neighbors(
    n: crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode,
) -> PlanNodeEnum {
    let deps: Vec<PlanNodeEnum> = n
        .deps
        .into_iter()
        .map(convert_logical_to_physical)
        .collect();
    let mut node =
        crate::planning::plan::core::nodes::access::graph_scan_node::GetNeighborsNode::new(
            n.space_id,
            &n.src_vids,
        );
    node.set_deps(deps);
    node.set_edge_types(n.edge_types);
    node.set_direction(&n.direction);
    if let Some(tag) = n.tag {
        node.set_tag(tag);
    }
    if let Some(expr) = n.expression {
        node.set_filter(expr);
    }
    node.set_dedup(n.dedup);
    if let Some(limit) = n.limit {
        node.set_limit(limit);
    }
    node.set_projected_properties(n.projected_properties);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::GetNeighbors(node)
}

pub(super) fn convert_scan_vertices(
    n: crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode::new(
            n.space_id,
            &n.space_name,
        );
    if let Some(tag) = n.tag {
        node.set_tag(&tag);
    }
    if let Some(expr) = n.expression {
        node.set_filter(expr);
    }
    if let Some(limit) = n.limit {
        node.set_limit(limit);
    }
    node.set_projected_properties(n.projected_properties);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::ScanVertices(node)
}

pub(super) fn convert_scan_edges(
    n: crate::planning::plan::logical::logical_nodes::access::LogicalScanEdgesNode,
) -> PlanNodeEnum {
    let edge_type = n.edge_type.unwrap_or_default();
    let mut node = crate::planning::plan::core::nodes::access::graph_scan_node::ScanEdgesNode::new(
        n.space_id, &edge_type,
    );
    if let Some(expr) = n.expression {
        node.set_filter(expr);
    }
    if let Some(limit) = n.limit {
        node.set_limit(limit);
    }
    node.set_projected_properties(n.projected_properties);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::ScanEdges(node)
}

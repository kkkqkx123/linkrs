use super::*;

pub(super) fn convert_project(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("ProjectNode missing input"));
    let mut node = crate::planning::plan::core::nodes::operation::project_node::ProjectNode::new(
        input, n.columns,
    )
    .expect("Failed to construct ProjectNode")
    .with_subqueries(n.subqueries);
    node.set_has_folded_expressions(n.has_folded_expressions);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    node.set_column_types(n.column_types);
    PlanNodeEnum::Project(node)
}

pub(super) fn convert_filter(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("FilterNode missing input"));
    let mut node = crate::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
        input,
        n.condition,
    )
    .expect("Failed to construct FilterNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Filter(node)
}

pub(super) fn convert_sort(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("SortNode missing input"));
    let mut node = crate::planning::plan::core::nodes::operation::sort_node::SortNode::new(
        input,
        n.sort_items,
    )
    .expect("Failed to construct SortNode");
    if let Some(l) = n.limit {
        node.set_limit(l);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Sort(node)
}

pub(super) fn convert_limit(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalLimitNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("LimitNode missing input"));
    let mut node = crate::planning::plan::core::nodes::operation::sort_node::LimitNode::new(
        input, n.offset, n.count,
    )
    .expect("Failed to construct LimitNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Limit(node)
}

pub(super) fn convert_skip(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalSkipNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("SkipNode missing input"));
    let mut node = crate::planning::plan::core::nodes::operation::sort_node::LimitNode::new(
        input,
        n.offset,
        i64::MAX,
    )
    .expect("Failed to construct LimitNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Limit(node)
}

pub(super) fn convert_top_n(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalTopNNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("TopNNode missing input"));
    let mut node = crate::planning::plan::core::nodes::operation::sort_node::TopNNode::new(
        input,
        n.sort_items,
        n.limit,
    )
    .expect("Failed to construct TopNNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::TopN(node)
}

pub(super) fn convert_sample(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalSampleNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("SampleNode missing input"));
    let mut node =
        crate::planning::plan::core::nodes::operation::sample_node::SampleNode::new(input, n.count)
            .expect("Failed to construct SampleNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Sample(node)
}

pub(super) fn convert_dedup(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalDedupNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("DedupNode missing input"));
    let mut node = match crate::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode::new(input) {
                Ok(n) => n,
                Err(e) => panic!("Failed to construct DedupNode: {}", e),
            };
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Dedup(node)
}

pub(super) fn convert_aggregate(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("AggregateNode missing input"));
    let group_keys: Vec<String> = n
        .group_key_exprs
        .iter()
        .map(|e| e.to_expression_string())
        .collect();
    let group_key_exprs = n.group_key_exprs.clone();
    let aggregation_functions = n.aggregation_functions.clone();
    let aggregation_args = n.aggregation_args.clone();
    let aggregation_distinct = n.aggregation_distinct.clone();
    let aggregation_filters = n.aggregation_filters.clone();
    let grouping_sets = n.grouping_sets.clone();
    let mut node =
        crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode::new(
            input,
            group_keys,
            aggregation_functions,
        )
        .expect("Failed to construct AggregateNode");
    // Preserve lossless identities for reverse conversion; execution
    // still uses `group_keys` strings.
    node.set_group_key_exprs(group_key_exprs);
    node.set_aggregation_args(aggregation_args);
    node.set_aggregation_distinct(aggregation_distinct);
    node.set_aggregation_filters(aggregation_filters);
    node.set_grouping_sets(grouping_sets);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Aggregate(node)
}

pub(super) fn convert_window(
    n: crate::planning::plan::logical::logical_nodes::operation::LogicalWindowNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("WindowNode missing input"));
    let mut node =
        crate::planning::plan::core::nodes::graph_operations::window_node::WindowNode::new(
            input,
            n.window_functions,
        )
        .expect("Failed to construct WindowNode");
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Window(node)
}

pub(super) fn convert_flatten(
    n: crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode,
) -> PlanNodeEnum {
    let input = convert_logical_to_physical(*n.input.expect("Flatten missing input"));
    let mut node = crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode::new(
        input,
        n.group_pos,
    )
    .expect("Failed to construct FlattenNode");
    node.set_group_columns(n.group_columns);
    if let Some(groups) = n.expected_groups {
        node.set_expected_groups(groups);
    }
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Flatten(node)
}

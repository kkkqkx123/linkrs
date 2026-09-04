use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNode, SingleInputNode,
};
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::QueryContext;

pub trait PhysicalPlanner: Send + Sync + std::fmt::Debug {
    fn plan(&self, logical: LogicalNodeEnum, qctx: &QueryContext) -> PlanNodeEnum;
}

#[derive(Debug)]
pub struct DefaultPhysicalPlanner;

impl Default for DefaultPhysicalPlanner {
    fn default() -> Self {
        Self
    }
}

impl DefaultPhysicalPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl PhysicalPlanner for DefaultPhysicalPlanner {
    fn plan(&self, logical: LogicalNodeEnum, _qctx: &QueryContext) -> PlanNodeEnum {
        convert_logical_to_physical(logical)
    }
}

pub(crate) fn convert_logical_to_physical(logical: LogicalNodeEnum) -> PlanNodeEnum {
    match logical {
        // ==================== Access Nodes ====================
        LogicalNodeEnum::Start(n) => {
            let mut node =
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::Start(node)
        }

        LogicalNodeEnum::GetVertices(n) => {
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

        LogicalNodeEnum::GetEdges(n) => {
            let mut node =
                crate::planning::plan::core::nodes::access::graph_scan_node::GetEdgesNode::new(
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

        LogicalNodeEnum::GetNeighbors(n) => {
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

        LogicalNodeEnum::ScanVertices(n) => {
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

        LogicalNodeEnum::ScanEdges(n) => {
            let edge_type = n.edge_type.unwrap_or_default();
            let mut node =
                crate::planning::plan::core::nodes::access::graph_scan_node::ScanEdgesNode::new(
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

        // ==================== Operation Nodes ====================
        LogicalNodeEnum::Project(n) => {
            let input = convert_logical_to_physical(*n.input.expect("ProjectNode missing input"));
            let mut node =
                crate::planning::plan::core::nodes::operation::project_node::ProjectNode::new(
                    input, n.columns,
                )
                .expect("Failed to construct ProjectNode");
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            if !n.col_names.is_empty() {
                node.set_col_names(n.col_names);
            }
            node.set_column_types(n.column_types);
            PlanNodeEnum::Project(node)
        }

        LogicalNodeEnum::Filter(n) => {
            let input = convert_logical_to_physical(*n.input.expect("FilterNode missing input"));
            let mut node =
                crate::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
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

        LogicalNodeEnum::Sort(n) => {
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

        LogicalNodeEnum::Limit(n) => {
            let input = convert_logical_to_physical(*n.input.expect("LimitNode missing input"));
            let mut node =
                crate::planning::plan::core::nodes::operation::sort_node::LimitNode::new(
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

        LogicalNodeEnum::Skip(n) => {
            let input = convert_logical_to_physical(*n.input.expect("SkipNode missing input"));
            let mut node =
                crate::planning::plan::core::nodes::operation::sort_node::LimitNode::new(
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

        LogicalNodeEnum::TopN(n) => {
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

        LogicalNodeEnum::Sample(n) => {
            let input = convert_logical_to_physical(*n.input.expect("SampleNode missing input"));
            let mut node =
                crate::planning::plan::core::nodes::operation::sample_node::SampleNode::new(
                    input, n.count,
                )
                .expect("Failed to construct SampleNode");
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::Sample(node)
        }

        LogicalNodeEnum::Dedup(n) => {
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

        LogicalNodeEnum::Aggregate(n) => {
            let input = convert_logical_to_physical(*n.input.expect("AggregateNode missing input"));
            let group_keys: Vec<String> = n
                .group_key_exprs
                .iter()
                .map(|e| e.to_expression_string())
                .collect();
            let group_key_exprs = n.group_key_exprs.clone();
            let aggregation_functions = n.aggregation_functions.clone();
            let aggregation_distinct = n.aggregation_distinct.clone();
            let aggregation_filters = n.aggregation_filters.clone();
            let grouping_sets = n.grouping_sets.clone();
            let mut node = crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode::new(
                input, group_keys, aggregation_functions,
            ).expect("Failed to construct AggregateNode");
            // Preserve lossless identities for reverse conversion; execution
            // still uses `group_keys` strings.
            node.set_group_key_exprs(group_key_exprs);
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

        LogicalNodeEnum::Window(n) => {
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

        // ==================== Join Nodes ====================
        //
        // The converter produces only the logical join variants
        // (InnerJoin/LeftJoin); hash keys remain attached to them and the
        // arena builder decides the physical hash vs nested-loop algorithm.
        LogicalNodeEnum::InnerJoin(n) => {
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

        LogicalNodeEnum::LeftJoin(n) => {
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

        LogicalNodeEnum::RightJoin(n) => {
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

        LogicalNodeEnum::CrossJoin(n) => {
            let left = convert_logical_to_physical(*n.left);
            let right = convert_logical_to_physical(*n.right);
            let mut node = crate::planning::plan::core::nodes::join::join_node::CrossJoinNode::new(
                left, right,
            )
            .expect("Failed to construct CrossJoinNode");
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::CrossJoin(node)
        }

        LogicalNodeEnum::FullOuterJoin(n) => {
            let left = convert_logical_to_physical(*n.left);
            let right = convert_logical_to_physical(*n.right);
            let mut node =
                crate::planning::plan::core::nodes::join::join_node::FullOuterJoinNode::new(
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

        LogicalNodeEnum::SemiJoin(n) => {
            let left = convert_logical_to_physical(*n.left);
            let right = convert_logical_to_physical(*n.right);
            let node = match &n.join_condition {
                Some(condition) => crate::planning::plan::core::nodes::join::join_node::SemiJoinNode::new_with_condition(
                    left,
                    right,
                    n.hash_keys,
                    n.probe_keys,
                    condition.clone(),
                    n.anti,
                )
                .expect("Failed to construct SemiJoinNode"),
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

        // ==================== Traversal Nodes ====================
        LogicalNodeEnum::Expand(n) => {
            let deps: Vec<PlanNodeEnum> = n
                .deps
                .into_iter()
                .map(convert_logical_to_physical)
                .collect();
            let mut node =
                crate::planning::plan::core::nodes::traversal::traversal_node::ExpandNode::new(
                    n.space_id,
                    n.edge_types,
                    n.direction,
                );
            if let Some(expr) = n.filter {
                node.set_filter(expr);
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

        LogicalNodeEnum::ExpandAll(n) => {
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
            if let Some(var) = n.input_var {
                node.set_input_var(var);
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

        LogicalNodeEnum::Traverse(n) => {
            let input = convert_logical_to_physical(*n.input.expect("TraverseNode missing input"));
            let mut node =
                crate::planning::plan::core::nodes::traversal::traversal_node::TraverseNode::new(
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
            if let Some(expr) = n.e_filter {
                node.set_e_filter(expr);
            }
            if let Some(expr) = n.v_filter {
                node.set_v_filter(expr);
            }
            if let Some(expr) = n.first_step_filter {
                node.set_first_step_filter(expr);
            }
            node.set_input(input);
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::Traverse(node)
        }

        LogicalNodeEnum::AppendVertices(n) => {
            let deps: Vec<PlanNodeEnum> = n
                .deps
                .into_iter()
                .map(convert_logical_to_physical)
                .collect();
            let mut node = crate::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode::new(
                n.space_id, &n.vertex_tag,
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

        LogicalNodeEnum::BiExpand(n) => {
            let left = convert_logical_to_physical(*n.left);
            let right = convert_logical_to_physical(*n.right);
            let mut node =
                crate::planning::plan::core::nodes::traversal::traversal_node::BiExpandNode::new(
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
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::BiExpand(node)
        }

        LogicalNodeEnum::BiTraverse(n) => {
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
                crate::planning::plan::core::nodes::traversal::traversal_node::BiTraverseNode::new(
                    params,
                );
            if let Some(alias) = n.edge_alias {
                node.set_edge_alias(alias);
            }
            if let Some(alias) = n.vertex_alias {
                node.set_vertex_alias(alias);
            }
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::BiTraverse(node)
        }

        // ==================== Control Flow Nodes ====================
        LogicalNodeEnum::Argument(n) => {
            let mut node = crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode::new(next_node_id(), &n.var);
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::Argument(node)
        }

        LogicalNodeEnum::Loop(n) => {
            let body = n.body().cloned().map(convert_logical_to_physical);
            let mut node =
                crate::planning::plan::core::nodes::control_flow::control_flow_node::LoopNode::new(
                    next_node_id(),
                    n.condition().clone(),
                );
            if let Some(b) = body {
                node.set_body(b);
            }
            if let Some(var) = n.output_var() {
                node.set_output_var(var.to_string());
            }
            node.set_col_names(n.col_names().to_vec());
            PlanNodeEnum::Loop(node)
        }

        LogicalNodeEnum::PassThrough(n) => {
            let mut node = crate::planning::plan::core::nodes::control_flow::control_flow_node::PassThroughNode::new(next_node_id());
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::PassThrough(node)
        }

        LogicalNodeEnum::Select(n) => {
            let mut node = crate::planning::plan::core::nodes::control_flow::control_flow_node::SelectNode::new(
                next_node_id(), n.condition().clone(),
            );
            if let Some(if_branch) = n.if_branch().cloned() {
                node.set_if_branch(convert_logical_to_physical(if_branch));
            }
            if let Some(else_branch) = n.else_branch().cloned() {
                node.set_else_branch(convert_logical_to_physical(else_branch));
            }
            if let Some(var) = n.output_var() {
                node.set_output_var(var.to_string());
            }
            node.set_col_names(n.col_names().to_vec());
            PlanNodeEnum::Select(node)
        }

        LogicalNodeEnum::BeginTransaction(n) => {
            let mut node = crate::planning::plan::core::nodes::control_flow::control_flow_node::BeginTransactionNode::new(next_node_id());
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::BeginTransaction(node)
        }

        LogicalNodeEnum::Commit(n) => {
            let mut node = crate::planning::plan::core::nodes::control_flow::control_flow_node::CommitNode::new(next_node_id());
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::Commit(node)
        }

        LogicalNodeEnum::Rollback(n) => {
            let mut node = crate::planning::plan::core::nodes::control_flow::control_flow_node::RollbackNode::new(next_node_id());
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::Rollback(node)
        }

        // ==================== Data Processing Nodes ====================
        LogicalNodeEnum::DataCollect(n) => {
            let input =
                convert_logical_to_physical(*n.input.expect("DataCollectNode missing input"));
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

        LogicalNodeEnum::Remove(n) => {
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

        LogicalNodeEnum::PatternApply(n) => {
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

        LogicalNodeEnum::CorrelatedApply(n) => {
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

        LogicalNodeEnum::RollUpApply(n) => {
            let input =
                convert_logical_to_physical(*n.input.expect("RollUpApplyNode missing input"));
            let fallback =
                crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new()
                    .into_enum();
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

        LogicalNodeEnum::Union(n) => {
            let input = convert_logical_to_physical(*n.input.expect("UnionNode missing input"));
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

        LogicalNodeEnum::Minus(n) => {
            let input = convert_logical_to_physical(*n.input.expect("MinusNode missing input"));
            let minus_input = convert_logical_to_physical(n.deps[1].clone());
            let mut node = crate::planning::plan::core::nodes::graph_operations::set_operations_node::MinusNode::new(
                input, minus_input,
            ).expect("Failed to construct MinusNode");
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::Minus(node)
        }

        LogicalNodeEnum::Intersect(n) => {
            let input = convert_logical_to_physical(*n.input.expect("IntersectNode missing input"));
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

        LogicalNodeEnum::Unwind(n) => {
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

        LogicalNodeEnum::Materialize(n) => {
            let input =
                convert_logical_to_physical(*n.input.expect("MaterializeNode missing input"));
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

        LogicalNodeEnum::Assign(n) => {
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

        LogicalNodeEnum::Apply(n) => {
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

        // ==================== Algorithm Nodes ====================
        LogicalNodeEnum::MultiShortestPath(n) => {
            let left = convert_logical_to_physical(*n.left);
            let right = convert_logical_to_physical(*n.right);
            let mut node = crate::planning::plan::core::nodes::traversal::path_algorithms::MultiShortestPathNode::new(
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

        LogicalNodeEnum::BFSShortest(n) => {
            let left = convert_logical_to_physical(*n.left);
            let right = convert_logical_to_physical(*n.right);
            let mut node = crate::planning::plan::core::nodes::traversal::path_algorithms::BFSShortestNode::new(
                left, right, n.space_id, n.steps, n.edge_types, n.with_cycle,
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

        LogicalNodeEnum::AllPaths(n) => {
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

        LogicalNodeEnum::ShortestPath(n) => {
            let left = convert_logical_to_physical(*n.left);
            let right = convert_logical_to_physical(*n.right);
            let mut node = crate::planning::plan::core::nodes::traversal::path_algorithms::ShortestPathNode::new(
                left, right, n.space_id, n.edge_types, n.max_step,
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

        // ==================== Search Nodes ====================
        LogicalNodeEnum::FulltextSearch(n) => {
            let mut node = crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextSearchNode::new(
                n.index_name, n.query, n.yield_clause, n.where_clause, n.order_clause, n.limit, n.offset,
            ).with_metadata(n.space_id, n.tag_name, n.field_name);
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::FulltextSearch(node)
        }

        LogicalNodeEnum::FulltextLookup(n) => {
            let mut node = crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextLookupNode::new(
                n.schema_name, n.index_name, n.query, n.yield_clause, n.limit,
            ).with_metadata(n.space_id, n.tag_name, n.field_name);
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::FulltextLookup(node)
        }

        LogicalNodeEnum::MatchFulltext(n) => {
            let mut node = crate::planning::plan::core::nodes::search::fulltext::data_access::MatchFulltextNode::new(
                n.pattern, n.fulltext_condition, n.yield_clause,
            ).with_metadata(n.space_id, n.tag_name, n.field_name);
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::MatchFulltext(node)
        }

        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorSearch(n) => {
            let mut node = crate::planning::plan::core::nodes::search::vector::data_access::VectorSearchNode::new(
                crate::planning::plan::core::nodes::search::vector::data_access::VectorSearchParams::new(
                    n.index_name.clone(),
                    n.space_id,
                    n.tag_name.clone(),
                    n.field_name.clone(),
                    n.query.clone(),
                )
                .with_threshold(n.threshold.unwrap_or(0.0))
                .with_filter(n.filter.clone())
                .with_limit(n.limit)
                .with_offset(n.offset)
                .with_output_fields(n.output_fields.clone())
                .with_metadata_version(n.metadata_version),
            );
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::VectorSearch(node)
        }

        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorLookup(n) => {
            let mut node = crate::planning::plan::core::nodes::search::vector::data_access::VectorLookupNode::new(
                n.schema_name,
                n.index_name,
                n.query,
                n.yield_fields,
                n.limit,
            )
            .with_metadata(n.space_id, n.tag_name, n.field_name);
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::VectorLookup(node)
        }

        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorMatch(n) => {
            let mut node = crate::planning::plan::core::nodes::search::vector::data_access::VectorMatchNode::new(
                n.pattern,
                n.field,
                n.query,
                n.threshold,
                n.yield_fields,
            ).with_metadata(n.space_id, n.tag_name, n.field_name);
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            PlanNodeEnum::VectorMatch(node)
        }

        LogicalNodeEnum::Flatten(n) => {
            let input = convert_logical_to_physical(*n.input.expect("Flatten missing input"));
            let mut node =
                crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode::new(
                    input,
                    n.group_pos,
                )
                .expect("Failed to construct FlattenNode");
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            node.set_col_names(n.col_names);
            node.set_column_types(n.column_types);
            PlanNodeEnum::Flatten(node)
        }

        LogicalNodeEnum::WcoIntersect(n) => lower_wco_intersect(n),

        LogicalNodeEnum::InsertVertices(n) => {
            let mut node =
                crate::planning::plan::core::nodes::data_modification::InsertVerticesNode::new(
                    n.id, n.info,
                );
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            if !n.col_names.is_empty() {
                node.set_col_names(n.col_names);
            }
            if !n.column_types.is_empty() {
                node.set_column_types(n.column_types);
            }
            PlanNodeEnum::InsertVertices(node)
        }

        LogicalNodeEnum::InsertEdges(n) => {
            let mut node =
                crate::planning::plan::core::nodes::data_modification::InsertEdgesNode::new(
                    n.id, n.info,
                );
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            if !n.col_names.is_empty() {
                node.set_col_names(n.col_names);
            }
            if !n.column_types.is_empty() {
                node.set_column_types(n.column_types);
            }
            PlanNodeEnum::InsertEdges(node)
        }

        LogicalNodeEnum::Update(n) => {
            let mut node = crate::planning::plan::core::nodes::data_modification::UpdateNode::new(
                n.id, n.info,
            );
            if let Some(var) = n.output_var {
                node.set_output_var(var);
            }
            if !n.col_names.is_empty() {
                node.set_col_names(n.col_names);
            }
            if !n.column_types.is_empty() {
                node.set_column_types(n.column_types);
            }
            PlanNodeEnum::Update(node)
        }
    }
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
fn lower_wco_intersect(
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

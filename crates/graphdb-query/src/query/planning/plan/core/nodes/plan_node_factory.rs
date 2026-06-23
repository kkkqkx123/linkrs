//! Implementation of the Node Factory
//!
//! Provide a unified interface for creating nodes.

use crate::query::planning::plan::core::nodes::access::graph_scan_node::{
    GetEdgesNode, GetNeighborsNode, GetVerticesNode, ScanEdgesNode, ScanVerticesNode,
};
use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::{
    ArgumentNode, LoopNode, PassThroughNode, SelectNode,
};
use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::{
    DataCollectNode, DedupNode, PatternApplyNode, RollUpApplyNode, UnionNode, UnwindNode,
};

use crate::core::types::operators::AggregateFunction;
use crate::core::types::ContextualExpression;
use crate::core::types::EdgeDirection;
use crate::core::YieldColumn;
use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::{
    LimitNode, SortItem, SortNode,
};
use crate::query::planning::plan::core::nodes::traversal::traversal_node::{
    AppendVerticesNode, ExpandAllNode, ExpandNode, TraverseNode,
};
use crate::query::planning::plan::PlanNodeEnum;

/// Node Factory
///
/// Provide a unified interface for node creation to simplify the process of creating nodes.
pub struct PlanNodeFactory;

impl PlanNodeFactory {
    /// Create a filter node.
    pub fn create_filter(
        input: PlanNodeEnum,
        condition: ContextualExpression,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
        let filter_node = FilterNode::new(input, condition)?;
        Ok(PlanNodeEnum::Filter(filter_node))
    }

    /// Create a projection node.
    pub fn create_project(
        input: PlanNodeEnum,
        columns: Vec<YieldColumn>,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
        let project_node = ProjectNode::new(input, columns)?;
        Ok(PlanNodeEnum::Project(project_node))
    }

    /// Create an inner join node.
    pub fn create_inner_join(
        left: PlanNodeEnum,
        right: PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        use crate::query::planning::plan::core::nodes::join::join_node::InnerJoinNode;

        let inner_join_node = InnerJoinNode::new(left, right, hash_keys, probe_keys)?;
        Ok(PlanNodeEnum::InnerJoin(inner_join_node))
    }

    /// Create the starting node.
    pub fn create_start_node() -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError>
    {
        Ok(PlanNodeEnum::Start(StartNode::new()))
    }

    /// Create a placeholder node (using ArgumentNode as the placeholder).
    pub fn create_placeholder_node(
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::Argument(ArgumentNode::new(-1, "placeholder")))
    }

    /// Create an aggregate node.
    pub fn create_aggregate(
        input: PlanNodeEnum,
        group_keys: Vec<String>,
        aggregation_functions: Vec<AggregateFunction>,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let aggregate_node = AggregateNode::new(input, group_keys, aggregation_functions)?;
        Ok(PlanNodeEnum::Aggregate(aggregate_node))
    }

    /// Create a sorting node.
    pub fn create_sort(
        input: PlanNodeEnum,
        sort_items: Vec<SortItem>,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let sort_node = SortNode::new(input, sort_items)?;
        Ok(PlanNodeEnum::Sort(sort_node))
    }

    /// Create a restricted node.
    pub fn create_limit(
        input: PlanNodeEnum,
        offset: i64,
        count: i64,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let limit_node = LimitNode::new(input, offset, count)?;
        Ok(PlanNodeEnum::Limit(limit_node))
    }

    /// Create a method to retrieve the vertex nodes
    pub fn create_get_vertices(
        space_id: u64,
        space_name: &str,
        src_vids: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::GetVertices(GetVerticesNode::new(
            space_id, space_name, src_vids,
        )))
    }

    /// Create a function to retrieve edge nodes
    pub fn create_get_edges(
        space_id: u64,
        src: &str,
        edge_type: &str,
        rank: &str,
        dst: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::GetEdges(GetEdgesNode::new(
            space_id, src, edge_type, rank, dst,
        )))
    }

    /// Create a mechanism to obtain information about neighboring nodes.
    pub fn create_get_neighbors(
        space_id: u64,
        src_vids: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::GetNeighbors(GetNeighborsNode::new(
            space_id, src_vids,
        )))
    }

    /// Create a node for scanning vertices.
    pub fn create_scan_vertices(
        space_id: u64,
        space_name: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::ScanVertices(ScanVerticesNode::new(
            space_id, space_name,
        )))
    }

    /// Create a node for scanning edges.
    pub fn create_scan_edges(
        space_id: u64,
        edge_type: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::ScanEdges(ScanEdgesNode::new(
            space_id, edge_type,
        )))
    }

    /// Create an extended node.
    pub fn create_expand(
        space_id: u64,
        edge_types: Vec<String>,
        direction: EdgeDirection,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::Expand(ExpandNode::new(
            space_id, edge_types, direction,
        )))
    }

    /// Create an extension to include all nodes.
    pub fn create_expand_all(
        space_id: u64,
        edge_types: Vec<String>,
        direction: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::ExpandAll(ExpandAllNode::new(
            space_id, edge_types, direction,
        )))
    }

    /// Create a function to traverse the nodes.
    pub fn create_traverse(
        space_id: u64,
        start_vids: &str,
        min_steps: u32,
        max_steps: u32,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::Traverse(TraverseNode::new(
            space_id, start_vids, min_steps, max_steps,
        )))
    }

    /// Create additional vertex nodes.
    pub fn create_append_vertices(
        space_id: u64,
        vertex_tag: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::AppendVertices(AppendVerticesNode::new(
            space_id, vertex_tag,
        )))
    }

    /// Create a parameter node.
    pub fn create_argument(
        id: i64,
        var: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::Argument(ArgumentNode::new(id, var)))
    }

    /// Create a selection node.
    pub fn create_select(
        id: i64,
        condition: ContextualExpression,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::Select(SelectNode::new(id, condition)))
    }

    /// Create a loop node
    pub fn create_loop(
        id: i64,
        condition: ContextualExpression,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::Loop(LoopNode::new(id, condition)))
    }

    /// Create a passthrough node
    pub fn create_pass_through(
        id: i64,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Ok(PlanNodeEnum::PassThrough(PassThroughNode::new(id)))
    }

    /// Create a joint node.
    pub fn create_union(
        input: PlanNodeEnum,
        union_input: PlanNodeEnum,
        distinct: bool,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let union_node = UnionNode::new(input, union_input, distinct)?;
        Ok(PlanNodeEnum::Union(union_node))
    }

    /// Create a difference set node.
    pub fn create_minus(
        input: PlanNodeEnum,
        minus_input: PlanNodeEnum,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        use crate::query::planning::plan::core::nodes::graph_operations::set_operations_node::MinusNode;
        let minus_node = MinusNode::new(input, minus_input)?;
        Ok(PlanNodeEnum::Minus(minus_node))
    }

    /// Create an intersection node.
    pub fn create_intersect(
        input: PlanNodeEnum,
        intersect_input: PlanNodeEnum,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        use crate::query::planning::plan::core::nodes::graph_operations::set_operations_node::IntersectNode;
        let intersect_node = IntersectNode::new(input, intersect_input)?;
        Ok(PlanNodeEnum::Intersect(intersect_node))
    }

    /// Create an expanded node.
    pub fn create_unwind(
        input: PlanNodeEnum,
        alias: &str,
        list_expression: crate::core::types::expr::contextual::ContextualExpression,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let unwind_node = UnwindNode::new(input, alias, list_expression)?;
        Ok(PlanNodeEnum::Unwind(unwind_node))
    }

    /// Create deduplication nodes.
    pub fn create_dedup(
        input: PlanNodeEnum,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let dedup_node = DedupNode::new(input)?;
        Ok(PlanNodeEnum::Dedup(dedup_node))
    }

    /// Create a RollUp application node.
    pub fn create_roll_up_apply(
        left_input: PlanNodeEnum,
        right_input: PlanNodeEnum,
        compare_cols: Vec<String>,
        collect_col: Option<String>,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let roll_up_apply_node =
            RollUpApplyNode::new(left_input, right_input, compare_cols, collect_col)?;
        Ok(PlanNodeEnum::RollUpApply(roll_up_apply_node))
    }

    /// Create a pattern application node.
    pub fn create_pattern_apply(
        left_input: PlanNodeEnum,
        right_input: PlanNodeEnum,
        key_cols: Vec<crate::core::types::ContextualExpression>,
        is_anti_predicate: bool,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let pattern_apply_node =
            PatternApplyNode::new(left_input, right_input, key_cols, is_anti_predicate)?;
        Ok(PlanNodeEnum::PatternApply(pattern_apply_node))
    }

    /// Create a data collection node.
    pub fn create_data_collect(
        input: PlanNodeEnum,
        collect_kind: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let data_collect_node = DataCollectNode::new(input, collect_kind)?;
        Ok(PlanNodeEnum::DataCollect(data_collect_node))
    }

    /// Create an index scanning node.
    pub fn create_index_scan(
        space_id: u64,
        tag_id: i32,
        index_id: i32,
        index_name: &str,
        scan_type: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        use crate::query::planning::plan::core::nodes::access::{IndexScanNode, ScanType};

        let index_scan_node = IndexScanNode::new(
            space_id,
            tag_id,
            index_id,
            index_name.to_string(),
            String::new(),
            ScanType::from_str_with_default(scan_type),
        );
        Ok(PlanNodeEnum::IndexScan(index_scan_node))
    }

    /// Create a border index scanning node.
    pub fn create_edge_index_scan(
        space_id: u64,
        edge_type: &str,
        index_name: &str,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        use crate::query::planning::plan::core::nodes::access::graph_scan_node::EdgeIndexScanNode;
        let edge_index_scan_node = EdgeIndexScanNode::new(space_id, edge_type, index_name);
        Ok(PlanNodeEnum::EdgeIndexScan(edge_index_scan_node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_start_node() {
        let start_node = PlanNodeFactory::create_start_node()
            .expect("Start node should be created successfully");

        assert_eq!(start_node.type_name(), "Start");
        assert_eq!(start_node.dependencies().len(), 0);
        assert_eq!(start_node.col_names().len(), 0);
    }

    #[test]
    fn test_create_placeholder_node() {
        let placeholder_node = PlanNodeFactory::create_placeholder_node()
            .expect("Placeholder node should be created successfully");

        assert_eq!(placeholder_node.type_name(), "Argument");
        assert_eq!(placeholder_node.dependencies().len(), 0);
        assert_eq!(placeholder_node.col_names().len(), 0);
    }

    #[test]
    fn test_create_get_vertices_node() {
        let get_vertices_node = PlanNodeFactory::create_get_vertices(1, "test_space", "1,2,3")
            .expect("GetVertices node should be created successfully");

        assert_eq!(get_vertices_node.type_name(), "GetVertices");
        assert_eq!(get_vertices_node.dependencies().len(), 0);
    }
}

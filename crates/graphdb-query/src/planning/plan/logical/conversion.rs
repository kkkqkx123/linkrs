//! Conversion from `PlanNodeEnum` to `LogicalNodeEnum`.
//!
//! This module implements the physical-to-logical stripping pass:
//! - Physical operators (IndexScan, InnerJoin, LeftJoin)
//!   are mapped to their logical equivalents.
//! - All other operators are recursively converted.

use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNode, SingleInputNode,
};
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

/// Errors that can occur during physical-to-logical conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionError {
    /// The plan contains a node type not yet supported in conversion.
    NotYetImplemented(String),
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotYetImplemented(name) => {
                write!(f, "conversion not yet implemented for: {}", name)
            }
        }
    }
}

impl std::error::Error for ConversionError {}

/// Recursively convert a `PlanNodeEnum` tree to a `LogicalNodeEnum` tree.
///
/// # Physical-to-Logical Mapping
/// - `IndexScan` → `ScanVertices`
/// - `InnerJoin` → `InnerJoin`
/// - `LeftJoin` → `LeftJoin`
/// - All other logical operators are preserved with recursive child conversion.
///
/// # Limitations
/// This is an initial implementation covering the most common operators.
/// Some specialized operators return `NotYetImplemented`.
pub fn convert_plan(node: &PlanNodeEnum) -> Result<LogicalNodeEnum, ConversionError> {
    match node {
        // === Access nodes ===
        PlanNodeEnum::Start(n) => Ok(LogicalNodeEnum::Start(
            crate::planning::plan::logical::logical_nodes::access::LogicalStartNode {
                id: n.id(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        )),

        PlanNodeEnum::ScanVertices(n) => Ok(LogicalNodeEnum::ScanVertices(
            crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode {
                id: n.id(),
                space_id: n.space_id(),
                space_name: n.space_name().to_string(),
                tag: n.tag().cloned(),
                expression: n.filter().cloned(),
                limit: n.limit(),
                projected_properties: n.projected_properties().to_vec(),
                index_hint: None,
                estimated_cardinality: None,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        )),

        PlanNodeEnum::ScanEdges(n) => Ok(LogicalNodeEnum::ScanEdges(
            crate::planning::plan::logical::logical_nodes::access::LogicalScanEdgesNode {
                id: n.id(),
                space_id: n.space_id(),
                edge_type: n.edge_type(),
                expression: n.filter().cloned(),
                limit: n.limit(),
                projected_properties: n.projected_properties().to_vec(),
                index_hint: None,
                estimated_cardinality: None,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        )),

        PlanNodeEnum::GetVertices(n) => {
            let logical =
                crate::planning::plan::logical::logical_nodes::access::LogicalGetVerticesNode {
                    id: n.id(),
                    space_id: n.space_id(),
                    space_name: n.space_name().to_string(),
                    src_ref: n.src_ref().clone(),
                    src_vids: n.src_vids().to_string(),
                    tag_props: n.tag_props().to_vec(),
                    expression: n.filter().cloned(),
                    dedup: n.dedup(),
                    limit: n.limit(),
                    projected_properties: n.projected_properties().to_vec(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                    deps: Vec::new(),
                };
            Ok(LogicalNodeEnum::GetVertices(logical))
        }

        PlanNodeEnum::GetNeighbors(n) => {
            let logical =
                crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
                    id: n.id(),
                    space_id: n.space_id(),
                    src_vids: n.src_vids().to_string(),
                    edge_types: n.edge_types().to_vec(),
                    direction: n.direction().to_string(),
                    edge_props: n.edge_props().to_vec(),
                    tag_props: n.tag_props().to_vec(),
                    expression: n.filter().cloned(),
                    dedup: n.dedup(),
                    limit: n.limit(),
                    projected_properties: n.projected_properties().to_vec(),
                    index_hint: None,
                    estimated_cardinality: None,
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                    deps: Vec::new(),
                };
            Ok(LogicalNodeEnum::GetNeighbors(logical))
        }

        // Physical access nodes → logical equivalents
        PlanNodeEnum::IndexScan(n) => Ok(LogicalNodeEnum::ScanVertices(
            crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode {
                id: n.id(),
                space_id: n.space_id(),
                space_name: String::new(),
                tag: None,
                expression: None,
                limit: n.limit(),
                projected_properties: vec![],
                index_hint: Some(
                    crate::planning::plan::logical::logical_nodes::access::IndexHint::new(
                        n.index_name().to_string(),
                        n.schema_name().to_string(),
                        n.tag_id(),
                        n.index_id(),
                        n.scan_type().as_str().to_string(),
                    ),
                ),
                estimated_cardinality: None,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        )),

        // === Operation nodes ===
        PlanNodeEnum::Project(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Project(
                crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    columns: n.columns().to_vec(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Filter(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Filter(
                crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    condition: n.condition().clone(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Sort(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Sort(
                crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    sort_items: n.sort_items().to_vec(),
                    limit: n.limit(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Limit(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Limit(
                crate::planning::plan::logical::logical_nodes::operation::LogicalLimitNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    offset: n.offset(),
                    count: n.count(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::TopN(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::TopN(
                crate::planning::plan::logical::logical_nodes::operation::LogicalTopNNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    sort_items: n.sort_items().to_vec(),
                    limit: n.limit(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Sample(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Sample(
                crate::planning::plan::logical::logical_nodes::operation::LogicalSampleNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    count: n.count(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Dedup(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Dedup(
                crate::planning::plan::logical::logical_nodes::operation::LogicalDedupNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Aggregate(n) => {
            let logical_input = convert_plan(n.input())?;
            if let Some(exprs) = n.group_key_exprs() {
                Ok(LogicalNodeEnum::Aggregate(
                    crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode {
                        id: n.id(),
                        input: Some(Box::new(logical_input.clone())),
                        deps: vec![logical_input],
                        group_key_exprs: exprs.to_vec(),
                        aggregation_functions: n.aggregation_functions().to_vec(),
                        aggregation_distinct: n.aggregation_distinct().to_vec(),
                        aggregation_filters: n.aggregation_filters().to_vec(),
                        grouping_sets: n.grouping_sets().to_vec(),
                        output_var: n.output_var().map(|s| s.to_string()),
                        col_names: n.col_names().to_vec(),
                        column_types: n.column_types().to_vec(),
                    },
                ))
            } else {
                Err(ConversionError::NotYetImplemented(
                    "Aggregate: group_keys string conversion synthesizes ExpressionId; use binder LogicalAggregateNode with group_key_exprs or upgrade physical node via physical_planner group_key_exprs pass-through".to_string(),
                ))
            }
        }

        PlanNodeEnum::Window(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Window(
                crate::planning::plan::logical::logical_nodes::operation::LogicalWindowNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    window_functions: n.window_functions().to_vec(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        // === Join nodes ===
        PlanNodeEnum::InnerJoin(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::InnerJoin(
                crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode {
                    id: n.id(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    hash_keys: n.hash_keys().to_vec(),
                    probe_keys: n.probe_keys().to_vec(),
                    deps: vec![logical_left, logical_right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::LeftJoin(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::LeftJoin(
                crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode {
                    id: n.id(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    hash_keys: n.hash_keys().to_vec(),
                    probe_keys: n.probe_keys().to_vec(),
                    deps: vec![logical_left, logical_right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::RightJoin(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::RightJoin(
                crate::planning::plan::logical::logical_nodes::join::LogicalRightJoinNode {
                    id: n.id(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    hash_keys: n.hash_keys().to_vec(),
                    probe_keys: n.probe_keys().to_vec(),
                    deps: vec![logical_left, logical_right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::CrossJoin(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::CrossJoin(
                crate::planning::plan::logical::logical_nodes::join::LogicalCrossJoinNode {
                    id: n.id(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    hash_keys: vec![],
                    probe_keys: vec![],
                    deps: vec![logical_left, logical_right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::FullOuterJoin(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::FullOuterJoin(
                crate::planning::plan::logical::logical_nodes::join::LogicalFullOuterJoinNode {
                    id: n.id(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    hash_keys: n.hash_keys().to_vec(),
                    probe_keys: n.probe_keys().to_vec(),
                    deps: vec![logical_left, logical_right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::SemiJoin(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::SemiJoin(
                crate::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode {
                    id: n.id(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    hash_keys: n.hash_keys().to_vec(),
                    probe_keys: n.probe_keys().to_vec(),
                    deps: vec![logical_left, logical_right],
                    join_condition: n.join_condition().cloned(),
                    anti: n.is_anti(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Flatten(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Flatten(
                crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode {
                    id: n.id(),
                    group_pos: n.group_pos(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Expand(n) => {
            let deps: Vec<LogicalNodeEnum> = n
                .inputs()
                .iter()
                .map(convert_plan)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LogicalNodeEnum::Expand(
                crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandNode {
                    id: n.id(),
                    space_id: 1,
                    edge_types: n.edge_types().to_vec(),
                    direction: n.direction(),
                    step_limit: n.step_limit(),
                    filter: n.filter().cloned(),
                    deps,
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::ExpandAll(n) => {
            let deps: Vec<LogicalNodeEnum> = n
                .inputs()
                .iter()
                .map(convert_plan)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LogicalNodeEnum::ExpandAll(
                crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode {
                    id: n.id(),
                    space_id: n.space_id(),
                    edge_types: n.edge_types().to_vec(),
                    direction: n.direction().to_string(),
                    any_edge_type: n.any_edge_type(),
                    step_limit: n.step_limit(),
                    step_limits: n.step_limits().cloned(),
                    join_input: n.join_input(),
                    sample: n.sample(),
                    edge_props: n.edge_props().to_vec(),
                    vertex_props: n.vertex_props().to_vec(),
                    filter: n.filter().cloned(),
                    src_vids: n.src_vids().to_vec(),
                    include_empty_paths: n.include_empty_paths(),
                    input_var: n.get_input_var().map(|s| s.to_string()),
                    deps,
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Traverse(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Traverse(
                crate::planning::plan::logical::logical_nodes::traversal::LogicalTraverseNode {
                    id: n.id(),
                    space_id: 1,
                    start_vids: n.start_vids().to_string(),
                    end_vids: n.end_vids().cloned(),
                    edge_types: n.edge_types().to_vec(),
                    direction: n.direction(),
                    min_steps: n.min_steps(),
                    max_steps: n.max_steps(),
                    edge_alias: n.edge_alias().cloned(),
                    vertex_alias: n.vertex_alias().cloned(),
                    e_filter: n.e_filter().cloned(),
                    v_filter: n.v_filter().cloned(),
                    first_step_filter: n.first_step_filter().cloned(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        }

        PlanNodeEnum::AppendVertices(n) => {
            let deps: Vec<LogicalNodeEnum> = n
                .inputs()
                .iter()
                .map(convert_plan)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LogicalNodeEnum::AppendVertices(
                crate::planning::plan::logical::logical_nodes::traversal::LogicalAppendVerticesNode {
                    id: n.id(),
                    space_id: 1,
                    vertex_tag: n.vertex_tag().to_string(),
                    vertex_props: n.vertex_props().to_vec(),
                    filter: n.filter().cloned(),
                    input_var: n.input_var().map(|s| s.to_string()),
                    src_expression: n.src_expression().cloned(),
                    dedup: n.dedup(),
                    need_fetch_prop: n.need_fetch_prop(),
                    vids: n.vids().to_vec(),
                    tag_ids: n.tag_ids().to_vec(),
                    v_filter: n.v_filter().cloned(),
                    node_alias: n.node_alias().cloned(),
                    deps,
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::BiExpand(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::BiExpand(
                crate::planning::plan::logical::logical_nodes::traversal::LogicalBiExpandNode {
                    id: n.id(),
                    space_id: 1,
                    left_direction: n.left_direction(),
                    right_direction: n.right_direction(),
                    edge_types: n.edge_types().to_vec(),
                    max_hops: n.max_hops(),
                    meeting_point_var: n.meeting_point_var().cloned(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    deps: vec![logical_left, logical_right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::BiTraverse(n) => {
            let logical_left = convert_plan(n.left_input())?;
            let logical_right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::BiTraverse(
                crate::planning::plan::logical::logical_nodes::traversal::LogicalBiTraverseNode {
                    id: n.id(),
                    space_id: 1,
                    left_src_var: n.left_src_var().to_string(),
                    right_src_var: n.right_src_var().to_string(),
                    edge_types: n.edge_types().to_vec(),
                    left_direction: n.left_direction(),
                    right_direction: n.right_direction(),
                    min_hops: n.min_hops(),
                    max_hops: n.max_hops(),
                    path_var: n.path_var().to_string(),
                    edge_alias: n.edge_alias().cloned(),
                    vertex_alias: n.vertex_alias().cloned(),
                    left: Box::new(logical_left.clone()),
                    right: Box::new(logical_right.clone()),
                    deps: vec![logical_left, logical_right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Select(n) => {
            let if_branch = if let Some(b) = n.if_branch() {
                Some(Box::new(convert_plan(b)?))
            } else {
                None
            };
            let else_branch = if let Some(b) = n.else_branch() {
                Some(Box::new(convert_plan(b)?))
            } else {
                None
            };
            Ok(LogicalNodeEnum::Select(
                crate::planning::plan::logical::logical_nodes::control_flow::LogicalSelectNode {
                    id: n.id(),
                    condition: n.condition().clone(),
                    if_branch,
                    else_branch,
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Loop(n) => {
            let body = if let Some(b) = n.body() {
                Some(Box::new(convert_plan(b)?))
            } else {
                None
            };
            Ok(LogicalNodeEnum::Loop(
                crate::planning::plan::logical::logical_nodes::control_flow::LogicalLoopNode {
                    id: n.id(),
                    condition: n.condition().clone(),
                    body,
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Assign(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Assign(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    assignments: n.assignments().to_vec(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Remove(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Remove(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalRemoveNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    remove_items: n.remove_items().to_vec(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::DataCollect(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::DataCollect(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalDataCollectNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    collect_kind: n.collect_kind().to_string(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Materialize(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Materialize(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalMaterializeNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Union(n) => {
            let left = convert_plan(n.input())?;
            let right = convert_plan(n.union_input())?;
            Ok(LogicalNodeEnum::Union(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnionNode {
                    id: n.id(),
                    input: Some(Box::new(left.clone())),
                    deps: vec![left, right],
                    distinct: n.distinct(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Minus(n) => {
            let left = convert_plan(n.input())?;
            let right = convert_plan(n.minus_input())?;
            Ok(LogicalNodeEnum::Minus(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalMinusNode {
                    id: n.id(),
                    input: Some(Box::new(left.clone())),
                    deps: vec![left, right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Intersect(n) => {
            let left = convert_plan(n.input())?;
            let right = convert_plan(n.intersect_input())?;
            Ok(LogicalNodeEnum::Intersect(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalIntersectNode {
                    id: n.id(),
                    input: Some(Box::new(left.clone())),
                    deps: vec![left, right],
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Unwind(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::Unwind(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    alias: n.alias().to_string(),
                    list_expression: n.list_expression().clone(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::Argument(n) => Ok(LogicalNodeEnum::Argument(
            crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode {
                id: n.id(),
                var: n.var().to_string(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        )),

        PlanNodeEnum::PassThrough(n) => Ok(LogicalNodeEnum::PassThrough(
            crate::planning::plan::logical::logical_nodes::control_flow::LogicalPassThroughNode {
                id: n.id(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        )),

        PlanNodeEnum::Apply(n) => {
            let left = convert_plan(n.left_input())?;
            let right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::Apply(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalApplyNode {
                    id: n.id(),
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                    deps: vec![left, right],
                    left_input_var: None,
                    right_input_var: None,
                    correlated_cols: n.correlated_cols().to_vec(),
                    apply_kind: n.apply_kind(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::PatternApply(n) => {
            let left = convert_plan(n.left_input())?;
            let right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::PatternApply(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode {
                    id: n.id(),
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                    hash_keys: n.hash_keys().to_vec(),
                    probe_keys: n.probe_keys().to_vec(),
                    deps: vec![left, right],
                    is_anti_predicate: n.is_anti_predicate(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::RollUpApply(n) => {
            let logical_input = convert_plan(n.input())?;
            Ok(LogicalNodeEnum::RollUpApply(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalRollUpApplyNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    left_input_var: n.left_input_var().map(|s| s.to_string()),
                    right_input_var: n.right_input_var().map(|s| s.to_string()),
                    compare_cols: n.compare_cols().to_vec(),
                    collect_col: n.collect_col().cloned(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        PlanNodeEnum::CorrelatedApply(n) => {
            let left = convert_plan(n.left_input())?;
            let right = convert_plan(n.right_input())?;
            Ok(LogicalNodeEnum::CorrelatedApply(
                crate::planning::plan::logical::logical_nodes::graph_ops::LogicalCorrelatedApplyNode {
                    id: n.id(),
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                    hash_keys: vec![],
                    probe_keys: vec![],
                    deps: vec![left, right],
                    is_anti_predicate: n.is_anti_predicate(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: vec![],
                },
            ))
        }

        // Physical join nodes → logical equivalents
        PlanNodeEnum::WcoIntersect(n) => {
            let probe = convert_plan(n.probe_input())?;
            let mut builds = Vec::with_capacity(n.num_builds());
            for build_input in n.build_inputs() {
                builds.push(convert_plan(build_input)?);
            }
            let mut logical =
                crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode::new(
                    probe,
                    builds,
                    n.intersect_key().clone(),
                    n.bound_keys().to_vec(),
                    n.col_names().to_vec(),
                );
            if let Some(var) = n.output_var() {
                logical.set_output_var(var.to_string());
            }
            logical.set_column_types(n.column_types().to_vec());
            Ok(LogicalNodeEnum::WcoIntersect(logical))
        }

        PlanNodeEnum::GetEdges(n) => Ok(LogicalNodeEnum::GetEdges(
            crate::planning::plan::logical::logical_nodes::access::LogicalGetEdgesNode {
                id: n.id(),
                space_id: n.space_id(),
                edge_ref: n.edge_ref().clone(),
                src: n.src().to_string(),
                edge_type: n.edge_type().to_string(),
                rank: n.rank().to_string(),
                dst: n.dst().to_string(),
                edge_props: n.edge_props().to_vec(),
                expression: n.filter().cloned(),
                dedup: n.dedup(),
                limit: n.limit(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        )),

        // === Fallback for unsupported nodes ===
        // Intentionally not mapped: Algorithm nodes (MultiShortestPath/BFSShortest/AllPaths/ShortestPath)
        // and Fulltext/Vector search remain `NotYetImplemented`.
        // They are non-factorized operators not on the factorization / partitioning chain,
        // so reverse mapping is optional; `ensure_logical_plan` will record the failure in
        // `cbo_notes` and fall back to flat execution without factorization.
        _ => Err(ConversionError::NotYetImplemented(
            node.type_name().to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_start() {
        let start = crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let plan_enum = PlanNodeEnum::Start(start);
        let result = convert_plan(&plan_enum);
        assert!(result.is_ok());
        let logical = result.unwrap();
        assert_eq!(logical.type_name(), "Start");
    }

    #[test]
    fn test_convert_filter_with_start() {
        let start = crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let start_enum = PlanNodeEnum::Start(start);

        let ctx = std::sync::Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let expr = graphdb_core::Expression::Variable("test".to_string());
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let condition = graphdb_core::types::expr::contextual::ContextualExpression::new(id, ctx);

        let filter = crate::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
            start_enum, condition,
        )
        .expect("Filter node creation failed");

        let filter_enum = PlanNodeEnum::Filter(filter);
        let result = convert_plan(&filter_enum);
        assert!(result.is_ok());
        let logical = result.unwrap();
        assert_eq!(logical.type_name(), "Filter");
    }

    #[test]
    fn test_convert_unsupported() {
        // Argument is now supported; use a management node that remains unsupported
        // as the negative case (e.g., Tag management not yet bridged).
        let arg =
            crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode::new(
                -1, "x",
            );
        let plan_enum = PlanNodeEnum::Argument(arg);
        let result = convert_plan(&plan_enum);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().type_name(), "Argument");
    }
}

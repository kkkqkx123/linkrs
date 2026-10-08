//! Logical-plan mirror helpers for pattern planning.

use crate::planning::plan::core::next_node_id;
use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
use crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
use crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::planner::PlannerError;
use linkrs_core::types::expr::ContextualExpression;

/// Logical mirror helpers. The physical plan stays the execution artifact;
/// a parallel pure-logical tree is attached to each SubPlan so the compiler
/// can build the `LogicalPlan` natively (instead of stripping it back out of
/// the physical tree).
pub(super) fn logical_scan_vertices(
    space_id: u64,
    space_name: &str,
    tag: Option<&str>,
    var_name: &str,
) -> LogicalNodeEnum {
    LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
        id: next_node_id(),
        space_id,
        space_name: space_name.to_string(),
        tag: tag.map(|s| s.to_string()),
        expression: None,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: Some(var_name.to_string()),
        col_names: vec![var_name.to_string()],
        column_types: vec![],
    })
}

pub(super) fn logical_filter(
    input: LogicalNodeEnum,
    condition: ContextualExpression,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Filter(LogicalFilterNode {
        id: next_node_id(),
        input: Some(Box::new(input)),
        condition,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn logical_expand_all(
    space_id: u64,
    edge_types: Vec<String>,
    direction: &str,
    any_edge_type: bool,
    input_var: Option<String>,
    col_names: Vec<String>,
    path_semantic: Option<crate::parser::ast::pattern::PathSemantic>,
    dst_tag: Option<String>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::ExpandAll(LogicalExpandAllNode {
        id: next_node_id(),
        deps: vec![],
        space_id,
        edge_types,
        direction: direction.to_string(),
        any_edge_type,
        step_limit: Some(1),
        step_limits: None,
        join_input: false,
        sample: false,
        edge_props: vec![],
        vertex_props: vec![],
        filter: None,
        src_vids: vec![],
        include_empty_paths: false,
        input_var,
        path_semantic,
        dst_tag,
        output_var: None,
        col_names,
        column_types: vec![],
    })
}

/// Resolve the neighbor label carried by a node pattern.
///
/// A single label is passed through to the plan. An unlabelled node pattern
/// yields an empty label: the executor derives the neighbor label from the
/// edge type schema and skips hops it cannot resolve. Multiple labels cannot
/// be represented by one plan-level label and stay an error.
pub(super) fn neighbor_tag(labels: &[String]) -> Result<String, PlannerError> {
    match labels {
        [] => Ok(String::new()),
        [single] => Ok(single.clone()),
        _ => Err(PlannerError::PlanGenerationFailed(
            "Traversal neighbor supports at most one label".to_string(),
        )),
    }
}

pub(super) fn logical_argument(var_name: &str) -> LogicalNodeEnum {
    LogicalNodeEnum::Argument(LogicalArgumentNode {
        id: next_node_id(),
        var: var_name.to_string(),
        output_var: Some(var_name.to_string()),
        col_names: vec![var_name.to_string()],
        column_types: vec![],
    })
}

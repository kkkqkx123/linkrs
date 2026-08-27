//! Conversion from `PlanNodeEnum` to `LogicalNodeEnum`.
//!
//! This module implements the physical-to-logical stripping pass:
//! - Physical operators (IndexScan, InnerJoin, LeftJoin)
//!   are mapped to their logical equivalents.
//! - All other operators are recursively converted.

use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
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
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        )),

        PlanNodeEnum::GetVertices(n) => {
            let logical = crate::planning::plan::logical::logical_nodes::access::LogicalGetVerticesNode {
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
            let logical = crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
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
            Ok(LogicalNodeEnum::Aggregate(
                crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input.clone())),
                    deps: vec![logical_input],
                    group_keys: n.group_keys().to_vec(),
                    aggregation_functions: n.aggregation_functions().to_vec(),
                    aggregation_distinct: n.aggregation_distinct().to_vec(),
                    aggregation_filters: n.aggregation_filters().to_vec(),
                    grouping_sets: n.grouping_sets().to_vec(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
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

        // Physical join nodes → logical equivalents

        // === Fallback for unsupported nodes ===
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
        let start =
            crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let plan_enum = PlanNodeEnum::Start(start);
        let result = convert_plan(&plan_enum);
        assert!(result.is_ok());
        let logical = result.unwrap();
        assert_eq!(logical.type_name(), "Start");
    }

    #[test]
    fn test_convert_filter_with_start() {
        let start =
            crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let start_enum = PlanNodeEnum::Start(start);

        let ctx = std::sync::Arc::new(
            crate::core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let expr = crate::core::Expression::Variable("test".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let condition = crate::core::types::expr::contextual::ContextualExpression::new(id, ctx);

        let filter =
            crate::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
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
        let arg = crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode::new(-1, "x");
        let plan_enum = PlanNodeEnum::Argument(arg);
        let result = convert_plan(&plan_enum);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::NotYetImplemented(name) => assert_eq!(name, "Argument"),
        }
    }
}

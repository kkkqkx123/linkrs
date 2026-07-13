//! Build physical source plans for scan nodes.

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::SourceSpec;
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::executor::streaming::physical_properties::PhysicalProperties;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Build a physical source plan for a scan node.
pub fn build_scan_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::ScanVertices(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|value| (value >= 0).then_some(value as usize));
            let col_names = scan_node.col_names().to_vec();
            Ok(PhysicalNode::Source(
                node.id(),
                SourceSpec::StorageScanVertices {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    limit,
                    col_names,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::ScanEdges(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|value| (value >= 0).then_some(value as usize));
            let col_names = scan_node.col_names().to_vec();
            let edge_type = scan_node.edge_type().map(|s| s.to_string());
            Ok(PhysicalNode::Source(
                node.id(),
                SourceSpec::StorageScanEdges {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    limit,
                    edge_type,
                    col_names,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::GetVertices(get_node) => {
            let vertex_ids = get_node.src_ref().constant_value().map(|v| vec![v]);
            Ok(PhysicalNode::Source(
                node.id(),
                SourceSpec::GetVertices {
                    space_name: get_node.space_name().to_string(),
                    vertex_ids,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

        PlanNodeEnum::GetEdges(get_node) => Ok(PhysicalNode::Source(
            node.id(),
            SourceSpec::GetEdges {
                space_name: context.space_name.clone().unwrap_or_default(),
                edge_type: Some(get_node.edge_type().to_string()),
                src: Some(get_node.src().to_string()),
                dst: Some(get_node.dst().to_string()),
                rank: 0,
            },
            PhysicalProperties::single_streaming(),
        )),

        PlanNodeEnum::GetNeighbors(get_node) => Ok(PhysicalNode::Source(
            node.id(),
            SourceSpec::GetNeighbors {
                space_name: context.space_name.clone().unwrap_or_default(),
                direction: get_node.direction().to_string(),
            },
            PhysicalProperties::single_streaming(),
        )),

        PlanNodeEnum::EdgeIndexScan(scan_node) => Ok(PhysicalNode::Source(
            node.id(),
            SourceSpec::EdgeIndexScan {
                space_name: context.space_name.clone().unwrap_or_default(),
                edge_type: Some(scan_node.edge_type().to_string()),
            },
            PhysicalProperties::single_streaming(),
        )),

        PlanNodeEnum::IndexScan(scan_node) => Ok(PhysicalNode::Source(
            node.id(),
            SourceSpec::IndexScan {
                space_name: context.space_name.clone().unwrap_or_default(),
                index_name: Some(scan_node.index_name().to_string()),
                index_value: None,
            },
            PhysicalProperties::single_streaming(),
        )),

        PlanNodeEnum::Start(_) => Ok(PhysicalNode::Source(
            node.id(),
            SourceSpec::Start,
            PhysicalProperties::single_streaming(),
        )),

        PlanNodeEnum::Argument(_) => Ok(PhysicalNode::Source(
            node.id(),
            SourceSpec::Argument,
            PhysicalProperties::single_streaming(),
        )),

        _ => Err(super::internal_routing_error(node, "scans")),
    }
}

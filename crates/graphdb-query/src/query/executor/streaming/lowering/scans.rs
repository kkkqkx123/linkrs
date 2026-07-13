//! Lower scan source plan nodes into PhysicalNode trees.

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::operator_spec::SourceSpec;
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Lower a scan source plan node into a PhysicalNode tree.
#[allow(unused_variables)]
pub fn lower_scan_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::ScanVertices(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|value| (value >= 0).then_some(value as usize));
            let col_names = scan_node.col_names().to_vec();
            Ok(PhysicalNode::Source(SourceSpec::StorageScanVertices {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                limit,
                col_names,
            }))
        }

        PlanNodeEnum::ScanEdges(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|value| (value >= 0).then_some(value as usize));
            let col_names = scan_node.col_names().to_vec();
            let edge_type = scan_node.edge_type().map(|s| s.to_string());
            Ok(PhysicalNode::Source(SourceSpec::StorageScanEdges {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                limit,
                edge_type,
                col_names,
            }))
        }

        PlanNodeEnum::GetVertices(get_node) => {
            let vertex_ids = get_node.src_ref().constant_value().map(|v| vec![v]);
            Ok(PhysicalNode::Source(SourceSpec::GetVertices {
                storage: context.storage.clone(),
                space_name: get_node.space_name().to_string(),
                vertex_ids,
            }))
        }

        PlanNodeEnum::GetEdges(get_node) => {
            Ok(PhysicalNode::Source(SourceSpec::GetEdges {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                edge_type: Some(get_node.edge_type().to_string()),
                src: Some(get_node.src().to_string()),
                dst: Some(get_node.dst().to_string()),
                rank: 0,
            }))
        }

        PlanNodeEnum::GetNeighbors(get_node) => {
            Ok(PhysicalNode::Source(SourceSpec::GetNeighbors {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                direction: get_node.direction().to_string(),
            }))
        }

        PlanNodeEnum::EdgeIndexScan(scan_node) => {
            Ok(PhysicalNode::Source(SourceSpec::EdgeIndexScan {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                edge_type: Some(scan_node.edge_type().to_string()),
            }))
        }

        PlanNodeEnum::IndexScan(scan_node) => {
            Ok(PhysicalNode::Source(SourceSpec::IndexScan {
                storage: context.storage.clone(),
                space_name: context.space_name.clone().unwrap_or_default(),
                index_name: Some(scan_node.index_name().to_string()),
                index_value: None,
            }))
        }

        PlanNodeEnum::Start(_) => {
            Ok(PhysicalNode::Source(SourceSpec::Start))
        }

        PlanNodeEnum::Argument(_) => {
            Ok(PhysicalNode::Source(SourceSpec::Argument))
        }

        _ => Err(QueryError::execution(format!(
            "lowering::scans does not handle node type: {}",
            node.name()
        ))),
    }
}

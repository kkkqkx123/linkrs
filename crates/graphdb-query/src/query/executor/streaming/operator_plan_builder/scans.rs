//! Build physical source plans for scan nodes.

use std::sync::Arc;

use std::collections::HashSet;

use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::{
    BoundIndexPredicate, IndexProjection, SourceSpec,
};
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::executor::streaming::plan::properties::PhysicalProperties;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Build a physical source plan for a scan node.
pub fn build_scan_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    match node {
        PlanNodeEnum::ScanVertices(scan_node) => {
            let limit = scan_node
                .limit()
                .and_then(|value| (value >= 0).then_some(value as usize));
            let col_names = scan_node.col_names().to_vec();
            let projected_properties = extract_projected_properties(&col_names);
            Ok(PhysicalNode::Source(
                node.id(),
                SourceSpec::StorageScanVertices {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    limit,
                    col_names,
                    projected_properties,
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
            let vertex_ids = get_node.src_ref().constant_value().map(|v| vec![v]).or_else(|| {
                let src_vids = get_node.src_vids();
                if src_vids.is_empty() {
                    None
                } else {
                    Some(
                        src_vids
                            .split(',')
                            .map(|s| crate::core::Value::String(s.trim().to_string()))
                            .collect::<Vec<_>>(),
                    )
                }
            });
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

        PlanNodeEnum::IndexScan(scan_node) => {
            let index_name = scan_node.index_name().to_string();
            if scan_node.filter().is_some() && scan_node.scan_limits().is_empty() {
                return Err(PlanBuildError::capability(
                    "index_residual_filter",
                    format!(
                        "IndexScan '{}' has a residual filter that is not pushed into the native index cursor",
                        index_name
                    ),
                ));
            }
            // Build a BoundIndexPredicate from the first scan limit.
            // When scan_limits is empty, fall back to a full index scan.
            let predicate = scan_node
                .scan_limits()
                .first()
                .map(|limit| {
                    let column = limit.column.clone();
                    match limit.scan_type {
                        crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Unique => {
                            BoundIndexPredicate::Equal {
                                column,
                                value: limit.begin_value.clone().unwrap_or(crate::core::Value::Null(crate::core::value::NullType::Null)),
                            }
                        }
                        crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Range => {
                            BoundIndexPredicate::Range {
                                column,
                                begin: limit.begin_value.clone(),
                                end: limit.end_value.clone(),
                                include_begin: limit.include_begin,
                                include_end: limit.include_end,
                            }
                        }
                        crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Prefix => {
                            BoundIndexPredicate::Prefix {
                                column,
                                prefix: limit.begin_value.clone().unwrap_or(crate::core::Value::Null(crate::core::value::NullType::Null)),
                            }
                        }
                        crate::query::planning::plan::core::nodes::access::index_scan::ScanType::Full => {
                            BoundIndexPredicate::Full
                        }
                    }
                })
                .unwrap_or(BoundIndexPredicate::Full);
            let projection = if scan_node.return_columns().is_empty() {
                IndexProjection::RowIdOnly
            } else {
                IndexProjection::Columns(scan_node.return_columns().to_vec())
            };
            let col_names = scan_node.col_names().to_vec();
            let output_layout = Arc::new(SlotLayout::from_names(&col_names));
            Ok(PhysicalNode::Source(
                node.id(),
                SourceSpec::IndexScan {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    index_name,
                    index_id: scan_node.index_id(),
                    predicate,
                    projection,
                    residual_filter: None,
                    output_layout,
                },
                PhysicalProperties::single_streaming(),
            ))
        }

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

/// Extract property names from `col_names` for projection pushdown.
///
/// When the projection-pushdown optimizer rewrites `col_names` from a
/// single vertex-variable name (`["p"]`) to a list of property aliases
/// (`["p.name", "p.age"]`), this function extracts just the property names
/// (`["name", "age"]`) so the scan can load only those columns.
///
/// Heuristic: if any name contains a dot OR there are multiple names, we
/// treat them as projected property references.  A single undotted name is
/// assumed to be a vertex variable (no projection).
fn extract_projected_properties(col_names: &[String]) -> Vec<String> {
    if col_names.len() == 1 && !col_names[0].contains('.') {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    col_names
        .iter()
        .filter_map(|name| {
            let prop = name.rsplit_once('.').map(|(_, p)| p).unwrap_or(name);
            if seen.insert(prop.to_string()) {
                Some(prop.to_string())
            } else {
                None
            }
        })
        .collect()
}

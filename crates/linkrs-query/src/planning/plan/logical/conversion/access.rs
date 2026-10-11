//! Physical-to-logical conversion: access node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::ConversionError;

use crate::planning::plan::core::nodes::access::graph_scan_node::{
    GetNeighborsNode, GetVerticesNode, ScanEdgesNode, ScanVerticesNode,
};
use crate::planning::plan::core::nodes::access::index_scan::IndexScanNode;
use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;

pub(super) fn convert_start(n: &StartNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::Start(
        crate::planning::plan::logical::logical_nodes::access::LogicalStartNode {
            id: n.id(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_scan_vertices(
    n: &ScanVerticesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::ScanVertices(
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
    ))
}

pub(super) fn convert_scan_edges(n: &ScanEdgesNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::ScanEdges(
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
    ))
}

pub(super) fn convert_get_vertices(
    n: &GetVerticesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical =
            crate::planning::plan::logical::logical_nodes::access::LogicalGetVerticesNode {
                id: n.id(),
                space_id: n.space_id(),
                space_name: n.space_name().to_string(),
                tag: n.tag().map(|s| s.to_string()),
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
}

pub(super) fn convert_get_neighbors(
    n: &GetNeighborsNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical =
            crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
                id: n.id(),
                space_id: n.space_id(),
                tag: n.tag().map(|s| s.to_string()),
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
}

pub(super) fn convert_index_scan(n: &IndexScanNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::ScanVertices(
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
    ))
}

//! Physical-to-logical conversion: access node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::ConversionError;

use crate::planning::plan::core::nodes::access::graph_scan_node::{
    GetNeighborsNode, GetVerticesNode, ScanEdgesNode, ScanVerticesNode,
};
use crate::planning::plan::core::nodes::access::index_scan::IndexScanNode;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
use crate::planning::plan::core::nodes::search::fulltext::data_access::{
    FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
};

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

pub(super) fn convert_fulltext_search(
    n: &FulltextSearchNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::FulltextSearch(
        crate::planning::plan::logical::logical_nodes::search::LogicalFulltextSearchNode {
            id: n.id(),
            index_name: n.index_name.clone(),
            query: n.query.clone(),
            yield_clause: n.yield_clause.clone(),
            where_clause: n.where_clause.clone(),
            order_clause: n.order_clause.clone(),
            limit: n.limit,
            offset: n.offset,
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    ))
}

pub(super) fn convert_fulltext_lookup(
    n: &FulltextLookupNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::FulltextLookup(
        crate::planning::plan::logical::logical_nodes::search::LogicalFulltextLookupNode {
            id: n.id(),
            schema_name: n.schema_name.clone(),
            index_name: n.index_name.clone(),
            query: n.query.clone(),
            yield_clause: n.yield_clause.clone(),
            limit: n.limit,
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    ))
}

pub(super) fn convert_match_fulltext(
    n: &MatchFulltextNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::MatchFulltext(
        crate::planning::plan::logical::logical_nodes::search::LogicalMatchFulltextNode {
            id: n.id(),
            pattern: n.pattern.clone(),
            fulltext_condition: n.fulltext_condition.clone(),
            yield_clause: n.yield_clause.clone(),
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    ))
}

#[cfg(feature = "vector")]
pub(super) fn convert_vector_search(
    n: &crate::planning::plan::core::nodes::search::vector::data_access::VectorSearchNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::VectorSearch(
        crate::planning::plan::logical::logical_nodes::search::LogicalVectorSearchNode {
            id: n.id(),
            index_name: n.index_name.clone(),
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            query: n.query.clone(),
            threshold: n.threshold,
            filter: n.filter.clone(),
            limit: n.limit,
            offset: n.offset,
            output_fields: n.output_fields.clone(),
            metadata_version: n.metadata_version,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    ))
}

#[cfg(feature = "vector")]
pub(super) fn convert_vector_lookup(
    n: &crate::planning::plan::core::nodes::search::vector::data_access::VectorLookupNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::VectorLookup(
        crate::planning::plan::logical::logical_nodes::search::LogicalVectorLookupNode {
            id: n.id(),
            schema_name: n.schema_name.clone(),
            index_name: n.index_name.clone(),
            query: n.query.clone(),
            yield_fields: n.yield_fields.clone(),
            limit: n.limit,
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    ))
}

#[cfg(feature = "vector")]
pub(super) fn convert_vector_match(
    n: &crate::planning::plan::core::nodes::search::vector::data_access::VectorMatchNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::VectorMatch(
        crate::planning::plan::logical::logical_nodes::search::LogicalVectorMatchNode {
            id: n.id(),
            pattern: n.pattern.clone(),
            field: n.field.clone(),
            query: n.query.clone(),
            threshold: n.threshold,
            yield_fields: n.yield_fields.clone(),
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    ))
}

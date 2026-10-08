use super::*;

pub(super) fn convert_fulltext_search(
    n: crate::planning::plan::logical::logical_nodes::search::LogicalFulltextSearchNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextSearchNode::new(
            n.index_name,
            n.query,
            n.yield_clause,
            n.where_clause,
            n.order_clause,
            n.limit,
            n.offset,
        )
        .with_metadata(n.space_id, n.tag_name, n.field_name);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::FulltextSearch(node)
}

pub(super) fn convert_fulltext_lookup(
    n: crate::planning::plan::logical::logical_nodes::search::LogicalFulltextLookupNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextLookupNode::new(
            n.schema_name,
            n.index_name,
            n.query,
            n.yield_clause,
            n.limit,
        )
        .with_metadata(n.space_id, n.tag_name, n.field_name);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::FulltextLookup(node)
}

pub(super) fn convert_match_fulltext(
    n: crate::planning::plan::logical::logical_nodes::search::LogicalMatchFulltextNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::search::fulltext::data_access::MatchFulltextNode::new(
            n.pattern,
            n.fulltext_condition,
            n.yield_clause,
        )
        .with_metadata(n.space_id, n.tag_name, n.field_name);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::MatchFulltext(node)
}

#[cfg(feature = "vector")]
pub(super) fn convert_simvec(
    n: crate::planning::plan::logical::logical_nodes::search::LogicalVectorSearchNode,
) -> PlanNodeEnum {
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
pub(super) fn convert_vector_lookup(
    n: crate::planning::plan::logical::logical_nodes::search::LogicalVectorLookupNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::search::vector::data_access::VectorLookupNode::new(
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
pub(super) fn convert_vector_match(
    n: crate::planning::plan::logical::logical_nodes::search::LogicalVectorMatchNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::search::vector::data_access::VectorMatchNode::new(
            n.pattern,
            n.field,
            n.query,
            n.threshold,
            n.yield_fields,
        )
        .with_metadata(n.space_id, n.tag_name, n.field_name);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::VectorMatch(node)
}

use crate::planning::plan::factorization::FactorizedSchema;

use super::bi_expand_schema;
use super::register_output_names;

use crate::planning::plan::logical::logical_nodes::traversal::{
    LogicalAppendVerticesNode, LogicalBiExpandNode, LogicalBiTraverseNode, LogicalExpandAllNode,
    LogicalExpandNode, LogicalTraverseNode,
};

pub(super) fn traverse(
    n: &LogicalTraverseNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.has_unflat_group() {
        if let Some(pos) = schema.unflat_group_pos() {
            schema.flatten_group(pos);
        }
    }
    let out = schema.create_group();
    if let Some(alias) = &n.vertex_alias {
        schema.insert_name_for_group(alias.clone(), out);
    }
    if let Some(alias) = &n.edge_alias {
        schema.insert_name_for_group(alias.clone(), out);
    }
    register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn expand(
    n: &LogicalExpandNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.has_unflat_group() {
        if let Some(pos) = schema.unflat_group_pos() {
            schema.flatten_group(pos);
        }
    }
    let out = schema.create_group();
    register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn expand_all(
    n: &LogicalExpandAllNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.has_unflat_group() {
        if let Some(pos) = schema.unflat_group_pos() {
            schema.flatten_group(pos);
        }
    }
    let out = schema.create_group();
    register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn append_vertices(
    n: &LogicalAppendVerticesNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.has_unflat_group() {
        if let Some(pos) = schema.unflat_group_pos() {
            schema.flatten_group(pos);
        }
    }
    let out = schema.create_group();
    if let Some(alias) = &n.node_alias {
        schema.insert_name_for_group(alias.clone(), out);
    }
    register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn bi_expand(
    n: &LogicalBiExpandNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut aliases = Vec::new();
    if let Some(var) = &n.meeting_point_var {
        aliases.push(var.clone());
    }
    if let Some(var) = n.output_var.as_deref() {
        aliases.push(var.to_string());
    }
    aliases.extend(n.col_names.iter().cloned());
    bi_expand_schema(child_schemas, &aliases)
}

pub(super) fn bi_traverse(
    n: &LogicalBiTraverseNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut aliases = vec![n.path_var.clone()];
    if let Some(alias) = &n.edge_alias {
        aliases.push(alias.clone());
    }
    if let Some(alias) = &n.vertex_alias {
        aliases.push(alias.clone());
    }
    if let Some(var) = n.output_var.as_deref() {
        aliases.push(var.to_string());
    }
    aliases.extend(n.col_names.iter().cloned());
    bi_expand_schema(child_schemas, &aliases)
}

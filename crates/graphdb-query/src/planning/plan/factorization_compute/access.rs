use crate::planning::plan::factorization::FactorizedSchema;

use super::{register_output_names, resolve_id};

use crate::planning::plan::logical::logical_nodes::access::{
    LogicalGetEdgesNode, LogicalGetNeighborsNode, LogicalGetVerticesNode, LogicalScanEdgesNode,
    LogicalScanVerticesNode,
};

pub(super) fn scan_vertices(n: &LogicalScanVerticesNode) -> FactorizedSchema {
    let mut schema = FactorizedSchema::new();
    let g = schema.create_flat_group(false);
    if let Some(expr) = &n.expression {
        let eid = resolve_id(expr);
        let name = n
            .col_names()
            .first()
            .cloned()
            .unwrap_or_else(|| "scan".to_string());
        schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn scan_edges(n: &LogicalScanEdgesNode) -> FactorizedSchema {
    let mut schema = FactorizedSchema::new();
    let g = schema.create_flat_group(false);
    if let Some(expr) = &n.expression {
        let eid = resolve_id(expr);
        let name = n
            .col_names()
            .first()
            .cloned()
            .unwrap_or_else(|| "scan".to_string());
        schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn get_vertices(
    n: &LogicalGetVerticesNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = if let Some(cs) = child_schemas.first() {
        cs.clone()
    } else {
        FactorizedSchema::new()
    };
    if schema.num_groups() == 0 {
        let g = schema.create_flat_group(false);
        {
            let eid = resolve_id(&n.src_ref);
            let name = n
                .col_names()
                .first()
                .cloned()
                .unwrap_or_else(|| "getv".to_string());
            schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
        }
        if let Some(expr) = &n.expression {
            let eid = resolve_id(expr);
            if !schema.is_expression_in_scope(&eid) {
                let name = expr.to_expression_string();
                schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
            }
        }
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn get_edges(n: &LogicalGetEdgesNode) -> FactorizedSchema {
    let mut schema = FactorizedSchema::new();
    let g = schema.create_flat_group(false);
    let eid = resolve_id(&n.edge_ref);
    schema.insert_to_group_and_scope_with_name(eid, Some(n.edge_type.clone()), g);
    if let Some(expr) = &n.expression {
        let eid2 = resolve_id(expr);
        if !schema.is_expression_in_scope(&eid2) {
            let name = expr.to_expression_string();
            schema.insert_to_group_and_scope_with_name(eid2, Some(name), g);
        }
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn get_neighbors(
    n: &LogicalGetNeighborsNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = if let Some(cs) = child_schemas.first() {
        cs.clone()
    } else {
        FactorizedSchema::new()
    };
    if schema.num_groups() == 0 {
        schema.create_flat_group(false);
    }
    if schema.has_unflat_group() {
        if let Some(pos) = schema.unflat_group_pos() {
            schema.flatten_group(pos);
        }
    }
    let output_group = schema.create_group();
    if let Some(expr) = &n.expression {
        let eid = resolve_id(expr);
        if !schema.is_expression_in_scope(&eid) {
            let name = n
                .col_names()
                .first()
                .cloned()
                .unwrap_or_else(|| "neighbors".to_string());
            schema.insert_to_group_and_scope_with_name(eid, Some(name), output_group);
        }
    } else {
        register_output_names(
            &mut schema,
            n.output_var.as_deref(),
            &n.col_names,
            output_group,
        );
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn start() -> FactorizedSchema {
    let mut schema = FactorizedSchema::new();
    schema.create_flat_group(false);
    schema
}

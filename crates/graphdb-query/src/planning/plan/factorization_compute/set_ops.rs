use crate::planning::plan::factorization::FactorizedSchema;

use crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode;

pub(super) fn union_minus(child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    if child_schemas.len() >= 2 {
        let left = &child_schemas[0];
        let right = &child_schemas[1];
        let mut merged = left.clone();
        let mapping = merged.merge_groups_from(right);
        for (expr_id, gpos) in right.expression_to_group_iter() {
            let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
            merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
        }
        merged.flatten_all();
        merged.validate_at_most_one_unflat();
        merged
    } else {
        child_schemas.first().cloned().unwrap_or_default()
    }
}

pub(super) fn intersect(child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    if child_schemas.len() > 2 {
        let mut schema = child_schemas[0].clone();
        if schema.has_unflat_group() {
            schema.flatten_all();
        }
        let out_pos = schema.create_group();
        for build_schema in &child_schemas[1..] {
            for expr in build_schema.expressions_in_scope() {
                if !schema.is_expression_in_scope(expr) {
                    schema.insert_to_group_and_scope(expr.clone(), out_pos);
                }
            }
        }
        schema.validate_at_most_one_unflat();
        schema
    } else if child_schemas.len() >= 2 {
        let left = &child_schemas[0];
        let right = &child_schemas[1];
        let mut merged = left.clone();
        let mapping = merged.merge_groups_from(right);
        for (expr_id, gpos) in right.expression_to_group_iter() {
            let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
            merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
        }
        merged.flatten_all();
        merged.validate_at_most_one_unflat();
        merged
    } else {
        child_schemas.first().cloned().unwrap_or_default()
    }
}

pub(super) fn wco_intersect(
    n: &LogicalWcoIntersectNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.has_unflat_group() {
        schema.flatten_all();
    }
    let out_pos = schema.create_group();
    let intersect_id = n.intersect_key().id().clone();
    if schema.is_expression_in_scope(&intersect_id) {
        if let Some(pos) = schema.get_group_pos(&intersect_id) {
            schema.flatten_group(pos);
        }
    } else if let Some(name) = n.intersect_key().as_variable() {
        schema.insert_to_group_and_scope_with_name(intersect_id.clone(), Some(name), out_pos);
    } else {
        schema.insert_to_group_and_scope(intersect_id.clone(), out_pos);
    }
    for (build_idx, build_schema) in child_schemas.iter().skip(1).enumerate() {
        let bound_id = n.bound_keys().get(build_idx).map(|k| k.id().clone());
        for expr in build_schema.expressions_in_scope() {
            if *expr == intersect_id {
                continue;
            }
            if let Some(bound) = &bound_id {
                if expr == bound {
                    continue;
                }
            }
            if !schema.is_expression_in_scope(expr) {
                schema.insert_to_group_and_scope(expr.clone(), out_pos);
            }
        }
        for (name, _) in build_schema.expression_name_to_group_iter() {
            if let Some(var) = n.intersect_key().as_variable() {
                if name.as_str() == var.as_str() {
                    continue;
                }
            }
            if schema.get_group_pos_by_name_opt(name).is_none() {
                schema.insert_name_for_group(name.clone(), out_pos);
            }
        }
    }
    schema.validate_at_most_one_unflat();
    schema
}

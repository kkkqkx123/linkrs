use crate::planning::plan::factorization::FactorizedSchema;

pub(super) fn flat_leaf() -> FactorizedSchema {
    let mut schema = FactorizedSchema::new();
    schema.create_flat_group(false);
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn flatten_all_from_child(child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    schema.flatten_all();
    schema.validate_at_most_one_unflat();
    schema
}

/// Barrier for binary operators without a per-operator flatten rule
/// (Apply family, shortest-path family): merge both children, then flatten.
/// Unlike `flatten_all_from_child`, the right child scope is preserved.
pub(super) fn barrier_binary(child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    if child_schemas.len() >= 2 {
        let mut merged = child_schemas[0].clone();
        let mapping = merged.merge_groups_from(&child_schemas[1]);
        for (expr_id, gpos) in child_schemas[1].expression_to_group_iter() {
            let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
            merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
        }
        for (name, gpos) in child_schemas[1].expression_name_to_group_iter() {
            let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
            if merged.get_group_pos_by_name_opt(name).is_none() {
                merged.insert_name_for_group(name.clone(), new_pos);
            }
        }
        merged.flatten_all();
        merged.validate_at_most_one_unflat();
        merged
    } else {
        flatten_all_from_child(child_schemas)
    }
}

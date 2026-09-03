use crate::planning::plan::factorization::{FGroupPos, FactorizedSchema};

pub(super) fn binary_join(child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    if child_schemas.len() >= 2 {
        let left = &child_schemas[0];
        let right = &child_schemas[1];
        let mut merged = left.clone();
        let mapping = merged.merge_groups_from(right);
        for (expr_id, gpos) in right.expression_to_group_iter() {
            let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
            merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
        }
        if merged.has_unflat_group() {
            let unflat_count = merged.groups().iter().filter(|g| !g.is_flat()).count();
            if unflat_count > 1 {
                let mut first = true;
                for i in 0..merged.num_groups() {
                    let pos = i as FGroupPos;
                    if let Some(g) = merged.get_group(pos) {
                        if !g.is_flat() {
                            if first {
                                first = false;
                            } else {
                                merged.flatten_group(pos);
                            }
                        }
                    }
                }
            }
        }
        merged.validate_at_most_one_unflat();
        merged
    } else {
        child_schemas.first().cloned().unwrap_or_default()
    }
}

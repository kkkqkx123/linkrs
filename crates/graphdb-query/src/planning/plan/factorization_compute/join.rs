use std::collections::HashSet;

use graphdb_core::types::expr::contextual::ContextualExpression;

use crate::optimizer::factorization::flatten_resolver::{FlattenAll, FlattenAllButOne};
use crate::planning::plan::factorization::{FGroupPos, FactorizedSchema};

/// Key-aware join schema shared with the rewriter.
///
/// Mirrors the rewriter `visit_hash_join_*` policy: for `Inner`/`Left`
/// (`probe_left`) the left key groups flatten fully and the right key groups
/// keep one alive, and vice versa for `Right`. This is conservative relative
/// to the baseline `requireFlatProbeKeys` conditional (multi-key, `LEFT`,
/// non-ID keys, non-unique build side), which needs type and uniqueness
/// information the schema does not carry yet; the over-flattening is
/// documented here instead of claimed as optimal.
fn key_groups(keys: &[ContextualExpression], schema: &FactorizedSchema) -> HashSet<FGroupPos> {
    let mut set = HashSet::new();
    for k in keys {
        if let Some(pos) = schema.get_group_pos(k.id()) {
            set.insert(pos);
        }
    }
    set
}

fn flatten_keys(schema: &mut FactorizedSchema, keys: &[ContextualExpression], keep_one: bool) {
    let groups = key_groups(keys, schema);
    let to_flatten = if keep_one {
        FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, schema)
    } else {
        FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, schema)
    };
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
}

fn merge_flattened(
    left: &FactorizedSchema,
    right: &FactorizedSchema,
    left_key_groups: &HashSet<FGroupPos>,
    right_key_groups: &HashSet<FGroupPos>,
) -> FactorizedSchema {
    let mut merged = left.clone();
    let mapping = merged.merge_groups_from(right);
    for (expr_id, gpos) in right.expression_to_group_iter() {
        let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
        merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
    }
    // Key groups must already be flat here: every binary policy above
    // flattens its join-key groups before the merge, so any surviving
    // unflat group is a non-key group by construction. The debug assertion
    // pins that contract; release keeps the conservative fallback below.
    for pos in left_key_groups {
        debug_assert!(
            merged.get_group(*pos).is_none_or(|g| g.is_flat()),
            "left join-key group {} must be flat before merge",
            pos
        );
    }
    for (old_pos, new_pos) in &mapping {
        if right_key_groups.contains(old_pos) {
            debug_assert!(
                merged.get_group(*new_pos).is_none_or(|g| g.is_flat()),
                "right join-key group {} must be flat before merge",
                old_pos
            );
        }
    }
    // Non-key unflat groups can still collide after the merge; keep the
    // first unflat group in position order to hold the global invariant.
    // This is deliberately conservative: without key/type/uniqueness
    // context the merge cannot prove which side preserves row identity,
    // so it keeps one side factorized instead of flattening everything.
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
}

/// Inner/Left/Semi policy: left keys flatten fully, right keeps one.
pub(super) fn binary_join_inner(
    left: &FactorizedSchema,
    right: &FactorizedSchema,
    hash_keys: &[ContextualExpression],
    probe_keys: &[ContextualExpression],
) -> FactorizedSchema {
    let left_keys = key_groups(hash_keys, left);
    let right_keys = key_groups(probe_keys, right);
    let mut left = left.clone();
    let mut right = right.clone();
    flatten_keys(&mut left, hash_keys, false);
    flatten_keys(&mut right, probe_keys, true);
    merge_flattened(&left, &right, &left_keys, &right_keys)
}

/// Right policy: left keeps one, right flattens fully.
pub(super) fn binary_join_right(
    left: &FactorizedSchema,
    right: &FactorizedSchema,
    hash_keys: &[ContextualExpression],
    probe_keys: &[ContextualExpression],
) -> FactorizedSchema {
    let left_keys = key_groups(hash_keys, left);
    let right_keys = key_groups(probe_keys, right);
    let mut left = left.clone();
    let mut right = right.clone();
    flatten_keys(&mut left, hash_keys, true);
    flatten_keys(&mut right, probe_keys, false);
    merge_flattened(&left, &right, &left_keys, &right_keys)
}

/// Full-outer policy: both key sides flatten fully.
///
/// Either side can emit unmatched rows, so neither side keeps an unflat
/// key group. Non-key unflat groups still collapse through the shared
/// positional fallback, keeping the at-most-one-unflat invariant.
pub(super) fn binary_join_full_outer(
    left: &FactorizedSchema,
    right: &FactorizedSchema,
    hash_keys: &[ContextualExpression],
    probe_keys: &[ContextualExpression],
) -> FactorizedSchema {
    let left_keys = key_groups(hash_keys, left);
    let right_keys = key_groups(probe_keys, right);
    let mut left = left.clone();
    let mut right = right.clone();
    flatten_keys(&mut left, hash_keys, false);
    flatten_keys(&mut right, probe_keys, false);
    merge_flattened(&left, &right, &left_keys, &right_keys)
}

/// Cross without keys: no key flattening, only the positional fallback.
pub(super) fn cross_join_no_keys(
    left: &FactorizedSchema,
    right: &FactorizedSchema,
) -> FactorizedSchema {
    let empty = HashSet::new();
    merge_flattened(left, right, &empty, &empty)
}

use std::collections::{HashMap, HashSet};

use super::{FGroupPos, FactorizationError, FactorizedSchema};

/// Utilities for factorization invariants.
pub struct SchemaUtils;

impl SchemaUtils {
    pub fn get_leading_group_pos(
        group_positions: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> Result<FGroupPos, FactorizationError> {
        if group_positions.is_empty() {
            return Err(FactorizationError::EmptyGroupPositions);
        }
        Self::validate_at_most_one_unflat(group_positions, schema)?;
        for &pos in group_positions {
            if let Some(g) = schema.get_group(pos) {
                if !g.is_flat() {
                    return Ok(pos);
                }
            }
        }
        group_positions
            .iter()
            .next()
            .copied()
            .ok_or(FactorizationError::NonEmptyGroupPositions)
    }

    pub fn validate_at_most_one_unflat(
        group_positions: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        let mut unflat = 0;
        for &pos in group_positions {
            if let Some(g) = schema.get_group(pos) {
                if !g.is_flat() {
                    unflat += 1;
                }
            }
        }
        if unflat > 1 {
            return Err(FactorizationError::TooManyUnflatGroups(unflat));
        }
        Ok(())
    }

    pub fn validate_no_unflat(
        group_positions: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        for &pos in group_positions {
            if let Some(g) = schema.get_group(pos) {
                if !g.is_flat() {
                    return Err(FactorizationError::GroupExpectedFlat(pos));
                }
            }
        }
        Ok(())
    }
}

/// Rebuilds factorized schemas across sink boundaries.
///
/// A sink operator collects its whole input before producing output, so the
/// output groups no longer share the input's nesting structure. Flat
/// payloads are gathered into one new group (marked single-state when unflat
/// payloads also exist); each contributing unflat input group is copied into
/// its own new group with the cardinality multiplier preserved.
///
/// Mirrors `lbug::planner::SinkOperatorUtil` in
/// `ref/ladybug/src/planner/operator/factorization/sink_util.cpp`.
pub struct SinkOperatorUtil;

impl SinkOperatorUtil {
    pub fn merge_schema(
        input_schema: &FactorizedSchema,
        expressions_to_merge: &[graphdb_core::types::expr::ExpressionId],
        result_schema: &mut FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        let mut flat_payloads = Vec::new();
        let mut unflat_per_group: HashMap<FGroupPos, Vec<graphdb_core::types::expr::ExpressionId>> =
            HashMap::new();
        for expr_id in expressions_to_merge {
            let Some(pos) = input_schema.get_group_pos(expr_id) else {
                continue;
            };
            let Some(group) = input_schema.get_group(pos) else {
                continue;
            };
            if group.is_flat() {
                flat_payloads.push(expr_id.clone());
            } else {
                unflat_per_group
                    .entry(pos)
                    .or_default()
                    .push(expr_id.clone());
            }
        }
        let mut unflat_groups: Vec<FGroupPos> = unflat_per_group.keys().copied().collect();
        unflat_groups.sort_unstable();
        let mut old_to_new: HashMap<FGroupPos, FGroupPos> = HashMap::new();
        let mut flat_new_pos: Option<FGroupPos> = None;
        if unflat_groups.is_empty() {
            if !flat_payloads.is_empty() {
                let new_pos = result_schema.create_group();
                for expr_id in &flat_payloads {
                    result_schema.insert_to_group_and_scope(expr_id.clone(), new_pos)?;
                }
                flat_new_pos = Some(new_pos);
            }
        } else {
            if !flat_payloads.is_empty() {
                let new_pos = result_schema.create_group();
                for expr_id in &flat_payloads {
                    result_schema.insert_to_group_and_scope(expr_id.clone(), new_pos)?;
                }
                result_schema.set_group_as_single_state(new_pos)?;
                flat_new_pos = Some(new_pos);
            }
            for old_pos in unflat_groups {
                let new_pos = result_schema.create_group();
                for expr_id in &unflat_per_group[&old_pos] {
                    result_schema.insert_to_group_and_scope(expr_id.clone(), new_pos)?;
                }
                if let Some(input_group) = input_schema.get_group(old_pos) {
                    let multiplier = input_group.cardinality_multiplier();
                    if let Some(new_group) = result_schema.get_group_mut(new_pos) {
                        new_group.set_multiplier(multiplier);
                    }
                }
                old_to_new.insert(old_pos, new_pos);
            }
        }
        if let Some(flat_new_pos) = flat_new_pos {
            let mut flat_old: Vec<FGroupPos> = flat_payloads
                .iter()
                .filter_map(|eid| input_schema.get_group_pos(eid))
                .collect();
            flat_old.sort_unstable();
            flat_old.dedup();
            for old_pos in flat_old {
                old_to_new.entry(old_pos).or_insert(flat_new_pos);
            }
        }
        Self::remap_names(
            input_schema,
            expressions_to_merge,
            &old_to_new,
            result_schema,
        )?;
        result_schema.validate_at_most_one_unflat()?;
        Ok(())
    }

    pub fn recompute_schema(
        input_schema: &FactorizedSchema,
        expressions_to_merge: &[graphdb_core::types::expr::ExpressionId],
        result_schema: &mut FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        result_schema.clear();
        Self::merge_schema(input_schema, expressions_to_merge, result_schema)
    }

    fn remap_names(
        input_schema: &FactorizedSchema,
        expressions_to_merge: &[graphdb_core::types::expr::ExpressionId],
        old_to_new: &HashMap<FGroupPos, FGroupPos>,
        result_schema: &mut FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        let mut owned_names: HashSet<&str> = HashSet::new();
        for expr_id in expressions_to_merge {
            if let Some(name) = input_schema.expression_name(expr_id) {
                owned_names.insert(name);
            }
        }
        let linked_names: HashSet<&str> = input_schema
            .expression_id_to_name
            .values()
            .map(String::as_str)
            .collect();
        let mut names: Vec<(&String, &FGroupPos)> =
            input_schema.expression_name_to_group_iter().collect();
        names.sort_by(|a, b| a.0.cmp(b.0));
        for (name, old_pos) in names {
            let Some(new_pos) = old_to_new.get(old_pos).copied() else {
                continue;
            };
            if !owned_names.contains(name.as_str()) && linked_names.contains(name.as_str()) {
                continue;
            }
            if result_schema.get_group_pos_by_name_opt(name).is_none() {
                result_schema.insert_name_for_group(name.clone(), new_pos)?;
            }
        }
        Ok(())
    }
}

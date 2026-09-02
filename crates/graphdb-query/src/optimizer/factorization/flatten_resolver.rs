use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::{
    FGroupPos, FactorizedSchema, SchemaUtils, INVALID_F_GROUP_POS,
};

use super::group_dependency_analyzer::GroupDependencyAnalyzer;

/// FlattenAllButOne: flatten all dependent unflat groups except the leading one.
///
/// Mirrors `lbug::planner::FlattenAllButOne` in
/// `ref/ladybug/src/planner/operator/factorization/flatten_resolver.cpp`.
pub struct FlattenAllButOne;

impl FlattenAllButOne {
    /// For a set of expressions: collect dependent groups, keep first unflat as leading, flatten rest.
    pub fn get_groups_pos_to_flatten_for_exprs(
        exprs: &[ExpressionId],
        schema: &FactorizedSchema,
        expr_store: &HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> HashSet<FGroupPos> {
        let mut result = HashSet::new();
        let mut dependent_groups = HashSet::new();

        for expr_id in exprs {
            let mut analyzer =
                GroupDependencyAnalyzer::with_expr_store(schema, false, expr_store.clone());
            analyzer.visit(expr_id);
            result.extend(analyzer.required_flat_groups().iter().copied());
            dependent_groups.extend(analyzer.dependent_groups().iter().copied());
        }

        let mut candidates: Vec<FGroupPos> = dependent_groups
            .into_iter()
            .filter(|pos| {
                schema
                    .get_group(*pos)
                    .map(|g| !g.is_flat() && !result.contains(pos))
                    .unwrap_or(false)
            })
            .collect();
        // Deterministic order
        candidates.sort_unstable();
        for pos in candidates.iter().skip(1) {
            result.insert(*pos);
        }
        result
    }

    pub fn get_groups_pos_to_flatten_for_exprs_with_leading(
        exprs: &[ExpressionId],
        schema: &FactorizedSchema,
        expr_store: &HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> (FGroupPos, HashSet<FGroupPos>) {
        let mut result = HashSet::new();
        let mut dependent_groups = HashSet::new();

        for expr_id in exprs {
            let mut analyzer =
                GroupDependencyAnalyzer::with_expr_store(schema, false, expr_store.clone());
            analyzer.visit(expr_id);
            result.extend(analyzer.required_flat_groups().iter().copied());
            dependent_groups.extend(analyzer.dependent_groups().iter().copied());
        }

        let mut candidates: Vec<FGroupPos> = dependent_groups
            .iter()
            .filter(|pos| {
                schema
                    .get_group(**pos)
                    .map(|g| !g.is_flat() && !result.contains(*pos))
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        candidates.sort_unstable();

        for pos in candidates.iter().skip(1) {
            result.insert(*pos);
        }

        if candidates.is_empty() {
            (INVALID_F_GROUP_POS, result)
        } else {
            (candidates[0], result)
        }
    }

    /// Single expression version.
    pub fn get_groups_pos_to_flatten_for_expr(
        expr_id: &ExpressionId,
        schema: &FactorizedSchema,
        expr_store: &HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> HashSet<FGroupPos> {
        Self::get_groups_pos_to_flatten_for_exprs(std::slice::from_ref(expr_id), schema, expr_store)
    }

    /// Group set version: flatten all but one.
    pub fn get_groups_pos_to_flatten_for_groups(
        dependent_groups: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> HashSet<FGroupPos> {
        let mut candidates: Vec<FGroupPos> = dependent_groups
            .iter()
            .filter(|pos| {
                schema
                    .get_group(**pos)
                    .map(|g| !g.is_flat())
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        candidates.sort_unstable();
        let mut result = HashSet::new();
        for pos in candidates.iter().skip(1) {
            result.insert(*pos);
        }
        result
    }

    /// Group set version with leading return.
    pub fn get_groups_pos_to_flatten_for_groups_with_leading(
        dependent_groups: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> (FGroupPos, HashSet<FGroupPos>) {
        let mut candidates: Vec<FGroupPos> = dependent_groups
            .iter()
            .filter(|pos| {
                schema
                    .get_group(**pos)
                    .map(|g| !g.is_flat())
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        candidates.sort_unstable();
        let mut result = HashSet::new();
        for pos in candidates.iter().skip(1) {
            result.insert(*pos);
        }
        if candidates.is_empty() {
            (INVALID_F_GROUP_POS, result)
        } else {
            (candidates[0], result)
        }
    }
}

/// FlattenAll: flatten every dependent unflat group.
pub struct FlattenAll;

impl FlattenAll {
    pub fn get_groups_pos_to_flatten_for_exprs(
        exprs: &[ExpressionId],
        schema: &FactorizedSchema,
        expr_store: &HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> HashSet<FGroupPos> {
        let mut result = HashSet::new();
        for expr_id in exprs {
            result.extend(Self::get_groups_pos_to_flatten_for_expr(
                expr_id, schema, expr_store,
            ));
        }
        result
    }

    pub fn get_groups_pos_to_flatten_for_expr(
        expr_id: &ExpressionId,
        schema: &FactorizedSchema,
        expr_store: &HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> HashSet<FGroupPos> {
        let mut analyzer =
            GroupDependencyAnalyzer::with_expr_store(schema, false, expr_store.clone());
        analyzer.visit(expr_id);
        Self::get_groups_pos_to_flatten_for_groups(analyzer.dependent_groups(), schema)
    }

    pub fn get_groups_pos_to_flatten_for_groups(
        dependent_groups: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> HashSet<FGroupPos> {
        dependent_groups
            .iter()
            .filter(|pos| {
                schema
                    .get_group(**pos)
                    .map(|g| !g.is_flat())
                    .unwrap_or(false)
            })
            .copied()
            .collect()
    }
}

/// Legacy wrapper providing the Ladybug-style static API that the plan doc expects.
pub struct FlattenResolver;

impl FlattenResolver {
    pub fn flatten_all_but_one(
        dependent_groups: &[FGroupPos],
        schema: &FactorizedSchema,
    ) -> (FGroupPos, Vec<FGroupPos>) {
        let set: HashSet<FGroupPos> = dependent_groups.iter().copied().collect();
        let (leading, to_flatten) =
            FlattenAllButOne::get_groups_pos_to_flatten_for_groups_with_leading(&set, schema);
        let mut vec: Vec<FGroupPos> = to_flatten.into_iter().collect();
        vec.sort_unstable();
        (leading, vec)
    }

    pub fn flatten_all(
        dependent_groups: &[FGroupPos],
        schema: &FactorizedSchema,
    ) -> Vec<FGroupPos> {
        let set: HashSet<FGroupPos> = dependent_groups.iter().copied().collect();
        let mut vec: Vec<FGroupPos> =
            FlattenAll::get_groups_pos_to_flatten_for_groups(&set, schema)
                .into_iter()
                .collect();
        vec.sort_unstable();
        vec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::factorization::FactorizedSchema;
    use graphdb_core::types::expr::ExpressionId;
    use std::collections::{HashMap, HashSet};

    fn expr(id: u64) -> ExpressionId {
        ExpressionId::new(id)
    }

    #[test]
    fn flatten_all_single_unflat() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0);
        schema.insert_to_group_and_scope(expr(2), g1);

        let mut set = HashSet::new();
        set.insert(g1);
        let res = FlattenAll::get_groups_pos_to_flatten_for_groups(&set, &schema);
        assert_eq!(res, HashSet::from([g1]));

        let res2 = FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&set, &schema);
        assert!(
            res2.is_empty(),
            "single unflat should not be flattened in AllButOne"
        );
    }

    #[test]
    fn flatten_all_but_one_two_unflats() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_group();
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0);
        schema.insert_to_group_and_scope(expr(2), g1);
        let mut set = HashSet::new();
        set.insert(g0);
        set.insert(g1);
        let res = FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&set, &schema);
        assert_eq!(res.len(), 1);
        let (_leading, to_flatten) =
            FlattenAllButOne::get_groups_pos_to_flatten_for_groups_with_leading(&set, &schema);
        assert_eq!(to_flatten.len(), 1);
    }

    #[test]
    fn flatten_resolver_api() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_group();
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(10), g0);
        schema.insert_to_group_and_scope(expr(20), g1);
        let (leading, to_flatten) = FlattenResolver::flatten_all_but_one(&[g0, g1], &schema);
        assert_ne!(leading, INVALID_F_GROUP_POS);
        assert_eq!(to_flatten.len(), 1);
        let all = FlattenResolver::flatten_all(&[g0, g1], &schema);
        assert_eq!(all.len(), 2);
    }
}

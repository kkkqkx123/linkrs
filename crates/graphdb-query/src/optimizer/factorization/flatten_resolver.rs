use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::{FGroupPos, FactorizedSchema, INVALID_F_GROUP_POS};

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
        // Baseline `FlattenAll(expr)` flattens only dependent groups and
        // ignores `required_flat` (see ladybug `flatten_resolver.cpp:92-97`).
        // Callers that need lambda-body flatness use `FlattenAllButOne`,
        // which merges `required_flat` explicitly.
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

/// Shared two-stage aggregate flatten rule mirroring
/// `LogicalAggregate::getGroupsPosToFlatten` (`logical_aggregate.cpp:32-57`).
///
/// Stage 1 runs `FlattenAllButOne` over the group keys and returns the
/// surviving `leading` group. Stage 2 walks every aggregate payload: lambda
/// bodies (`required_flat`) always flatten, while `distinct` payloads
/// additionally flatten any dependent group other than `leading`. Plain
/// payloads on the leading group stay factorized.
pub fn aggregate_groups_to_flatten(
    key_ids: &[ExpressionId],
    key_store: &HashMap<ExpressionId, graphdb_core::Expression>,
    aggregate_args: &[Vec<graphdb_core::Expression>],
    aggregate_distinct: &[bool],
    child_schema: &FactorizedSchema,
) -> (FGroupPos, HashSet<FGroupPos>) {
    let (leading, mut to_flatten) =
        FlattenAllButOne::get_groups_pos_to_flatten_for_exprs_with_leading(
            key_ids,
            child_schema,
            key_store,
        );
    if leading == INVALID_F_GROUP_POS {
        return (leading, to_flatten);
    }
    for (idx, args) in aggregate_args.iter().enumerate() {
        let is_distinct = aggregate_distinct.get(idx).copied().unwrap_or(false);
        for payload in args {
            let mut analyzer =
                GroupDependencyAnalyzer::with_expr_store(child_schema, false, key_store.clone());
            analyzer.visit_expression(payload);
            for pos in analyzer.required_flat_groups().iter().copied() {
                if child_schema.get_group(pos).is_some_and(|g| !g.is_flat()) {
                    to_flatten.insert(pos);
                }
            }
            if is_distinct {
                for pos in analyzer.dependent_groups().iter().copied() {
                    if pos != leading && child_schema.get_group(pos).is_some_and(|g| !g.is_flat()) {
                        to_flatten.insert(pos);
                    }
                }
            }
        }
    }
    (leading, to_flatten)
}

/// Legacy wrapper providing the Ladybug-style static API.
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
    use std::collections::HashSet;

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

    #[test]
    fn flatten_all_expr_matches_dependent_only() {
        use crate::optimizer::factorization::GroupDependencyAnalyzer;
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope_with_name(expr(10), Some("a".to_string()), g0);
        schema.insert_to_group_and_scope_with_name(expr(20), Some("x".to_string()), g1);
        // Simulate list_extract(lambda) where lambda body depends on x (unflat).
        // Baseline `FlattenAll(expr)` flattens only dependent groups and
        // ignores `required_flat`; `AllButOne` merges `required_flat`.
        let the_expr = graphdb_core::Expression::Function {
            name: "list_extract".to_string(),
            args: vec![
                graphdb_core::Expression::List(vec![graphdb_core::Expression::Literal(
                    graphdb_core::Value::BigInt(1),
                )]),
                graphdb_core::Expression::Variable("x".to_string()),
            ],
        };
        let mut store = HashMap::new();
        let fake_id = expr(999);
        store.insert(fake_id.clone(), the_expr);
        let mut analyzer = GroupDependencyAnalyzer::with_expr_store(&schema, false, store.clone());
        analyzer.visit(&fake_id);
        let expected =
            FlattenAll::get_groups_pos_to_flatten_for_groups(analyzer.dependent_groups(), &schema);
        let all = FlattenAll::get_groups_pos_to_flatten_for_expr(&fake_id, &schema, &store);
        assert_eq!(all, expected, "FlattenAll(expr) must equal dependent-only");
        assert!(all.contains(&g1));
        let but_one =
            FlattenAllButOne::get_groups_pos_to_flatten_for_expr(&fake_id, &schema, &store);
        assert!(
            but_one.contains(&g1),
            "required_flat group g1 should be flattened via AllButOne, got {:?}",
            but_one
        );
    }
}

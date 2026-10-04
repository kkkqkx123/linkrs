//! Factorized-plan schema description for the query optimizer.
//!
//! **Naming note (R1).** The types here (`FactorizedSchema`,
//! `FactorizationGroup`, `FactorizationRewriter`) reuse Kuzu/Ladybug
//! terminology. In linkrs they describe a *planning-side* grouping of
//! expressions into nested levels and drive a *minimal-flatten* rewrite —
//! they are **not** a "factorized-table execution engine" (no compressed
//! factor tables, `SEMI_MASKER`, or multiplicity reduction). The benefit is
//! fewer materialization points and smaller fan-out, not skipping the
//! cross-product. Keep this distinction in mind when reading the code or the
//! EXPLAIN output: a `LogicalFlatten` is a cross-product materialization
//! point, not a factorization step.

use graphdb_core::types::expr::ExpressionId;
use thiserror::Error;

mod compute;
mod expression;
mod group;

pub use compute::{SchemaUtils, SinkOperatorUtil};
pub use expression::FactorizedSchema;
pub use group::FactorizationGroup;

/// Factorization group position identifier.
pub type FGroupPos = u32;

/// Invalid group position sentinel.
pub const INVALID_F_GROUP_POS: FGroupPos = u32::MAX;

/// Errors raised while building or validating a [`FactorizedSchema`].
///
/// These replace the previous `assert!`-based invariant checks so a malformed
/// plan surfaces as a query error instead of aborting the server process.
/// See `AGENTS.md`: "Never use unwrap".
#[derive(Error, Debug, Clone, PartialEq)]
pub enum FactorizationError {
    /// More than one group is left unflat at the same nesting level.
    #[error("at most one unflat group allowed, found {0}")]
    TooManyUnflatGroups(usize),
    /// A group that was expected to be flat is still unflat.
    #[error("group {0} expected flat but is unflat")]
    GroupExpectedFlat(FGroupPos),
    /// A group position passed to a flatten/set operation is out of range.
    #[error("group_pos {0} out of range")]
    GroupPosOutOfRange(FGroupPos),
    /// An expression was inserted into scope more than once.
    #[error("expression {0:?} already in scope")]
    ExpressionAlreadyInScope(ExpressionId),
    /// An expression was mapped to a group more than once.
    #[error("expression {0:?} already mapped to group")]
    ExpressionAlreadyMapped(ExpressionId),
    /// A duplicate expression id was registered inside a group.
    #[error("duplicate expression id {0:?} in group")]
    DuplicateExpressionId(ExpressionId),
    /// A duplicate expression name was registered inside a group.
    #[error("duplicate expression name {0} in group")]
    DuplicateExpressionName(String),
    /// `flatten_group` was given an invalid group position.
    #[error("flatten_group: invalid pos {0}")]
    InvalidFlattenPos(FGroupPos),
    /// `set_group_as_single_state` was given an invalid group position.
    #[error("set_group_as_single_state: invalid pos {0}")]
    InvalidSingleStatePos(FGroupPos),
    /// `set_flat` was called on a group that is already flat.
    #[error("group already flat")]
    GroupAlreadyFlat,
    /// `set_single_state` was called on a group that is already single-state.
    #[error("group already single state")]
    GroupAlreadySingleState,
    /// A group position set passed to a leader/validator was empty.
    #[error("groupPositions empty")]
    EmptyGroupPositions,
    /// A non-empty group position set was required but found empty.
    #[error("expected non-empty group positions")]
    NonEmptyGroupPositions,
}

/// Trait for operators that can compute factorized schemas.
///
/// `child_schemas` must be the bottom-up computed results for the direct children;
/// passing an empty slice forces recomputation and violates the factorization invariant.
pub trait FactorizedSchemaCompute {
    fn compute_factorized_schema(
        &mut self,
        child_schemas: &[FactorizedSchema],
    ) -> Result<FactorizedSchema, FactorizationError>;
    fn compute_flat_schema(
        &mut self,
        child_schemas: &[FactorizedSchema],
    ) -> Result<FactorizedSchema, FactorizationError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn expr(id: u64) -> ExpressionId {
        ExpressionId::new(id)
    }

    #[test]
    fn group_basic() {
        let mut g = FactorizationGroup::new();
        assert!(!g.is_flat());
        g.set_flat().unwrap();
        assert!(g.is_flat());
    }

    #[test]
    fn group_single_state_forces_flat() {
        let mut g = FactorizationGroup::new();
        g.set_single_state().unwrap();
        assert!(g.is_flat());
        assert!(g.is_single_state());
    }

    #[test]
    fn schema_single_flat_group() {
        let mut schema = FactorizedSchema::new();
        let pos = schema.create_flat_group(false);
        assert_eq!(pos, 0);
        schema.insert_to_group_and_scope(expr(1), pos).unwrap();
        schema.insert_to_group_and_scope(expr(2), pos).unwrap();
        assert_eq!(schema.num_groups(), 1);
        assert!(schema.get_group(pos).expect("group").is_flat());
        assert_eq!(schema.get_group_pos(&expr(1)), Some(0));
    }

    #[test]
    fn schema_unflat_group_and_flatten() {
        let mut schema = FactorizedSchema::new();
        let flat_pos = schema.create_flat_group(false);
        let unflat_pos = schema.create_group();
        schema
            .insert_to_group_and_scope(expr(10), flat_pos)
            .unwrap();
        schema
            .insert_to_group_and_scope(expr(20), unflat_pos)
            .unwrap();
        assert!(!schema.get_group(unflat_pos).expect("unflat").is_flat());
        assert!(schema.has_unflat_group());
        assert_eq!(schema.unflat_group_pos(), Some(unflat_pos));
        schema.validate_at_most_one_unflat().unwrap();
        schema.flatten_group(unflat_pos).unwrap();
        assert!(schema.is_flat_schema());
        assert!(!schema.has_unflat_group());
    }

    #[test]
    fn schema_at_most_one_unflat_invariant() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_group();
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0).unwrap();
        schema.insert_to_group_and_scope(expr(2), g1).unwrap();
        assert!(schema.validate_at_most_one_unflat().is_err());
    }

    #[test]
    fn flatten_group_validates_invariant_at_runtime() {
        let mut schema = FactorizedSchema::new();
        let flat_pos = schema.create_flat_group(false);
        let g0 = schema.create_group();
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), flat_pos).unwrap();
        schema.insert_to_group_and_scope(expr(2), g0).unwrap();
        schema.insert_to_group_and_scope(expr(3), g1).unwrap();
        assert!(schema.flatten_group(flat_pos).is_err());
    }

    #[test]
    fn schema_copy_and_flat_copy() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0).unwrap();
        schema.insert_to_group_and_scope(expr(2), g1).unwrap();
        let flat = schema.flat_copy().unwrap();
        assert!(flat.is_flat_schema());
        assert!(!schema.is_flat_schema());
        let copied = schema.copy();
        assert_eq!(copied.num_groups(), 2);
    }

    #[test]
    fn schema_utils_leading_group() {
        let mut schema = FactorizedSchema::new();
        let flat = schema.create_flat_group(false);
        let unflat = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), flat).unwrap();
        schema.insert_to_group_and_scope(expr(2), unflat).unwrap();
        let mut set = HashSet::new();
        set.insert(flat);
        set.insert(unflat);
        let leading = SchemaUtils::get_leading_group_pos(&set, &schema).unwrap();
        assert_eq!(leading, unflat);
        let mut flat_only = HashSet::new();
        flat_only.insert(flat);
        let leading2 = SchemaUtils::get_leading_group_pos(&flat_only, &schema).unwrap();
        assert_eq!(leading2, flat);
    }

    #[test]
    fn extend_schema_simulation() {
        let mut scan_schema = FactorizedSchema::new();
        let g0 = scan_schema.create_flat_group(false);
        scan_schema
            .insert_to_group_and_scope(expr(100), g0)
            .unwrap();
        scan_schema
            .insert_to_group_and_scope(expr(101), g0)
            .unwrap();

        let mut extend_schema = scan_schema.copy();
        let g1 = extend_schema.create_group();
        extend_schema
            .insert_to_group_and_scope(expr(200), g1)
            .unwrap();
        extend_schema
            .insert_to_group_and_scope(expr(201), g1)
            .unwrap();
        assert_eq!(extend_schema.num_groups(), 2);
        assert!(extend_schema.get_group(g0).expect("g0").is_flat());
        assert!(!extend_schema.get_group(g1).expect("g1").is_flat());
        extend_schema.validate_at_most_one_unflat().unwrap();
    }

    #[test]
    fn member_names_reports_sorted_aliases_per_group() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0).unwrap();
        schema
            .insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), g1)
            .unwrap();
        schema
            .insert_to_group_and_scope_with_name(expr(3), Some("a".to_string()), g1)
            .unwrap();
        assert_eq!(
            schema.member_names(g1),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(schema.member_names(g0).is_empty());
        assert!(schema.member_names(99).is_empty());
    }

    #[test]
    fn hash_join_merge_schema() {
        let mut left = FactorizedSchema::new();
        let lg = left.create_flat_group(false);
        left.insert_to_group_and_scope(expr(1), lg).unwrap();

        let mut right = FactorizedSchema::new();
        let rg = right.create_group();
        right.insert_to_group_and_scope(expr(2), rg).unwrap();

        let mut merged = left.copy();
        let mapping = merged.merge_groups_from(&right);
        assert_eq!(mapping.get(&0), Some(&1));
        assert_eq!(merged.num_groups(), 2);
    }

    #[test]
    fn merge_groups_from_keeps_expression_id_lookup() {
        let mut left = FactorizedSchema::new();
        let lg = left.create_flat_group(false);
        left.insert_to_group_and_scope(expr(1), lg).unwrap();

        let mut right = FactorizedSchema::new();
        let rg = right.create_group();
        right
            .insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), rg)
            .unwrap();

        let mut merged = left.copy();
        merged.merge_groups_from(&right);
        assert_eq!(merged.get_group_pos(&expr(1)), Some(0));
        assert_eq!(merged.get_group_pos(&expr(2)), Some(1));
        assert!(merged.is_expression_in_scope(&expr(2)));
        assert!(merged.expressions_in_scope().contains(&expr(2)));
        assert_eq!(merged.get_group_pos_by_name_opt("b"), Some(1));
        assert_eq!(merged.expression_name(&expr(2)), Some("b"));
    }

    #[test]
    fn aggregate_flattens_all_but_one() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0).unwrap();
        schema.insert_to_group_and_scope(expr(2), g1).unwrap();
        let mut agg_schema = FactorizedSchema::new();
        let out = agg_schema.create_flat_group(false);
        agg_schema.insert_to_group_and_scope(expr(10), out).unwrap();
        assert!(agg_schema.is_flat_schema());
    }

    #[test]
    fn expression_id_to_name_roundtrip() {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_flat_group(false);
        schema
            .insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), g)
            .unwrap();
        assert_eq!(schema.expression_name(&expr(1)), Some("a"));
        assert_eq!(schema.expression_name(&expr(2)), None);
        let mut other = FactorizedSchema::new();
        other.merge_groups_from(&schema);
        other.insert_to_scope_may_repeat(expr(1), 0).unwrap();
        assert_eq!(other.expression_name(&expr(1)), Some("a"));
        other.clear_expressions_in_scope();
        assert_eq!(other.expression_name(&expr(1)), None);
    }

    #[test]
    fn sink_merge_preserves_multiplier_and_single_state() {
        let mut input = FactorizedSchema::new();
        let flat_pos = input.create_flat_group(false);
        input
            .insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), flat_pos)
            .unwrap();
        let unflat_pos = input.create_group();
        input
            .insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), unflat_pos)
            .unwrap();
        input
            .get_group_mut(unflat_pos)
            .expect("unflat group")
            .set_multiplier(2.5);
        let mut out = FactorizedSchema::new();
        SinkOperatorUtil::recompute_schema(&input, &[expr(1), expr(2)], &mut out).unwrap();
        out.validate_at_most_one_unflat().unwrap();
        assert_eq!(out.num_groups(), 2);
        let a_pos = out.get_group_pos(&expr(1)).expect("a placed");
        let a_group = out.get_group(a_pos).expect("a group");
        assert!(a_group.is_flat());
        assert!(a_group.is_single_state());
        let b_pos = out.get_group_pos(&expr(2)).expect("b placed");
        assert_ne!(a_pos, b_pos);
        let b_group = out.get_group(b_pos).expect("b group");
        assert!(!b_group.is_flat());
        assert_eq!(b_group.cardinality_multiplier(), 2.5);
        assert_eq!(out.get_group_pos_by_name_opt("a"), Some(a_pos));
        assert_eq!(out.get_group_pos_by_name_opt("b"), Some(b_pos));
    }

    #[test]
    fn sink_recompute_all_flat_input() {
        let mut input = FactorizedSchema::new();
        let g0 = input.create_flat_group(false);
        input
            .insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), g0)
            .unwrap();
        let g1 = input.create_flat_group(false);
        input
            .insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), g1)
            .unwrap();
        let mut out = FactorizedSchema::new();
        SinkOperatorUtil::recompute_schema(&input, &[expr(1), expr(2)], &mut out).unwrap();
        assert_eq!(out.num_groups(), 1);
        assert!(out.is_expression_in_scope(&expr(1)));
        assert!(out.is_expression_in_scope(&expr(2)));
        assert!(out.get_group_pos_by_name_opt("a").is_some());
        assert!(out.get_group_pos_by_name_opt("b").is_some());
        out.validate_at_most_one_unflat().unwrap();
    }

    #[test]
    fn sink_merge_partial_scope_leaves_unmerged_names() {
        let mut input = FactorizedSchema::new();
        let flat_pos = input.create_flat_group(false);
        input
            .insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), flat_pos)
            .unwrap();
        let unflat_pos = input.create_group();
        input
            .insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), unflat_pos)
            .unwrap();
        input
            .insert_name_for_group("bare".to_string(), unflat_pos)
            .unwrap();
        let mut out = FactorizedSchema::new();
        SinkOperatorUtil::recompute_schema(&input, &[expr(1)], &mut out).unwrap();
        assert!(out.is_expression_in_scope(&expr(1)));
        assert!(!out.is_expression_in_scope(&expr(2)));
        assert!(out.get_group_pos_by_name_opt("a").is_some());
        assert!(out.get_group_pos_by_name_opt("b").is_none());
        assert!(out.get_group_pos_by_name_opt("bare").is_none());
    }

    #[test]
    fn scope_and_bare_name_registration_reject_out_of_range() {
        let mut schema = FactorizedSchema::new();
        let err = schema
            .insert_to_scope(expr(1), 99)
            .expect_err("scope OOR must fail");
        assert_eq!(err, FactorizationError::GroupPosOutOfRange(99));
        let err = schema
            .insert_to_scope_with_name(expr(1), "a".to_string(), 99)
            .expect_err("named scope OOR must fail");
        assert_eq!(err, FactorizationError::GroupPosOutOfRange(99));
        let err = schema
            .insert_name_for_group("bare".to_string(), 99)
            .expect_err("bare name OOR must fail");
        assert_eq!(err, FactorizationError::GroupPosOutOfRange(99));
    }

    #[test]
    fn duplicate_scope_registration_distinguishes_mapping_from_scope() {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_flat_group(false);
        schema.insert_to_scope(expr(1), g).unwrap();
        let err = schema
            .insert_to_scope(expr(1), g)
            .expect_err("repeat must fail");
        assert_eq!(err, FactorizationError::ExpressionAlreadyMapped(expr(1)));
        let mut other = FactorizedSchema::new();
        let h = other.create_flat_group(false);
        other.expressions_in_scope.insert(expr(2));
        let err = other
            .insert_to_scope(expr(2), h)
            .expect_err("scope repeat must fail");
        assert_eq!(err, FactorizationError::ExpressionAlreadyInScope(expr(2)));
    }

    #[test]
    fn resolve_group_pos_prefers_id_over_bare_name() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema
            .insert_to_group_and_scope_with_name(expr(1), Some("shared".to_string()), g0)
            .unwrap();
        schema
            .insert_name_for_group("shared".to_string(), g1)
            .unwrap();
        assert_eq!(
            schema.resolve_group_pos(Some(&expr(1)), Some("shared")),
            Some(g0)
        );
        assert_eq!(schema.resolve_group_pos(None, Some("shared")), Some(g1));
        assert_eq!(schema.resolve_group_pos(None, Some("ghost")), None);
    }
}

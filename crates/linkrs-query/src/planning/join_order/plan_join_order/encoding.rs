//! Encoding helpers for the join-order enumerator.

use super::{JoinOrderEnumerator, JoinOrderEnumeratorContext};
use crate::planning::plan::logical::LogicalNodeEnum;

impl JoinOrderEnumerator {
    pub(super) fn variable_is_flat(
        schema: &crate::planning::plan::factorization::FactorizedSchema,
        context: &mut JoinOrderEnumeratorContext,
        variable: &str,
    ) -> bool {
        let key = context.join_key(variable);
        let id = key.id().clone();
        if let Some(pos) = schema.get_group_pos(&id) {
            if let Some(group) = schema.get_group(pos) {
                return group.is_flat();
            }
        }
        schema
            .get_group_pos_by_name_opt(variable)
            .and_then(|pos| schema.get_group(pos))
            .is_some_and(|g| g.is_flat())
    }

    /// Deterministic folding for overflow nodes and rel variables.
    pub(super) fn mix_overflow(encoding: u64, salt: u64) -> u64 {
        // SplitMix-style single round: cheap, deterministic, no tables.
        let mut z = encoding
            .wrapping_add(0x9e3779b97f4a7c15)
            .wrapping_add(salt.wrapping_mul(0xbf58476d1ce4e5b9));
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        encoding ^ (z ^ (z >> 31))
    }

    /// Schema helper for DP candidate plans.
    ///
    /// DP enumeration only builds scan leaves plus InnerJoin and WcoIntersect
    /// combinators, so recursion covers exactly those shapes. Leaf scans take
    /// no child schema by construction; expansion, filter and projection nodes
    /// never appear inside DP candidates and are planned outside join order.
    pub(super) fn compute_schema_for_plan(
        plan: &mut LogicalNodeEnum,
    ) -> Result<
        crate::planning::plan::factorization::FactorizedSchema,
        crate::planning::plan::factorization::FactorizationError,
    > {
        use crate::planning::plan::factorization::FactorizedSchemaCompute;
        match plan {
            LogicalNodeEnum::InnerJoin(n) => {
                let mut left = (*n.left).clone();
                let mut right = (*n.right).clone();
                let left_schema = Self::compute_schema_for_plan(&mut left)?;
                let right_schema = Self::compute_schema_for_plan(&mut right)?;
                plan.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::WcoIntersect(n) => {
                let mut child_schemas = Vec::with_capacity(n.deps.len());
                for dep in n.deps.iter().cloned() {
                    let mut owned = dep;
                    child_schemas.push(Self::compute_schema_for_plan(&mut owned)?);
                }
                plan.compute_factorized_schema(&child_schemas)
            }
            _ => plan.compute_factorized_schema(&[]),
        }
    }
}

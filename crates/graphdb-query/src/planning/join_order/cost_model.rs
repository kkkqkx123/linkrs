//! Cost model comparing binary hash joins against WCO intersects.
//!
//! Costs are pairs of `(cost, cardinality)` carried alongside each candidate
//! plan because `LogicalNodeEnum` itself stores no statistics. The formulas
//! follow the reference design: a hash join pays the probe scan plus a build
//! penalty scaled by the flat cardinality of the join keys, while an
//! intersect pays the probe scan plus every build side once.

/// Penalty multiplier applied to build-side key cardinalities.
pub const BUILD_PENALTY: u64 = 2;

/// Cost model comparing binary hash joins against WCO intersects.
#[derive(Debug, Default, Clone, Copy)]
pub struct CostModel;

impl CostModel {
    /// Hash join cost: both child costs plus the probe scan plus the
    /// build penalty scaled by the join-key cardinality.
    pub fn compute_hash_join_cost(
        probe_cost: u64,
        probe_cardinality: u64,
        build_cost: u64,
        join_key_cardinality: u64,
    ) -> u64 {
        probe_cost
            .saturating_add(build_cost)
            .saturating_add(probe_cardinality)
            .saturating_add(BUILD_PENALTY.saturating_mul(join_key_cardinality))
    }

    /// Intersect (WCO) cost: probe cost plus probe scan plus every build
    /// cost plus the output cardinality (every emitted row must be
    /// materialized, mirroring the hash join discipline).
    pub fn compute_intersect_cost(
        probe_cost: u64,
        probe_cardinality: u64,
        build_costs: &[u64],
        output_cardinality: u64,
    ) -> u64 {
        let mut cost = probe_cost.saturating_add(probe_cardinality);
        for build_cost in build_costs {
            cost = cost.saturating_add(*build_cost);
        }
        cost.saturating_add(output_cardinality)
    }

    /// Extend cost: child cost plus child scan.
    pub fn compute_extend_cost(child_cost: u64, child_cardinality: u64) -> u64 {
        child_cost.saturating_add(child_cardinality)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_join_sums_children_probe_and_penalty() {
        assert_eq!(
            CostModel::compute_hash_join_cost(10, 100, 20, 50),
            10 + 20 + 100 + 2 * 50
        );
    }

    #[test]
    fn hash_join_saturates_on_overflow() {
        assert_eq!(
            CostModel::compute_hash_join_cost(u64::MAX, u64::MAX, u64::MAX, u64::MAX),
            u64::MAX
        );
    }

    #[test]
    fn intersect_sums_probe_and_builds() {
        assert_eq!(
            CostModel::compute_intersect_cost(10, 100, &[20, 30], 50),
            210
        );
        assert_eq!(CostModel::compute_intersect_cost(10, 100, &[], 0), 110);
    }

    #[test]
    fn extend_adds_scan() {
        assert_eq!(CostModel::compute_extend_cost(10, 100), 110);
    }
}

//! Join cardinality estimation.
//!
//! Scan and binary-join estimates live here so the DP enumerator can price
//! candidates. Intersect-specific estimation is added in the WCO phase.

use std::collections::HashMap;

use graphdb_core::types::expr::ExpressionId;

/// Selectivity applied to non-equality filter predicates.
pub const NON_EQUALITY_PREDICATE_SELECTIVITY: f64 = 0.2;

/// Default cardinality assumed for a base table scan without statistics.
pub const DEFAULT_SCAN_CARDINALITY: u64 = 1_000;

/// Default average out-degree used for extend pricing without statistics.
pub const DEFAULT_AVG_DEGREE: u64 = 16;

/// Owned statistics snapshot for join ordering.
///
/// This is intentionally decoupled from `optimizer::stats::StatisticsManager`
/// (which depends on the logical plan types): callers that own both sides
/// copy the needed counts into this snapshot. An empty snapshot reproduces
/// the legacy constant behavior.
#[derive(Debug, Default, Clone)]
pub struct JoinOrderStats {
    /// Tag name -> vertex count.
    pub vertex_counts: HashMap<String, u64>,
    /// Edge type -> edge count.
    pub edge_counts: HashMap<String, u64>,
    /// Edge type (or tag) -> average out-degree.
    pub avg_out_degrees: HashMap<String, f64>,
}

impl JoinOrderStats {
    pub fn vertex_count(&self, tag: &str) -> u64 {
        self.vertex_counts.get(tag).copied().unwrap_or(0)
    }

    pub fn edge_count(&self, edge_type: &str) -> u64 {
        self.edge_counts.get(edge_type).copied().unwrap_or(0)
    }

    /// Largest known average out-degree, or the default when unknown.
    pub fn avg_degree_hint(&self) -> u64 {
        self.avg_out_degrees
            .values()
            .copied()
            .fold(0.0f64, f64::max)
            .max(0.0) as u64
    }

    /// Largest known vertex count, or 0 when no statistics are present.
    pub fn max_vertex_count(&self) -> u64 {
        self.vertex_counts.values().copied().max().unwrap_or(0)
    }

    /// Copy tag/edge counts and average degrees for `space` out of the
    /// optimizer statistics manager.
    pub fn from_manager(manager: &crate::optimizer::stats::StatisticsManager, space: &str) -> Self {
        let mut snapshot = Self::default();
        for tag in manager.get_all_tags() {
            if let Some(stats) = manager.get_tag_stats(space, &tag) {
                snapshot
                    .vertex_counts
                    .insert(tag.clone(), stats.vertex_count);
                if stats.avg_out_degree > 0.0 {
                    snapshot.avg_out_degrees.insert(tag, stats.avg_out_degree);
                }
            }
        }
        for edge_type in manager.get_all_edge_types() {
            if let Some(stats) = manager.get_edge_stats(space, &edge_type) {
                snapshot
                    .edge_counts
                    .insert(edge_type.clone(), stats.edge_count);
                if stats.avg_out_degree > 0.0 {
                    snapshot
                        .avg_out_degrees
                        .insert(edge_type, stats.avg_out_degree);
                }
            }
        }
        snapshot
    }
}

/// Estimates output cardinalities for base scans and binary joins.
#[derive(Debug, Default, Clone)]
pub struct CardinalityEstimator {
    node_id_domains: HashMap<ExpressionId, u64>,
    stats: JoinOrderStats,
}

impl CardinalityEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a statistics snapshot; returns the estimator for chaining.
    pub fn with_stats(mut self, stats: JoinOrderStats) -> Self {
        self.stats = stats;
        self
    }

    /// Replace the statistics snapshot.
    pub fn set_stats(&mut self, stats: JoinOrderStats) {
        self.stats = stats;
    }

    /// Borrow the statistics snapshot.
    pub fn stats(&self) -> &JoinOrderStats {
        &self.stats
    }

    /// Record the domain size (distinct values) of a node id expression.
    pub fn set_node_id_domain(&mut self, id: ExpressionId, domain: u64) {
        self.node_id_domains.insert(id, domain);
    }

    /// Domain size of a node id expression, falling back to the scan default.
    pub fn get_node_id_domain(&self, id: &ExpressionId) -> u64 {
        self.node_id_domains
            .get(id)
            .copied()
            .unwrap_or(DEFAULT_SCAN_CARDINALITY)
            .max(1)
    }

    /// Base table scan cardinality.
    pub fn estimate_scan(&self) -> u64 {
        DEFAULT_SCAN_CARDINALITY
    }

    /// Node scan cardinality from label statistics, falling back to the
    /// default when no label has a known count.
    pub fn estimate_node_scan(&self, labels: &[String]) -> u64 {
        labels
            .iter()
            .map(|label| self.stats.vertex_count(label))
            .max()
            .filter(|count| *count > 0)
            .unwrap_or(DEFAULT_SCAN_CARDINALITY)
    }

    /// Rel scan cardinality from edge-type statistics, falling back to the
    /// default when no edge type has a known count.
    pub fn estimate_rel_scan(&self, edge_types: &[String]) -> u64 {
        edge_types
            .iter()
            .map(|edge_type| self.stats.edge_count(edge_type))
            .max()
            .filter(|count| *count > 0)
            .unwrap_or(DEFAULT_SCAN_CARDINALITY)
    }

    /// Average out-degree hint for extend pricing: statistics maximum,
    /// otherwise the legacy default.
    pub fn avg_degree_hint(&self) -> u64 {
        let hint = self.stats.avg_degree_hint();
        if hint > 0 {
            hint
        } else {
            DEFAULT_AVG_DEGREE
        }
    }

    /// Binary hash join cardinality under the independence assumption:
    /// `probe * build / domain` per join key, at least 1.
    pub fn estimate_hash_join(
        &self,
        probe_cardinality: u64,
        build_cardinality: u64,
        join_key_domains: &[u64],
    ) -> u64 {
        let mut numerator = (probe_cardinality as u128).saturating_mul(build_cardinality as u128);
        let mut denominator: u128 = 1;
        for domain in join_key_domains {
            denominator = denominator.saturating_mul((*domain).max(1) as u128);
        }
        if denominator > 0 {
            numerator /= denominator;
        }
        numerator.max(1).min(u64::MAX as u128) as u64
    }

    /// Extend (single-edge expansion) cardinality: child rows fan out by the
    /// average degree estimate.
    pub fn estimate_extend(&self, child_cardinality: u64, avg_degree: u64) -> u64 {
        (child_cardinality as u128)
            .saturating_mul(avg_degree.max(1) as u128)
            .min(u64::MAX as u128) as u64
    }

    /// Intersect (WCO) cardinality: the cheaper of a conservative
    /// probe-filtering estimate and the independence-assumption estimate,
    /// at least 1.
    pub fn estimate_intersect(
        &self,
        probe_cardinality: u64,
        build_cardinalities: &[u64],
        join_key_domains: &[u64],
    ) -> u64 {
        let conservative = (probe_cardinality as f64 * NON_EQUALITY_PREDICATE_SELECTIVITY) as u64;
        let mut numerator = probe_cardinality as u128;
        for build_cardinality in build_cardinalities {
            numerator = numerator.saturating_mul(*build_cardinality as u128);
        }
        let mut denominator: u128 = 1;
        for domain in join_key_domains {
            denominator = denominator.saturating_mul((*domain).max(1) as u128);
        }
        let independent = if denominator > 0 {
            (numerator / denominator).min(u64::MAX as u128) as u64
        } else {
            u64::MAX
        };
        conservative.min(independent).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_join_independence_assumption() {
        let est = CardinalityEstimator::new();
        assert_eq!(est.estimate_hash_join(1_000, 1_000, &[1_000]), 1_000);
        assert_eq!(est.estimate_hash_join(100, 100, &[10_000]), 1);
    }

    #[test]
    fn hash_join_minimum_one() {
        let est = CardinalityEstimator::new();
        assert_eq!(est.estimate_hash_join(0, 0, &[]), 1);
    }

    #[test]
    fn node_domain_roundtrip() {
        let mut est = CardinalityEstimator::new();
        let id = ExpressionId::new(7);
        assert_eq!(est.get_node_id_domain(&id), DEFAULT_SCAN_CARDINALITY);
        est.set_node_id_domain(id.clone(), 42);
        assert_eq!(est.get_node_id_domain(&id), 42);
    }

    #[test]
    fn extend_scales_by_degree() {
        let est = CardinalityEstimator::new();
        assert_eq!(est.estimate_extend(100, 5), 500);
    }

    #[test]
    fn intersect_takes_cheaper_estimate() {
        let est = CardinalityEstimator::new();
        // Conservative: 1000 * 0.2 = 200; independent: 1000*1000/10000 = 100.
        assert_eq!(est.estimate_intersect(1_000, &[1_000], &[10_000]), 100);
        // Conservative: 20; independent: 100*100/10 = 1000 -> 20.
        assert_eq!(est.estimate_intersect(100, &[100], &[10]), 20);
    }

    #[test]
    fn intersect_minimum_one() {
        let est = CardinalityEstimator::new();
        assert_eq!(est.estimate_intersect(0, &[], &[]), 1);
    }

    #[test]
    fn scan_uses_stats_snapshot_when_present() {
        let mut stats = JoinOrderStats::default();
        stats.vertex_counts.insert("person".to_string(), 5_000);
        stats.edge_counts.insert("knows".to_string(), 20_000);
        stats.avg_out_degrees.insert("knows".to_string(), 4.0);
        let est = CardinalityEstimator::new().with_stats(stats);
        assert_eq!(est.estimate_node_scan(&["person".to_string()]), 5_000);
        assert_eq!(est.estimate_rel_scan(&["knows".to_string()]), 20_000);
        assert_eq!(est.avg_degree_hint(), 4);
        // Unknown labels fall back to the default.
        assert_eq!(
            est.estimate_node_scan(&["unknown".to_string()]),
            DEFAULT_SCAN_CARDINALITY
        );
        assert_eq!(
            CardinalityEstimator::new().avg_degree_hint(),
            DEFAULT_AVG_DEGREE
        );
    }
}

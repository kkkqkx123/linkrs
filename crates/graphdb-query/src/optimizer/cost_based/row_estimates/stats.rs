//! Statistics-driven helpers shared by the physical and logical row estimators.

use crate::optimizer::cost::config::DEFAULT_FANOUT;
use crate::optimizer::stats::StatsView;

/// Fallback row multiplier for neighborhood / expansion operators.
/// Single source: `DEFAULT_FANOUT` in cost config.
pub(super) const DEFAULT_NEIGHBORHOOD_FANOUT: u64 = DEFAULT_FANOUT;
/// Default selectivity applied to a filter when the expression gives none.
pub(super) const DEFAULT_FILTER_SELECTIVITY: f64 = 0.1;

/// Average neighborhood fanout for an edge-type list, preferring collected
/// average degrees and falling back to the single no-statistics default.
pub(super) fn stats_fanout(stats: &StatsView, edge_types: &[String]) -> u64 {
    edge_types
        .iter()
        .find_map(|edge_type| {
            stats
                .edge_stats(edge_type)
                .map(|s| (s.avg_out_degree.max(0.0)) as u64)
        })
        .filter(|fanout| *fanout > 0)
        .unwrap_or(DEFAULT_NEIGHBORHOOD_FANOUT)
}

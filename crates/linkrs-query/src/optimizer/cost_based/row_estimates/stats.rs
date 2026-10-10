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
///
/// Degree choice is the rounded max of average out/in degree so IN and BOTH
/// traversals are not underpriced by an out-only figure, and the max across
/// edge types (not the first hit) so a low first type cannot shrink the
/// estimate. Rounding (not truncation) keeps fractional averages such as 2.9
/// at 3 instead of 2.
pub(super) fn stats_fanout(stats: &StatsView, edge_types: &[String]) -> u64 {
    edge_types
        .iter()
        .filter_map(|edge_type| {
            stats
                .edge_stats(edge_type)
                .map(|s| (s.avg_out_degree.max(s.avg_in_degree).round().max(0.0)) as u64)
        })
        .max()
        .filter(|fanout| *fanout > 0)
        .unwrap_or(DEFAULT_NEIGHBORHOOD_FANOUT)
}

/// Skew-aware expansion fanout sharing the edge statistics skew definition.
///
/// Multiplies the base fanout by the largest `skew_row_factor` across the
/// edge types so heavily skewed neighborhoods price like the traversal cost
/// estimator does; uniform graphs keep a factor of 1.0 and are unaffected.
pub(super) fn stats_fanout_skewed(stats: &StatsView, edge_types: &[String]) -> u64 {
    let base = stats_fanout(stats, edge_types) as f64;
    let factor = edge_types
        .iter()
        .filter_map(|edge_type| stats.edge_stats(edge_type))
        .map(|s| s.skew_row_factor())
        .fold(1.0f64, f64::max);
    ((base * factor).round() as u64).max(1)
}

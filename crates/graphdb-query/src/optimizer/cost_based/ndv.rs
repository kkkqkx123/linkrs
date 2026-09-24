//! Distinct-value (NDV) lookup helpers for cardinality estimation.
//!
//! Bridges the statistics collected by `StatisticsCollector` (per-property
//! NDV samples, combination cardinality, tag/edge counts) into the row
//! estimate layer. Group-by and join cardinality estimates consult these
//! helpers before falling back to fixed-selectivity heuristics.

use crate::optimizer::stats::StatsView;
use graphdb_core::types::ContextualExpression;

/// Fallback join selectivity when no NDV is available for either side.
pub const DEFAULT_JOIN_SELECTIVITY: f64 = 0.3;

/// Try to obtain NDV for a column from `StatsView`.
///
/// Preference order:
/// 1. Per-property `distinct_values` from column statistics
///    (populated by `StatisticsCollector::collect_property_stats`).
/// 2. Tag vertex count as a coarse upper bound for vertex ID columns.
pub fn ndv_from_stats(stats: &StatsView, tag: Option<&str>, col: &str) -> Option<u64> {
    if let Some(ndv) = stats.property_ndv(tag, col) {
        return Some(ndv);
    }
    // For edge-type qualified property names the tag is the edge type itself.
    // Try again without tag qualification (global property fallback).
    if tag.is_some() {
        if let Some(ndv) = stats.property_ndv(None, col) {
            return Some(ndv);
        }
    }
    // Fallback: tag vertex count only for vertex-ID-like columns (the
    // column is the tag name itself or an id field). This keeps generic
    // property groups (e.g. "n.age") from incorrectly borrowing the full
    // vertex cardinality and collapsing the GROUP BY selectivity to 1.0.
    if let Some(t) = tag {
        if col == t || col.eq_ignore_ascii_case("id") || col.eq_ignore_ascii_case("vid") {
            if let Some(tag_stats) = stats.tag_stats(t) {
                if tag_stats.vertex_count > 0 {
                    return Some(tag_stats.vertex_count);
                }
            }
        }
    }
    None
}

/// NDV for a grouping key set (GROUP BY / factorized key).
///
/// When combination statistics are available (`PropertyCombinationStats`),
/// their joint NDV is preferred; otherwise the product of per-column NDVs
/// (capped by the input row count elsewhere) is returned.
pub fn ndv_for_group_keys(stats: &StatsView, tag: Option<&str>, keys: &[String]) -> Option<u64> {
    if keys.is_empty() {
        return None;
    }
    if let Some(combined) = stats.combined_cardinality(tag, keys) {
        return Some(combined);
    }
    // Product of per-column NDVs as a conservative estimate (upper bound).
    let mut prod: u64 = 1;
    for key in keys {
        // Keys may be qualified like "n.age" - strip alias.
        let col = key.split('.').next_back().unwrap_or(key);
        let ndv = ndv_from_stats(stats, tag, col)?;
        prod = prod.saturating_mul(ndv.max(1));
    }
    Some(prod)
}

/// Extract the property column referenced by a join key.
///
/// Prefers the property name (`n.id` -> `id`); falls back to the trailing
/// segment of the rendered expression so plain variables still resolve.
fn join_key_column(expr: &ContextualExpression) -> Option<String> {
    if let Some(name) = expr.as_property_name() {
        if !name.is_empty() {
            return Some(name);
        }
    }
    let rendered = expr.to_expression_string();
    rendered
        .rsplit('.')
        .next()
        .map(|s| {
            s.trim()
                .trim_matches(|c| c == '`' || c == '"' || c == '\'' || c == ')' || c == '(')
                .to_string()
        })
        .filter(|s| !s.is_empty())
}

/// Containment-based equi-join selectivity from NDV statistics.
///
/// Per key pair uses `1 / max(ndv_left, ndv_right)`; multiple pairs multiply.
/// Returns `None` when no pair has NDV on either side, letting callers keep
/// the fixed fallback instead of inventing precision.
pub fn join_selectivity(
    stats: &StatsView,
    left_keys: &[ContextualExpression],
    right_keys: &[ContextualExpression],
) -> Option<f64> {
    let mut selectivity = 1.0;
    let mut matched = false;
    for (left, right) in left_keys.iter().zip(right_keys.iter()) {
        let left_col = join_key_column(left)?;
        let right_col = join_key_column(right)?;
        let left_ndv = stats
            .property_ndv(None, &left_col)
            .filter(|&n| n > 0)?;
        let right_ndv = stats
            .property_ndv(None, &right_col)
            .filter(|&n| n > 0)?;
        let pair = 1.0 / u64::max(left_ndv, right_ndv).max(1) as f64;
        selectivity *= pair.clamp(0.0, 1.0);
        matched = true;
    }
    if matched {
        Some(selectivity.clamp(1e-9, 1.0))
    } else {
        None
    }
}

/// Join selectivity with fallback: NDV estimate when available, else `fallback`.
pub fn refine_join_selectivity(
    stats: &StatsView,
    left_keys: &[ContextualExpression],
    right_keys: &[ContextualExpression],
    fallback: f64,
) -> f64 {
    join_selectivity(stats, left_keys, right_keys).unwrap_or(fallback)
}

/// Containment-based equi-join output rows: `left * right * selectivity`.
pub fn join_output_rows(left_rows: u64, right_rows: u64, selectivity: f64) -> u64 {
    ((left_rows as f64 * right_rows as f64) * selectivity.clamp(0.0, 1.0)).max(1.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::stats::{PropertyStatistics, StatisticsManager};
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{Expression, ExpressionMeta};
    use std::sync::Arc;

    fn variable_key(
        ctx: &Arc<ExpressionAnalysisContext>,
        name: &str,
    ) -> ContextualExpression {
        let meta = ExpressionMeta::new(Expression::Variable(name.to_string()));
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx.clone())
    }

    fn stats_with_ndv(space: &str, col: &str, ndv: u64) -> StatisticsManager {
        let manager = StatisticsManager::new();
        let mut stats = PropertyStatistics::new(col.to_string(), None);
        stats.distinct_values = ndv;
        manager.update_property_stats(space, stats);
        manager
    }

    #[test]
    fn test_join_selectivity_uses_ndv_containment() {
        let manager = StatisticsManager::new();
        for (col, ndv) in [("lid", 1000u64), ("rid", 100u64)] {
            let mut stats = PropertyStatistics::new(col.to_string(), None);
            stats.distinct_values = ndv;
            manager.update_property_stats("s", stats);
        }
        let view = StatsView::new(&manager, Some("s"));
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = vec![variable_key(&ctx, "l.lid")];
        let right = vec![variable_key(&ctx, "r.rid")];
        let sel = join_selectivity(&view, &left, &right).expect("NDV available");
        assert!((sel - 0.001).abs() < 1e-12, "selectivity={sel}");
        assert_eq!(join_output_rows(10_000, 500, sel), 5_000);
    }

    #[test]
    fn test_join_selectivity_falls_back_without_stats() {
        let manager = StatisticsManager::new();
        let view = StatsView::new(&manager, Some("s"));
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = vec![variable_key(&ctx, "l.id")];
        let right = vec![variable_key(&ctx, "r.id")];
        assert!(join_selectivity(&view, &left, &right).is_none());
        assert_eq!(
            refine_join_selectivity(&view, &left, &right, DEFAULT_JOIN_SELECTIVITY),
            DEFAULT_JOIN_SELECTIVITY
        );
    }

    #[test]
    fn test_stats_with_ndv_helper() {
        let manager = stats_with_ndv("s", "id", 42);
        let view = StatsView::new(&manager, Some("s"));
        assert_eq!(view.property_ndv(None, "id"), Some(42));
    }
}

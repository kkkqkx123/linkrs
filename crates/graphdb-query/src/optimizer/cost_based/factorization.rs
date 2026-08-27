//! Distinct-value (NDV) lookup helpers for cardinality estimation.
//!
//! Bridges the statistics collected by `StatisticsCollector` (per-property
//! NDV samples, combination cardinality, tag/edge counts) into the row
//! estimate layer. Group-by and join cardinality estimates consult these
//! helpers before falling back to fixed-selectivity heuristics.

use crate::optimizer::stats::StatsView;

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

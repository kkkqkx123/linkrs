//! Factorization cost model.
//!
//! Extends the logical row-estimate layer with factorized compression
//! awareness.  When an `Expand` chain operates on lists that can stay
//! factorized (via `ListVector`), the *logical* flat row count overstates
//! both memory and CPU.  This module estimates the compressed row count
//! and the benefit of injecting `SemiMasker` / `MultiplicityReducer`.

use crate::query::optimizer::stats::StatsView;

// ── Compression estimation ──────────────────────────────────────────────────

/// Estimated result of factorizing `flat_rows` over `group_keys`.
///
/// `ndv` - number of distinct values of the group key (from zone maps /
/// statistics).  When absent, `flat_rows` itself is used (no compression).
/// `avg_degree` - average adjacency fanout for expands (from edge stats).
#[derive(Debug, Clone)]
pub struct FactorizationEstimate {
    /// Estimated flat rows without factorization.
    pub flat_rows: u64,
    /// Estimated compressed rows (factorized groups).
    pub factorized_rows: u64,
    /// Compression ratio (flat / factorized, >= 1.0).
    pub compression_ratio: f64,
    /// Whether factorization is expected to save memory/CPU.
    pub beneficial: bool,
    /// Estimated memory bytes saved (positive when beneficial).
    pub memory_saving: f64,
}

/// Estimate factorization benefit for `flat_rows` over a group key.
///
/// Caller provides `ndv` from column statistics / zone maps and
/// `avg_degree` from edge degree stats.  The estimate mirrors
/// `chunk::factorized::estimate_factorized_rows` but adds a cost-model
/// layer (memory saving, threshold).
pub fn estimate_factorization(
    flat_rows: u64,
    ndv: Option<u64>,
    avg_degree: f64,
    min_compression_ratio: f64,
) -> FactorizationEstimate {
    if flat_rows == 0 {
        return FactorizationEstimate {
            flat_rows: 0,
            factorized_rows: 0,
            compression_ratio: 1.0,
            beneficial: false,
            memory_saving: 0.0,
        };
    }
    let distinct = ndv.unwrap_or(flat_rows);
    let factorized_rows = distinct.min(flat_rows).max(1);
    let compression_ratio = if factorized_rows == 0 {
        1.0
    } else {
        flat_rows as f64 / factorized_rows as f64 * avg_degree.max(1.0)
    };
    let beneficial = compression_ratio >= min_compression_ratio && factorized_rows < flat_rows;
    let memory_saving = if beneficial {
        1.0 - (factorized_rows as f64 / flat_rows as f64)
    } else {
        0.0
    };
    FactorizationEstimate {
        flat_rows,
        factorized_rows,
        compression_ratio,
        beneficial,
        memory_saving,
    }
}

/// Whether an Expand chain should stay factorized.
///
/// Heuristic: expand fanout * downstream selectivity < threshold means
/// most expanded rows will be pruned, so semi-mask pushdown wins.
pub fn should_keep_factorized(
    expand_flat_rows: u64,
    downstream_selectivity: f64,
    threshold: f64,
) -> bool {
    let kept = expand_flat_rows as f64 * downstream_selectivity;
    let saving = 1.0 - downstream_selectivity;
    kept < expand_flat_rows as f64 * threshold && saving > 0.2
}

/// Semi-mask selectivity from zone maps / column stats.
///
/// `mask_distinct` - distinct keys in the build-side mask.
/// `probe_ndv` - distinct keys probed in the expand side.
pub fn semi_mask_selectivity(mask_distinct: u64, probe_ndv: Option<u64>) -> f64 {
    match probe_ndv {
        Some(ndv) if ndv > 0 => (mask_distinct as f64 / ndv as f64).clamp(0.0, 1.0),
        _ => 1.0,
    }
}

/// Integrate zone-map pruning into row estimation.
///
/// `zone_maps` provide per-chunk min/max/ndv for a column. This helper
/// estimates how many zones survive a predicate `col BETWEEN lo AND hi`.
/// Used by `row_estimates.rs` for columnar scans.
pub fn zone_map_pruning_factor(num_zones: usize, zones_with_overlap: usize) -> f64 {
    if num_zones == 0 {
        return 1.0;
    }
    (zones_with_overlap as f64 / num_zones as f64).clamp(0.0, 1.0)
}

/// Cost of a factorized vs. flat plan for the same logical subtree.
///
/// Includes a simple model: `flat_cost = flat_rows * per_row_cpu`,
/// `factorized_cost = factorized_rows * per_row_cpu + mask_build_cost`.
#[derive(Debug, Clone)]
pub struct FactorizationCost {
    pub flat_cost: f64,
    pub factorized_cost: f64,
    pub speedup: f64,
}

pub fn compare_factorized_cost(
    flat_rows: u64,
    factorized_rows: u64,
    mask_build_rows: u64,
) -> FactorizationCost {
    const PER_ROW_CPU: f64 = 1.0;
    const MASK_BUILD_PER_ROW: f64 = 2.0;
    let flat_cost = flat_rows as f64 * PER_ROW_CPU;
    let factorized_cost =
        factorized_rows as f64 * PER_ROW_CPU + mask_build_rows as f64 * MASK_BUILD_PER_ROW;
    let speedup = if factorized_cost == 0.0 {
        1.0
    } else {
        flat_cost / factorized_cost
    };
    FactorizationCost {
        flat_cost,
        factorized_cost,
        speedup,
    }
}

// ── Stats view helpers ──────────────────────────────────────────────────────

/// Try to obtain NDV for a column from `StatsView`.
///
/// Preference order:
/// 1. Per-property `distinct_values` from column statistics / zone maps
///    (populated by `StatisticsCollector::collect_property_stats`).
/// 2. Edge adjacency degree (`avg_out_degree`) for unqualified adjacency columns.
/// 3. Tag vertex count as a coarse upper bound for vertex ID columns.
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
    if let Some(edge_stats) = stats.edge_stats(col) {
        if edge_stats.avg_out_degree > 0.0 {
            return Some(edge_stats.avg_out_degree as u64);
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
        let col = key.split('.').last().unwrap_or(key);
        if let Some(ndv) = ndv_from_stats(stats, tag, col) {
            prod = prod.saturating_mul(ndv.max(1));
        } else {
            return None;
        }
    }
    Some(prod)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factorization_basic() {
        let est = estimate_factorization(1000, Some(100), 1.0, 2.0);
        assert_eq!(est.factorized_rows, 100);
        assert!(est.beneficial);
        assert!((est.compression_ratio - 10.0).abs() < 1e-9);
        assert!(est.memory_saving > 0.8);
    }

    #[test]
    fn factorization_not_beneficial_when_low_compression() {
        let est = estimate_factorization(100, Some(90), 1.0, 2.0);
        assert!(!est.beneficial);
    }

    #[test]
    fn factorization_with_degree() {
        let est = estimate_factorization(1000, Some(100), 5.0, 2.0);
        // ratio incorporates degree
        assert!(est.compression_ratio > 40.0);
    }

    #[test]
    fn should_keep_factorized_heuristic() {
        assert!(should_keep_factorized(10000, 0.1, 0.5));
        assert!(!should_keep_factorized(100, 0.9, 0.5));
    }

    #[test]
    fn semi_mask_selectivity_calc() {
        assert!((semi_mask_selectivity(100, Some(1000)) - 0.1).abs() < 1e-9);
        assert_eq!(semi_mask_selectivity(100, None), 1.0);
    }

    #[test]
    fn zone_map_pruning() {
        assert!((zone_map_pruning_factor(10, 2) - 0.2).abs() < 1e-9);
        assert_eq!(zone_map_pruning_factor(0, 0), 1.0);
    }

    #[test]
    fn cost_comparison() {
        let cost = compare_factorized_cost(10000, 1000, 100);
        assert!(cost.speedup > 5.0);
        let cost2 = compare_factorized_cost(100, 90, 1000);
        assert!(cost2.speedup < 1.0);
    }
}

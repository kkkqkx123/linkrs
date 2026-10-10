//! Physical plan row-count estimation walker.

use crate::optimizer::cost::config::{
    DEDUP_SELECTIVITY, GET_EDGES_DEFAULT_ROWS, GET_VERTICES_DEFAULT_ROWS, UNKNOWN_SCAN_ROWS,
};
use crate::optimizer::cost::SelectivityEstimator;
use crate::optimizer::cost_based::ndv as factor_cost;
use crate::optimizer::cost_based::ndv::DEFAULT_JOIN_SELECTIVITY;
use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::optimizer::stats::StatsView;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::PlanNodeEnum;

use super::cardinality::corrected_rows;
use super::stats::{stats_fanout_skewed, DEFAULT_FILTER_SELECTIVITY, DEFAULT_NEIGHBORHOOD_FANOUT};

/// Estimate the output row count of a logical node, post-order.
///
/// `stats` drives leaf scan estimates; `selectivity` is used for filters.
/// The result is a conservative upper-bound style estimate; nodes without
/// statistics fall back to fixed heuristics.
pub fn estimate_node_output_rows(
    node: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
) -> u64 {
    estimate_node_output_rows_impl(node, stats, selectivity, None)
}

/// Like [`estimate_node_output_rows`] but applies learned per-shape
/// cardinality corrections.
///
/// Consumed by cost-based decisions (subquery unnesting, TopN conversion);
/// the raw variant stays for the plan writeback so recorded feedback
/// measures the uncorrected estimate error.
pub fn estimate_node_output_rows_corrected(
    node: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
    cardinality: &CardinalityFeedbackManager,
) -> u64 {
    estimate_node_output_rows_impl(node, stats, selectivity, Some(cardinality))
}

fn estimate_node_output_rows_impl(
    node: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
    cardinality: Option<&CardinalityFeedbackManager>,
) -> u64 {
    use PlanNodeEnum::*;

    match node {
        // ── Leaf scans ──
        // Scan-level filters are NOT discounted here: the `Filter` node that
        // carries the same predicate sits directly above the scan and applies
        // its selectivity exactly once. Discounting at both levels would
        // square the selectivity. When storage-side chunk pruning (zone maps)
        // lands, introduce an explicit IO-reduction factor here instead.
        ScanVertices(n) => {
            let tag_rows = n
                .tag()
                .map(|tag| stats.vertex_count(tag))
                .unwrap_or(UNKNOWN_SCAN_ROWS);
            let raw = n
                .limit()
                .map(|limit| tag_rows.min(limit as u64))
                .unwrap_or(tag_rows);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        ScanEdges(n) => {
            let edge_rows = n
                .edge_type()
                .map(|edge_type| stats.edge_count(&edge_type))
                .unwrap_or(UNKNOWN_SCAN_ROWS);
            let raw = n
                .limit()
                .map(|limit| edge_rows.min(limit as u64))
                .unwrap_or(edge_rows);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        GetVertices(n) => corrected_rows(
            node,
            n.limit().unwrap_or(GET_VERTICES_DEFAULT_ROWS as i64).max(1) as u64,
            stats.space(),
            cardinality,
        ),
        GetEdges(n) => corrected_rows(
            node,
            n.limit().unwrap_or(GET_EDGES_DEFAULT_ROWS as i64).max(1) as u64,
            stats.space(),
            cardinality,
        ),
        GetNeighbors(n) => {
            let fanout = n
                .edge_types()
                .iter()
                .find_map(|edge_type| {
                    stats
                        .edge_stats(edge_type)
                        .map(|s| (s.avg_out_degree.max(0.0)) as u64)
                })
                .unwrap_or(DEFAULT_NEIGHBORHOOD_FANOUT);
            // A limit truncates the neighborhood, so the estimate is capped
            // by it instead of widened to the fanout.
            let estimated = fanout.max(1);
            let raw = match n.limit() {
                Some(limit) => (limit.max(1) as u64).min(estimated),
                None => estimated,
            };
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        IndexScan(n) => corrected_rows(
            node,
            n.limit().unwrap_or(UNKNOWN_SCAN_ROWS as i64).max(1) as u64,
            stats.space(),
            cardinality,
        ),
        Start(_) | Argument(_) => 1,

        // ── Single-input operations ──
        Filter(n) => {
            let input_rows =
                estimate_node_output_rows_impl(n.input(), stats, selectivity, cardinality);
            let condition = n.condition();
            let tag_name = first_tag_of_input(n.input());
            let expr_selectivity = condition
                .expression()
                .map(|meta| {
                    selectivity.estimate_from_expression(
                        stats.space(),
                        meta.inner(),
                        tag_name.as_deref(),
                    )
                })
                .unwrap_or(DEFAULT_FILTER_SELECTIVITY);
            (input_rows as f64 * expr_selectivity).max(1.0) as u64
        }
        Project(_) | Sort(_) | Sample(_) | Window(_) => {
            child_rows_of_impl(node, stats, selectivity, cardinality)
        }
        TopN(n) => {
            let input_rows =
                estimate_node_output_rows_impl(n.input(), stats, selectivity, cardinality);
            input_rows.min(n.limit() as u64)
        }
        Limit(n) => {
            let input_rows =
                estimate_node_output_rows_impl(n.input(), stats, selectivity, cardinality);
            input_rows.min((n.offset() + n.count()) as u64)
        }
        Dedup(n) => {
            let input_rows =
                estimate_node_output_rows_impl(n.input(), stats, selectivity, cardinality);
            (input_rows as f64 * DEDUP_SELECTIVITY).max(1.0) as u64
        }
        Aggregate(n) => {
            let input_rows =
                estimate_node_output_rows_impl(n.input(), stats, selectivity, cardinality);
            let raw = if n.group_keys().is_empty() {
                1
            } else {
                // Columnar NDV-aware group cardinality: prefer joint NDV from
                // `PropertyCombinationStats`, else the single shared
                // `estimate_group_count` fallback. Capped by input rows.
                let tag_for_ndv = first_tag_of_input(n.input());
                let ndv =
                    factor_cost::ndv_for_group_keys(stats, tag_for_ndv.as_deref(), n.group_keys());
                if let Some(distinct) = ndv {
                    distinct.min(input_rows).max(1)
                } else {
                    factor_cost::estimate_group_count(input_rows, n.group_keys().len())
                }
            };
            corrected_rows(node, raw, stats.space(), cardinality)
        }

        // ── Binary operators ──
        // Joins use containment selectivity when NDV is known, else the
        // shared default fallback so logical and physical tracks agree.
        InnerJoin(n) => {
            let children = node.children();
            let raw = if children.len() >= 2 {
                let left =
                    estimate_node_output_rows_impl(children[0], stats, selectivity, cardinality);
                let right =
                    estimate_node_output_rows_impl(children[1], stats, selectivity, cardinality);
                let sel = factor_cost::join_selectivity(stats, n.hash_keys(), n.probe_keys())
                    .unwrap_or(DEFAULT_JOIN_SELECTIVITY);
                factor_cost::join_output_rows(left, right, sel)
            } else {
                child_rows_of_impl(node, stats, selectivity, cardinality)
            };
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        LeftJoin(_) => {
            let children = node.children();
            let raw = if children.len() >= 2 {
                estimate_node_output_rows_impl(children[0], stats, selectivity, cardinality)
            } else {
                child_rows_of_impl(node, stats, selectivity, cardinality)
            };
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        RightJoin(_) => {
            let children = node.children();
            let raw = if children.len() >= 2 {
                estimate_node_output_rows_impl(children[1], stats, selectivity, cardinality)
            } else {
                child_rows_of_impl(node, stats, selectivity, cardinality)
            };
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        CrossJoin(_) => {
            let children = node.children();
            let raw = if children.len() >= 2 {
                let left =
                    estimate_node_output_rows_impl(children[0], stats, selectivity, cardinality);
                let right =
                    estimate_node_output_rows_impl(children[1], stats, selectivity, cardinality);
                left.saturating_mul(right)
            } else {
                child_rows_of_impl(node, stats, selectivity, cardinality)
            };
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        FullOuterJoin(_) => {
            let children = node.children();
            let raw = if children.len() >= 2 {
                let left =
                    estimate_node_output_rows_impl(children[0], stats, selectivity, cardinality);
                let right =
                    estimate_node_output_rows_impl(children[1], stats, selectivity, cardinality);
                left.saturating_add(right)
            } else {
                child_rows_of_impl(node, stats, selectivity, cardinality)
            };
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        SemiJoin(_) => child_rows_of_impl(node, stats, selectivity, cardinality),
        Union(_) => {
            let mut total = 0u64;
            for child in node.children() {
                total = total.saturating_add(estimate_node_output_rows_impl(
                    child,
                    stats,
                    selectivity,
                    cardinality,
                ));
            }
            corrected_rows(node, total, stats.space(), cardinality)
        }
        Minus(_) | Intersect(_) => {
            let mut smallest = u64::MAX;
            for child in node.children() {
                smallest = smallest.min(estimate_node_output_rows_impl(
                    child,
                    stats,
                    selectivity,
                    cardinality,
                ));
            }
            let raw = if smallest == u64::MAX { 0 } else { smallest };
            corrected_rows(node, raw, stats.space(), cardinality)
        }

        // ── Traversal / apply operators ──
        // Expansion fanouts are skew-aware so skewed neighborhoods price
        // like the traversal cost estimator; uniform graphs are unaffected.
        Expand(n) => {
            let fanout = stats_fanout_skewed(stats, n.edge_types());
            let raw =
                child_rows_of_impl(node, stats, selectivity, cardinality).saturating_mul(fanout);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        ExpandAll(n) => {
            let fanout = stats_fanout_skewed(stats, n.edge_types());
            let raw =
                child_rows_of_impl(node, stats, selectivity, cardinality).saturating_mul(fanout);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        Traverse(n) => {
            let fanout = stats_fanout_skewed(stats, n.edge_types());
            let raw =
                child_rows_of_impl(node, stats, selectivity, cardinality).saturating_mul(fanout);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        BiExpand(n) => {
            let fanout = stats_fanout_skewed(stats, n.edge_types());
            let raw =
                child_rows_of_impl(node, stats, selectivity, cardinality).saturating_mul(fanout);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        BiTraverse(n) => {
            let fanout = stats_fanout_skewed(stats, n.edge_types());
            let raw =
                child_rows_of_impl(node, stats, selectivity, cardinality).saturating_mul(fanout);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        AppendVertices(_) => {
            // Flat estimate: input rows times the average neighborhood fanout.
            // No factorized discount is applied here — factorized execution is
            // not wired into the executor, so claiming compressed row counts
            // would misprice plans against the representation actually run.
            let raw = child_rows_of_impl(node, stats, selectivity, cardinality)
                .saturating_mul(DEFAULT_NEIGHBORHOOD_FANOUT);
            corrected_rows(node, raw, stats.space(), cardinality)
        }
        PatternApply(_) | Apply(_) | RollUpApply(_) => {
            let children = node.children();
            let mut total = 1u64;
            for child in children {
                total = total.saturating_mul(estimate_node_output_rows_impl(
                    child,
                    stats,
                    selectivity,
                    cardinality,
                ));
            }
            corrected_rows(node, total, stats.space(), cardinality)
        }

        // ── Pass-through / control flow ──
        PassThrough(_) | Materialize(_) | Unwind(_) | DataCollect(_) | Remove(_) | Assign(_) => {
            child_rows_of_impl(node, stats, selectivity, cardinality)
        }

        // ── Leaf or unsupported nodes: fall back to the input or a constant ──
        node => child_rows_of_impl(node, stats, selectivity, cardinality),
    }
}

/// Estimate of the first child (pass-through semantics), or 1 for leaves.
fn child_rows_of_impl(
    node: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
    cardinality: Option<&CardinalityFeedbackManager>,
) -> u64 {
    node.children()
        .first()
        .map(|child| estimate_node_output_rows_impl(child, stats, selectivity, cardinality))
        .unwrap_or(1)
}
/// The tag referenced by a leaf scan (if any), for filter selectivity.
fn first_tag_of_input(node: &PlanNodeEnum) -> Option<String> {
    match node {
        PlanNodeEnum::ScanVertices(n) => n.tag().cloned(),
        _ => None,
    }
}

//! Logical plan row-count estimation walker.

use crate::optimizer::cost::config::{
    DEDUP_SELECTIVITY, GET_EDGES_DEFAULT_ROWS, GET_VERTICES_DEFAULT_ROWS, UNKNOWN_SCAN_ROWS,
};
use crate::optimizer::cost::SelectivityEstimator;
use crate::optimizer::cost_based::ndv as factor_cost;
use crate::optimizer::cost_based::ndv::DEFAULT_JOIN_SELECTIVITY;
use crate::optimizer::stats::StatsView;
use crate::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;
use crate::planning::plan::logical::LogicalNodeEnum;

use super::stats::{stats_fanout, DEFAULT_FILTER_SELECTIVITY, DEFAULT_NEIGHBORHOOD_FANOUT};

/// Estimate the output row count of a logical node, post-order.
///
/// The estimation heuristics mirror [`estimate_node_output_rows`]; the
/// logical tree only contains operators produced by
/// `conversion::convert_plan`, and anything unsupported falls back to the
/// child estimate or 1.
pub fn estimate_node_output_rows_logical(
    node: &LogicalNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
) -> u64 {
    use LogicalNodeEnum::*;

    match node {
        // ── Leaf scans ──
        ScanVertices(n) => {
            let tag_rows = n
                .tag
                .as_deref()
                .map(|tag| stats.vertex_count(tag))
                .unwrap_or(UNKNOWN_SCAN_ROWS);
            n.limit
                .map(|limit| tag_rows.min(limit as u64))
                .unwrap_or(tag_rows)
        }
        ScanEdges(n) => {
            let edge_rows = n
                .edge_type
                .as_deref()
                .map(|edge_type| stats.edge_count(edge_type))
                .unwrap_or(UNKNOWN_SCAN_ROWS);
            n.limit
                .map(|limit| edge_rows.min(limit as u64))
                .unwrap_or(edge_rows)
        }
        GetVertices(n) => n.limit.unwrap_or(GET_VERTICES_DEFAULT_ROWS as i64).max(1) as u64,
        GetEdges(n) => n.limit.unwrap_or(GET_EDGES_DEFAULT_ROWS as i64).max(1) as u64,
        GetNeighbors(n) => {
            let fanout = n
                .edge_types
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
            match n.limit {
                Some(limit) => (limit.max(1) as u64).min(estimated),
                None => estimated,
            }
        }
        Start(_) => 1,

        // ── Single-input operations ──
        Filter(n) => {
            let input_rows = estimate_node_output_rows_logical(n.input(), stats, selectivity);
            let tag_name = first_tag_of_logical_input(n.input());
            let expr_selectivity = n
                .condition
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
            child_rows_of_logical(node, stats, selectivity)
        }
        TopN(n) => {
            let input_rows = estimate_node_output_rows_logical(n.input(), stats, selectivity);
            input_rows.min(n.limit as u64)
        }
        Limit(n) => {
            let input_rows = estimate_node_output_rows_logical(n.input(), stats, selectivity);
            input_rows.min((n.offset + n.count) as u64)
        }
        Skip(n) => {
            let input_rows = estimate_node_output_rows_logical(n.input(), stats, selectivity);
            input_rows.saturating_sub(n.offset.max(0) as u64)
        }
        Dedup(n) => {
            let input_rows = estimate_node_output_rows_logical(n.input(), stats, selectivity);
            (input_rows as f64 * DEDUP_SELECTIVITY).max(1.0) as u64
        }
        Aggregate(n) => {
            let input_rows = estimate_node_output_rows_logical(n.input(), stats, selectivity);
            if n.group_key_exprs.is_empty() {
                1
            } else {
                let tag_for_ndv = first_tag_of_logical_input(n.input());
                let keys: Vec<String> = n
                    .group_key_exprs
                    .iter()
                    .map(|e| e.to_expression_string())
                    .collect();
                let ndv = factor_cost::ndv_for_group_keys(stats, tag_for_ndv.as_deref(), &keys);
                if let Some(distinct) = ndv {
                    distinct.min(input_rows).max(1)
                } else {
                    factor_cost::estimate_group_count(input_rows, keys.len())
                }
            }
        }

        // ── Binary operators ──
        InnerJoin(n) => {
            let Some((left_node, right_node)) = logical_binary_inputs(node) else {
                return child_rows_of_logical(node, stats, selectivity);
            };
            let left = estimate_node_output_rows_logical(left_node, stats, selectivity);
            let right = estimate_node_output_rows_logical(right_node, stats, selectivity);
            let sel = factor_cost::join_selectivity(stats, n.hash_keys(), n.probe_keys())
                .unwrap_or(DEFAULT_JOIN_SELECTIVITY);
            factor_cost::join_output_rows(left, right, sel)
        }
        LeftJoin(_) => {
            let Some((left, _)) = logical_binary_inputs(node) else {
                return child_rows_of_logical(node, stats, selectivity);
            };
            estimate_node_output_rows_logical(left, stats, selectivity)
        }
        RightJoin(_) => {
            let Some((_, right)) = logical_binary_inputs(node) else {
                return child_rows_of_logical(node, stats, selectivity);
            };
            estimate_node_output_rows_logical(right, stats, selectivity)
        }
        CrossJoin(_) => {
            let Some((left, right)) = logical_binary_inputs(node) else {
                return child_rows_of_logical(node, stats, selectivity);
            };
            let left = estimate_node_output_rows_logical(left, stats, selectivity);
            let right = estimate_node_output_rows_logical(right, stats, selectivity);
            left.saturating_mul(right)
        }
        FullOuterJoin(_) => {
            let Some((left, right)) = logical_binary_inputs(node) else {
                return child_rows_of_logical(node, stats, selectivity);
            };
            let left = estimate_node_output_rows_logical(left, stats, selectivity);
            let right = estimate_node_output_rows_logical(right, stats, selectivity);
            left.saturating_add(right)
        }
        SemiJoin(_) => child_rows_of_logical(node, stats, selectivity),
        LogicalNodeEnum::WcoIntersect(n) => {
            let probe = estimate_node_output_rows_logical(n.probe_side(), stats, selectivity);
            let mut build_cards = Vec::with_capacity(n.num_builds());
            let mut smallest_build = u64::MAX;
            for build in n.build_sides() {
                let rows = estimate_node_output_rows_logical(build, stats, selectivity);
                smallest_build = smallest_build.min(rows);
                build_cards.push(rows);
            }
            if smallest_build == u64::MAX {
                probe
            } else {
                // Conservative probe filtering, mirroring the join-order
                // estimator's probe selectivity.
                let conservative = (probe as f64
                    * crate::planning::join_order::cardinality_estimator::NON_EQUALITY_PREDICATE_SELECTIVITY)
                    .max(1.0) as u64;
                // Independence-assumption upper bound over the vertex
                // domain: the largest known tag count, else the largest
                // observed side so the bound never divides by zero.
                let mut domain = probe.max(smallest_build).max(1);
                for tag in stats.manager().get_all_tags() {
                    domain = domain.max(stats.vertex_count(&tag)).max(1);
                }
                let mut numerator = probe as u128;
                for card in &build_cards {
                    numerator = numerator.saturating_mul(*card as u128);
                }
                let mut denominator: u128 = 1;
                for _ in 0..build_cards.len() {
                    denominator = denominator.saturating_mul(domain as u128);
                }
                let independent = (numerator / denominator.max(1)).min(u64::MAX as u128) as u64;
                conservative.min(independent).min(smallest_build).max(1)
            }
        }

        // ── Traversal operators (mirror the physical fanout arms so logical
        // decisions such as aggregate strategy price the same expansion) ──
        Expand(n) => {
            let fanout = stats_fanout(stats, &n.edge_types);
            child_rows_of_logical(node, stats, selectivity).saturating_mul(fanout)
        }
        ExpandAll(n) => {
            let fanout = stats_fanout(stats, &n.edge_types);
            child_rows_of_logical(node, stats, selectivity).saturating_mul(fanout)
        }
        Traverse(n) => {
            let fanout = stats_fanout(stats, &n.edge_types);
            child_rows_of_logical(node, stats, selectivity).saturating_mul(fanout)
        }
        BiExpand(n) => {
            let fanout = stats_fanout(stats, &n.edge_types);
            child_rows_of_logical(node, stats, selectivity).saturating_mul(fanout)
        }
        BiTraverse(n) => {
            let fanout = stats_fanout(stats, &n.edge_types);
            child_rows_of_logical(node, stats, selectivity).saturating_mul(fanout)
        }
        AppendVertices(_) => child_rows_of_logical(node, stats, selectivity)
            .saturating_mul(DEFAULT_NEIGHBORHOOD_FANOUT),
        PatternApply(_) | Apply(_) | CorrelatedApply(_) => {
            if let Some((left, right)) = logical_binary_inputs(node) {
                let left_rows = estimate_node_output_rows_logical(left, stats, selectivity);
                let right_rows = estimate_node_output_rows_logical(right, stats, selectivity);
                left_rows.saturating_mul(right_rows).max(1)
            } else {
                child_rows_of_logical(node, stats, selectivity)
            }
        }

        // ── Unsupported nodes: fall back to the child or a constant ──
        node => child_rows_of_logical(node, stats, selectivity),
    }
}

/// Estimate of the first child (pass-through semantics), or 1 for leaves.
fn child_rows_of_logical(
    node: &LogicalNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
) -> u64 {
    logical_first_child(node)
        .map(|child| estimate_node_output_rows_logical(child, stats, selectivity))
        .unwrap_or(1)
}

/// The first child of a logical node (convertible subset), if any.
fn logical_first_child(node: &LogicalNodeEnum) -> Option<&LogicalNodeEnum> {
    match node {
        LogicalNodeEnum::Project(n) => Some(n.input()),
        LogicalNodeEnum::Filter(n) => Some(n.input()),
        LogicalNodeEnum::Sort(n) => Some(n.input()),
        LogicalNodeEnum::Limit(n) => Some(n.input()),
        LogicalNodeEnum::Skip(n) => Some(n.input()),
        LogicalNodeEnum::TopN(n) => Some(n.input()),
        LogicalNodeEnum::Sample(n) => Some(n.input()),
        LogicalNodeEnum::Dedup(n) => Some(n.input()),
        LogicalNodeEnum::Aggregate(n) => Some(n.input()),
        LogicalNodeEnum::Window(n) => Some(n.input()),
        LogicalNodeEnum::InnerJoin(n) => Some(n.left_input()),
        LogicalNodeEnum::LeftJoin(n) => Some(n.left_input()),
        LogicalNodeEnum::RightJoin(n) => Some(n.left_input()),
        LogicalNodeEnum::CrossJoin(n) => Some(n.left_input()),
        LogicalNodeEnum::FullOuterJoin(n) => Some(n.left_input()),
        LogicalNodeEnum::SemiJoin(n) => Some(n.left_input()),
        LogicalNodeEnum::WcoIntersect(n) => Some(n.probe_side()),
        LogicalNodeEnum::GetVertices(n) => n.dependencies().first(),
        LogicalNodeEnum::GetNeighbors(n) => n.dependencies().first(),
        LogicalNodeEnum::Expand(n) => n.dependencies().first(),
        LogicalNodeEnum::ExpandAll(n) => n.dependencies().first(),
        LogicalNodeEnum::Traverse(n) => n.dependencies().first(),
        LogicalNodeEnum::AppendVertices(n) => n.dependencies().first(),
        LogicalNodeEnum::BiExpand(n) => Some(n.left_input()),
        LogicalNodeEnum::BiTraverse(n) => Some(n.left_input()),
        LogicalNodeEnum::PatternApply(n) => Some(n.left_input()),
        LogicalNodeEnum::CorrelatedApply(n) => Some(n.left_input()),
        LogicalNodeEnum::Apply(n) => Some(n.left_input()),
        LogicalNodeEnum::RollUpApply(n) => n.dependencies().first(),
        _ => None,
    }
}

/// The two inputs of a logical join node, if any.
///
/// `WcoIntersect` is N-way and intentionally returns `None` here; its
/// estimate is handled by the dedicated `WcoIntersect` arm using the
/// probe side as the driving cardinality.
fn logical_binary_inputs(node: &LogicalNodeEnum) -> Option<(&LogicalNodeEnum, &LogicalNodeEnum)> {
    match node {
        LogicalNodeEnum::InnerJoin(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::LeftJoin(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::RightJoin(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::CrossJoin(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::FullOuterJoin(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::SemiJoin(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::PatternApply(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::CorrelatedApply(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::Apply(n) => Some((n.left_input(), n.right_input())),
        LogicalNodeEnum::WcoIntersect(_) => None,
        _ => None,
    }
}

/// The tag referenced by a logical leaf scan (if any), for filter selectivity.
fn first_tag_of_logical_input(node: &LogicalNodeEnum) -> Option<String> {
    match node {
        LogicalNodeEnum::ScanVertices(n) => n.tag.clone(),
        _ => None,
    }
}

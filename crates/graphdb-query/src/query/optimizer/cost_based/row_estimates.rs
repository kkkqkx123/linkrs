//! Logical row-count estimation walker.
//!
//! Produces per-logical-node output row estimates for the cost-based phase.
//! The estimates are conservative heuristics driven by the optimizer's
//! statistics when available (tag vertex counts, edge counts, selectivity
//! estimates) and by fixed defaults otherwise. They are consumed by:
//!
//! - the TopN conversion decision (`topn_wiring`),
//! - the `estimated_rows` writeback pass (per-logical-node map).

use std::collections::HashMap;

use crate::query::optimizer::cost::SelectivityEstimator;
use crate::query::optimizer::stats::StatsView;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Fallback row count for scans whose statistics are unknown.
const UNKNOWN_SCAN_ROWS: u64 = 100;
/// Fallback row multiplier for neighborhood / expansion operators.
const DEFAULT_NEIGHBORHOOD_FANOUT: u64 = 10;
/// Default selectivity applied to a filter when the expression gives none.
const DEFAULT_FILTER_SELECTIVITY: f64 = 0.1;
/// Row multiplier applied to dedup.
const DEDUP_SELECTIVITY: f64 = 0.8;
/// Row multiplier applied to aggregation with group keys.
const AGGREGATE_SELECTIVITY: f64 = 0.1;

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
    use PlanNodeEnum::*;

    match node {
        // ── Leaf scans ──
        ScanVertices(n) => {
            let tag_rows = n
                .tag()
                .map(|tag| stats.vertex_count(tag))
                .unwrap_or(UNKNOWN_SCAN_ROWS);
            n.limit()
                .map(|limit| tag_rows.min(limit as u64))
                .unwrap_or(tag_rows)
        }
        ScanEdges(n) => {
            let edge_rows = n
                .edge_type()
                .map(|edge_type| stats.edge_count(&edge_type))
                .unwrap_or(UNKNOWN_SCAN_ROWS);
            n.limit()
                .map(|limit| edge_rows.min(limit as u64))
                .unwrap_or(edge_rows)
        }
        GetVertices(n) => n.limit().unwrap_or(1).max(1) as u64,
        GetEdges(n) => n.limit().unwrap_or(10).max(1) as u64,
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
            (n.limit().unwrap_or(fanout as i64).max(1) as u64).max(fanout)
        }
        IndexScan(n) => n.limit().unwrap_or(UNKNOWN_SCAN_ROWS as i64).max(1) as u64,
        Start(_) | Argument(_) => 1,

        // ── Single-input operations ──
        Filter(n) => {
            let input_rows = estimate_node_output_rows(n.input(), stats, selectivity);
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
        Project(_) | Sort(_) | Sample(_) | Window(_) => child_rows_of(node, stats, selectivity),
        TopN(n) => {
            let input_rows = estimate_node_output_rows(n.input(), stats, selectivity);
            input_rows.min(n.limit() as u64)
        }
        Limit(n) => {
            let input_rows = estimate_node_output_rows(n.input(), stats, selectivity);
            input_rows.min((n.offset() + n.count()) as u64)
        }
        Dedup(n) => {
            let input_rows = estimate_node_output_rows(n.input(), stats, selectivity);
            (input_rows as f64 * DEDUP_SELECTIVITY).max(1.0) as u64
        }
        Aggregate(n) => {
            let input_rows = estimate_node_output_rows(n.input(), stats, selectivity);
            if n.group_keys().is_empty() {
                1
            } else {
                (input_rows as f64 * AGGREGATE_SELECTIVITY).max(1.0) as u64
            }
        }

        // ── Binary operators ──
        // Joins multiply their inputs (semi-join keeps the left side).
        InnerJoin(_) | HashInnerJoin(_) | LeftJoin(_) | HashLeftJoin(_) | RightJoin(_)
        | CrossJoin(_) => {
            let children = node.children();
            if children.len() >= 2 {
                let left = estimate_node_output_rows(children[0], stats, selectivity);
                let right = estimate_node_output_rows(children[1], stats, selectivity);
                left.saturating_mul(right)
            } else {
                child_rows_of(node, stats, selectivity)
            }
        }
        FullOuterJoin(_) => {
            let children = node.children();
            if children.len() >= 2 {
                let left = estimate_node_output_rows(children[0], stats, selectivity);
                let right = estimate_node_output_rows(children[1], stats, selectivity);
                left.saturating_add(right)
            } else {
                child_rows_of(node, stats, selectivity)
            }
        }
        SemiJoin(_) => child_rows_of(node, stats, selectivity),
        Union(_) => {
            let mut total = 0u64;
            for child in node.children() {
                total = total.saturating_add(estimate_node_output_rows(child, stats, selectivity));
            }
            total
        }
        Minus(_) | Intersect(_) => {
            let mut smallest = u64::MAX;
            for child in node.children() {
                smallest = smallest.min(estimate_node_output_rows(child, stats, selectivity));
            }
            if smallest == u64::MAX {
                0
            } else {
                smallest
            }
        }

        // ── Traversal / apply operators ──
        Expand(_) | ExpandAll(_) | Traverse(_) | BiExpand(_) | BiTraverse(_)
        | AppendVertices(_) => {
            child_rows_of(node, stats, selectivity).saturating_mul(DEFAULT_NEIGHBORHOOD_FANOUT)
        }
        PatternApply(_) | Apply(_) | RollUpApply(_) => {
            let children = node.children();
            let mut total = 1u64;
            for child in children {
                total = total.saturating_mul(estimate_node_output_rows(child, stats, selectivity));
            }
            total
        }

        // ── Pass-through / control flow ──
        PassThrough(_) | Materialize(_) | Unwind(_) | DataCollect(_) | Remove(_) | Assign(_) => {
            child_rows_of(node, stats, selectivity)
        }

        // ── Leaf or unsupported nodes: fall back to the input or a constant ──
        node => child_rows_of(node, stats, selectivity),
    }
}

/// Estimate of the first child (pass-through semantics), or 1 for leaves.
fn child_rows_of(
    node: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
) -> u64 {
    node.children()
        .first()
        .map(|child| estimate_node_output_rows(child, stats, selectivity))
        .unwrap_or(1)
}

/// Collect output row estimates for every node in the plan, keyed by node id.
///
/// The map is attached to the optimized [`ExecutionPlan`](crate::query::planning::plan::ExecutionPlan)
/// and later written into physical operator specs by the `estimated_rows`
/// metadata pass (matched by `logical_node_id`).
pub fn collect_node_row_estimates(
    root: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
) -> HashMap<i64, u64> {
    let mut estimates = HashMap::new();
    collect_node_row_estimates_recursive(root, stats, selectivity, &mut estimates);
    estimates
}

fn collect_node_row_estimates_recursive(
    node: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
    estimates: &mut HashMap<i64, u64>,
) {
    for child in node.children() {
        collect_node_row_estimates_recursive(child, stats, selectivity, estimates);
    }
    let estimate = estimate_node_output_rows(node, stats, selectivity);
    estimates.insert(node.id(), estimate);
}

/// The tag referenced by a leaf scan (if any), for filter selectivity.
fn first_tag_of_input(node: &PlanNodeEnum) -> Option<String> {
    match node {
        PlanNodeEnum::ScanVertices(n) => n.tag().cloned(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use crate::core::value::Value;
    use crate::query::optimizer::stats::StatisticsManager;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::query::planning::plan::core::nodes::operation::sort_node::{
        LimitNode, SortItem, SortNode, TopNNode,
    };
    use std::sync::Arc;

    fn setup() -> (Arc<StatisticsManager>, SelectivityEstimator) {
        let manager = Arc::new(StatisticsManager::new());
        let selectivity = SelectivityEstimator::new(manager.clone());
        (manager, selectivity)
    }

    #[test]
    fn scan_without_stats_falls_back_to_constant() {
        let (manager, selectivity) = setup();
        let view = StatsView::new(&manager, Some("test"));
        let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "test"));
        assert_eq!(
            estimate_node_output_rows(&scan, &view, &selectivity),
            UNKNOWN_SCAN_ROWS
        );
    }

    #[test]
    fn filter_uses_expression_selectivity() {
        let (manager, selectivity) = setup();
        let view = StatsView::new(&manager, Some("test"));
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        let context = Arc::new(ExpressionAnalysisContext::new());
        let id = context
            .register_expression(ExpressionMeta::new(Expression::Literal(Value::Bool(true))));
        let condition = ContextualExpression::new(id, context);
        let filter = FilterNode::new(PlanNodeEnum::ScanVertices(scan), condition)
            .expect("filter should build");
        let estimate =
            estimate_node_output_rows(&PlanNodeEnum::Filter(filter), &view, &selectivity);
        assert!(estimate >= 1);
        assert!(estimate <= UNKNOWN_SCAN_ROWS);
    }

    #[test]
    fn limit_caps_input_rows() {
        let (manager, selectivity) = setup();
        let view = StatsView::new(&manager, Some("test"));
        let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "test"));
        let limit = LimitNode::new(scan, 5, 10).expect("limit should build");
        let estimate = estimate_node_output_rows(&PlanNodeEnum::Limit(limit), &view, &selectivity);
        assert_eq!(estimate, 15);
    }

    #[test]
    fn topn_caps_input_rows() {
        let (manager, selectivity) = setup();
        let view = StatsView::new(&manager, Some("test"));
        let mut tag_stats =
            crate::query::optimizer::stats::TagStatistics::new("person".to_string());
        tag_stats.vertex_count = 100;
        manager.update_tag_stats("test", tag_stats);
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        let sort = SortNode::new(
            PlanNodeEnum::ScanVertices(scan),
            vec![SortItem::column_asc("x".to_string())],
        )
        .expect("sort should build");
        let topn = TopNNode::new(
            PlanNodeEnum::Sort(sort),
            vec![SortItem::column_asc("x".to_string())],
            7,
        )
        .expect("topn should build");
        let estimate = estimate_node_output_rows(&PlanNodeEnum::TopN(topn), &view, &selectivity);
        assert_eq!(estimate, 7);
    }

    #[test]
    fn collect_returns_estimates_for_every_node() {
        let (manager, selectivity) = setup();
        let view = StatsView::new(&manager, Some("test"));
        let start = PlanNodeEnum::Start(
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
        );
        let sort = SortNode::new(start, vec![SortItem::column_asc("x".to_string())])
            .expect("sort should build");
        let limit = LimitNode::new(PlanNodeEnum::Sort(sort), 0, 5).expect("limit should build");
        let plan = PlanNodeEnum::Limit(limit);
        let estimates = collect_node_row_estimates(&plan, &view, &selectivity);
        assert_eq!(estimates.len(), 3);
        assert!(estimates.contains_key(&plan.id()));
    }
}

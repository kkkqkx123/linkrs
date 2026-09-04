//! Cost-based Sort + Limit → TopN rewriting.
//!
//! The heuristic phase already converts `Limit(offset=0) -> Sort` into
//! `TopN` unconditionally. This phase handles the residual patterns and
//! records the cost-based decision:
//!
//! - `Limit(offset>0, count) -> Sort`: rewritten into
//!   `Limit(offset>0, count) -> TopN(offset + count)` so the full sort is
//!   avoided while preserving the offset semantics.
//! - `Limit(offset=0, count) -> Sort` that survived the heuristic phase:
//!   converted with the cost-based decision.
//!
//! The decision (convert or keep) always uses [`SortEliminationOptimizer`]
//! so the choice is cost-driven rather than syntactic.

use crate::optimizer::cost::SelectivityEstimator;
use crate::optimizer::cost_based::row_estimates::{
    estimate_node_output_rows_corrected, estimate_node_output_rows_logical,
};
use crate::optimizer::cost_based::traversal::rewrite_children;
use crate::optimizer::cost_based::traversal_logical::rewrite_children_logical;
use crate::optimizer::cost_based::{
    SortContext, SortEliminationDecision, SortEliminationOptimizer,
};
use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::optimizer::stats::StatsView;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::core::nodes::operation::sort_node::TopNNode;
use crate::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;

/// Rewrite `Limit -> Sort` subtrees into `Limit -> TopN` when the
/// cost-based decision prefers it. Decisions are appended to `notes`.
pub fn rewrite_sort_with_limits(
    node: &PlanNodeEnum,
    optimizer: &SortEliminationOptimizer,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
    cardinality: &CardinalityFeedbackManager,
    notes: &mut Vec<String>,
) -> PlanNodeEnum {
    use PlanNodeEnum::*;

    // Try the Limit -> Sort pattern at this level first.
    if let Limit(limit) = node {
        if let Sort(sort) = limit.input() {
            let offset = limit.offset();
            let count = limit.count();
            if offset >= 0 && count > 0 {
                let input_rows = estimate_node_output_rows_corrected(
                    sort.input(),
                    stats,
                    selectivity,
                    cardinality,
                );
                let context =
                    SortContext::new(sort.clone(), input_rows).with_limit(count + offset.max(0));
                match optimizer.optimize_with_memory(&context, None) {
                    SortEliminationDecision::ConvertToTopN { reason, .. } => {
                        let topn = TopNNode::new(
                            sort.input().clone(),
                            sort.sort_items().to_vec(),
                            count + offset.max(0),
                        );
                        if let Ok(topn) = topn {
                            notes.push(format!(
                                "sort: convert sort+limit -> topn (limit={}, offset={}, reason={:?})",
                                count, offset, reason
                            ));
                            let mut new_limit = limit.clone();
                            new_limit.set_input(PlanNodeEnum::TopN(topn));
                            return Limit(new_limit);
                        }
                    }
                    SortEliminationDecision::KeepSort { reason, .. } => {
                        notes.push(format!(
                            "sort: keep sort+limit (limit={}, offset={}, reason={:?})",
                            count, offset, reason
                        ));
                    }
                }
            }
        }
    }

    // Recursively rewrite children.
    let mut closure = |child: &PlanNodeEnum| {
        rewrite_sort_with_limits(child, optimizer, stats, selectivity, cardinality, notes)
    };
    rewrite_children(node, &mut closure)
}

/// Rewrite `Limit -> Sort` subtrees into `Limit -> TopN` on the logical
/// tree when the cost-based decision prefers it.
///
/// Mirrors [`rewrite_sort_with_limits`] but operates on `LogicalNodeEnum`
/// so the CBO decision and rewrite live on the same tree; the physical
/// walker applies the corresponding rewrite to the executable root.
pub fn rewrite_sort_with_limits_logical(
    node: &LogicalNodeEnum,
    optimizer: &SortEliminationOptimizer,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
    notes: &mut Vec<String>,
) -> LogicalNodeEnum {
    use LogicalNodeEnum::*;

    // Try the Limit -> Sort pattern at this level first.
    if let Limit(limit) = node {
        if let Sort(sort) = limit.input() {
            let offset = limit.offset;
            let count = limit.count;
            if offset >= 0 && count > 0 {
                let input_rows =
                    estimate_node_output_rows_logical(sort.input(), stats, selectivity);
                let total = count + offset.max(0);
                if optimizer
                    .check_topn_conversion_cost(&sort.sort_items, total, input_rows)
                    .is_some()
                {
                    let topn =
                        crate::planning::plan::logical::logical_nodes::operation::LogicalTopNNode {
                            id: crate::planning::plan::core::node_id_generator::next_node_id(),
                            input: Some(Box::new(sort.input().clone())),
                            deps: vec![sort.input().clone()],
                            sort_items: sort.sort_items.clone(),
                            limit: total,
                            output_var: sort.output_var.clone(),
                            col_names: sort.col_names.clone(),
                            column_types: sort.column_types.clone(),
                        };
                    notes.push(format!(
                        "sort: convert sort+limit -> topn (limit={}, offset={})",
                        count, offset,
                    ));
                    let mut new_limit = limit.clone();
                    new_limit.set_input(LogicalNodeEnum::TopN(topn));
                    return Limit(new_limit);
                }
                notes.push(format!(
                    "sort: keep sort+limit (limit={}, offset={})",
                    count, offset,
                ));
            }
        }
    }

    // Recursively rewrite children.
    let mut closure = |child: &LogicalNodeEnum| {
        rewrite_sort_with_limits_logical(child, optimizer, stats, selectivity, notes)
    };
    rewrite_children_logical(node, &mut closure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::cost::CostCalculator;
    use crate::optimizer::stats::{StatisticsManager, TagStatistics};
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::operation::sort_node::{LimitNode, SortItem, SortNode};
    use std::sync::Arc;

    fn test_optimizer() -> SortEliminationOptimizer {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        SortEliminationOptimizer::new(cost_calculator)
    }

    fn setup(vertex_count: u64) -> (SortEliminationOptimizer, Arc<StatisticsManager>) {
        let manager = Arc::new(StatisticsManager::new());
        let mut tag_stats = TagStatistics::new("person".to_string());
        tag_stats.vertex_count = vertex_count;
        manager.update_tag_stats("test", tag_stats);
        (test_optimizer(), manager)
    }

    fn build_limit_sort(offset: i64, count: i64) -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        let sort = SortNode::new(
            PlanNodeEnum::ScanVertices(scan),
            vec![SortItem::column_asc("x".to_string())],
        )
        .expect("sort should build");
        let limit =
            LimitNode::new(PlanNodeEnum::Sort(sort), offset, count).expect("limit should build");
        PlanNodeEnum::Limit(limit)
    }

    #[test]
    fn converts_offset_limit_sort_to_topn() {
        let (optimizer, manager) = setup(100_000);
        let view = StatsView::new(&manager, Some("test"));
        let selectivity = SelectivityEstimator::new(manager.clone());
        let plan = build_limit_sort(10, 100);
        let mut notes = Vec::new();
        let rewritten = rewrite_sort_with_limits(
            &plan,
            &optimizer,
            &view,
            &selectivity,
            &CardinalityFeedbackManager::new(),
            &mut notes,
        );
        let PlanNodeEnum::Limit(limit) = &rewritten else {
            panic!("expected limit at root");
        };
        assert!(matches!(limit.input(), PlanNodeEnum::TopN(_)));
        if let PlanNodeEnum::TopN(topn) = limit.input() {
            assert_eq!(topn.limit(), 110);
        }
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("convert sort+limit -> topn"));
    }

    #[test]
    fn sort_without_limit_is_left_untouched() {
        let (optimizer, manager) = setup(100_000);
        let view = StatsView::new(&manager, Some("test"));
        let selectivity = SelectivityEstimator::new(manager.clone());
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        let sort = SortNode::new(
            PlanNodeEnum::ScanVertices(scan),
            vec![SortItem::column_asc("x".to_string())],
        )
        .expect("sort should build");
        let mut notes = Vec::new();
        let rewritten = rewrite_sort_with_limits(
            &PlanNodeEnum::Sort(sort),
            &optimizer,
            &view,
            &selectivity,
            &CardinalityFeedbackManager::new(),
            &mut notes,
        );
        assert!(matches!(rewritten, PlanNodeEnum::Sort(_)));
        assert!(notes.is_empty());
    }
}

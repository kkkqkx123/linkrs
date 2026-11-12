use crate::optimizer::stats::StatsView;
use crate::optimizer::stats::feedback::selectivity::SelectivitySnapshot;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::{PartitionSource, PartitionSpec, PartitionStrategy, PlanNodeEnum};

use super::super::config::{PartitioningDecision, PartitioningLayoutInfo};
use super::core::{fallback, PartitioningPlanner};
use super::helpers::{hop_fanout, split_range};

    /// Choose a partition layout for an anchored bounded traversal (E4):
    /// a linear chain of `ExpandAll` hops above the anchor vertex
    /// scan. The anchor is partitioned by vertex-id range; every partition
    /// runs the bounded traversal locally over its anchor subrange and the
    /// results are gathered globally. Recursive traversals and path
    /// algorithms are not chain-walkable and fall through to rejection.
    ///
    /// Returns `None` when the plan is not an anchored bounded traversal, so
    /// the caller can fall back to the generic graph-traversal rejection.
pub(crate) fn decide_anchored_traversal(
    planner: &PartitioningPlanner,
        root: &PlanNodeEnum,
        statistics: &StatsView,
        range: &std::ops::Range<i64>,
        layout: &PartitioningLayoutInfo,
        snapshot: &SelectivitySnapshot,
    ) -> Option<PartitioningDecision> {
        let mut chain = Vec::new();
        if !collect_anchored_chain(root, &mut chain) {
            return None;
        }
        // chain is root-first, ending with the anchor scan.
        let expand_count = chain
            .iter()
            .filter(|n| matches!(n, PlanNodeEnum::ExpandAll(_)))
            .count();
        if expand_count == 0 {
            return Some(fallback(
                "anchored traversal must contain at least one ExpandAll hop",
            ));
        }
        // C1: every ExpandAll hop must be de-materialized (id_only/count_only)
        // and filter-free, so each partition runs the full bounded chain over
        // its anchor subrange without changing row semantics. Other columnar
        // hops stay on the non-partitioned path by design.
        let hops_ok = chain.iter().all(|n| match n {
            PlanNodeEnum::ExpandAll(expand) => {
                expand.filter().is_none()
                    && expand.step_limit().unwrap_or(1) == 1
                    && (expand.id_only() || expand.count_only())
            }
            _ => true,
        });
        if !hops_ok {
            return Some(fallback(
                "anchored traversal hops must be filter-free and de-materialized (id_only/count_only); other columnar hops run unpartitioned",
            ));
        }
        let PlanNodeEnum::ScanVertices(scan) = chain[chain.len() - 1] else {
            return Some(fallback(
                "anchored traversal must end in a tagged vertex scan",
            ));
        };
        let Some(tag) = scan.tag() else {
            return Some(fallback(
                "anchor vertex scan has no tag statistics key",
            ));
        };
        let rows = statistics.vertex_count(tag);
        if rows == 0 {
            return Some(fallback(format!(
                "missing statistics for anchor tag '{tag}'; cannot estimate row count"
            )));
        }
        let threshold = planner.config().min_rows_per_partition.saturating_mul(2);
        if rows < threshold {
            return Some(fallback(format!(
                "estimated anchor rows ({rows}) are below the partition threshold"
            )));
        }
        let filtered = planner.estimate_filtered_rows(&chain, rows, tag, statistics, snapshot);
        // Each partition expands its own anchor slice, so the gate input is
        // the expanded work, not the anchor count: a small anchor with a
        // high fanout still pays off partitioned.
        let fanout = chain
            .iter()
            .filter_map(|node| match node {
                PlanNodeEnum::ExpandAll(expand) => {
                    Some(hop_fanout(statistics, expand.edge_types()))
                }
                _ => None,
            })
            .fold(1u64, |accumulated, hop| {
                accumulated.saturating_mul(hop).max(1)
            });
        let work = filtered.saturating_mul(fanout);
        if work < threshold {
            return Some(fallback(format!(
                "estimated expanded rows ({work}) are below the partition threshold"
            )));
        }

        let desired = planner.desired_partition_count(work);
        if let Some(reason) = planner.cost_gate_reason(work, work, desired) {
            return Some(fallback(reason));
        }
        let ranges = split_range(range, desired);
        let source = PartitionSource::VertexId {
            tag: tag.to_string(),
        };
        let layout_version =
            planner.layout_signature_with_layout(&source, &PartitionStrategy::Range, layout);
        match PartitionSpec::try_new(ranges, source, Some(layout_version)) {
            Ok(spec) => Some(PartitioningDecision {
                partition_spec: Some(spec),
                reason: format!(
                    "partitioned anchored traversal by '{tag}' vertex-id ranges into {desired} partitions"
                ),
            }),
            Err(error) => Some(fallback(format!(
                "invalid configured partition layout: {error}"
            ))),
        }
    }

    /// Walk a linear chain that ends in a tagged vertex scan. `Flatten` is
    /// row-preserving (it replays child rows), so it passes through like the
    /// other non-graph operators. Any other graph operator (Traverse, Loop,
    /// path algorithms, vertex-property fetches) fails the walk.
    fn collect_anchored_chain<'a>(
        node: &'a PlanNodeEnum,
        chain: &mut Vec<&'a PlanNodeEnum>,
    ) -> bool {
        chain.push(node);
        match node {
            PlanNodeEnum::ScanVertices(_) => true,
            PlanNodeEnum::Filter(filter) => collect_anchored_chain(filter.input(), chain),
            PlanNodeEnum::Project(project) => collect_anchored_chain(project.input(), chain),
            PlanNodeEnum::Flatten(flatten) => collect_anchored_chain(flatten.input(), chain),
            PlanNodeEnum::Limit(limit) => collect_anchored_chain(limit.input(), chain),
            PlanNodeEnum::Sort(sort) => collect_anchored_chain(sort.input(), chain),
            PlanNodeEnum::Aggregate(agg) => collect_anchored_chain(agg.input(), chain),
            PlanNodeEnum::TopN(topn) => collect_anchored_chain(topn.input(), chain),
            PlanNodeEnum::Dedup(dedup) => collect_anchored_chain(dedup.input(), chain),
            PlanNodeEnum::Window(window) => collect_anchored_chain(window.input(), chain),
            PlanNodeEnum::ExpandAll(expand_all) => {
                if let Some(input) = expand_all.inputs().first() {
                    collect_anchored_chain(input, chain)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Split a binary-op root into its two branch inputs when the op is a set
    /// op, cross join, or equality join on the partition key (vertex-id).
pub(crate) fn split_independent_branches(
        node: &PlanNodeEnum,
    ) -> Option<(&PlanNodeEnum, &PlanNodeEnum, &'static str)> {
        match node {
            PlanNodeEnum::Union(union) => Some((union.input(), union.union_input(), "union")),
            PlanNodeEnum::Minus(minus) => Some((minus.input(), minus.minus_input(), "minus")),
            PlanNodeEnum::Intersect(intersect) => {
                Some((intersect.input(), intersect.intersect_input(), "intersect"))
            }
            PlanNodeEnum::CrossJoin(join) => {
                Some((join.left_input(), join.right_input(), "cross join"))
            }
            PlanNodeEnum::InnerJoin(join) => {
                // E1b: allow equality join when the join key is a simple variable
                // reference (i.e. the vertex-id partition key).  Complex join keys
                // (expressions, composite keys) are rejected for now.
                if equality_join_keys_are_simple(join.hash_keys(), join.probe_keys()) {
                    Some((join.left_input(), join.right_input(), "equality join"))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Whether a join's hash/probe keys are each a single simple variable
    /// reference (the only key shape the partitioned join path supports).
pub(crate) fn equality_join_keys_are_simple(
        hash_keys: &[linkrs_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[linkrs_core::types::expr::contextual::ContextualExpression],
    ) -> bool {
        hash_keys.len() == 1
            && probe_keys.len() == 1
            && hash_keys
                .first()
                .and_then(|k| k.expression())
                .is_some_and(|m| {
                    matches!(m.inner(), linkrs_core::types::expr::Expression::Variable(_))
                })
            && probe_keys
                .first()
                .and_then(|k| k.expression())
                .is_some_and(|m| {
                    matches!(m.inner(), linkrs_core::types::expr::Expression::Variable(_))
                })
    }

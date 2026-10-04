//! Learned cardinality feedback applied to raw row estimates.

use crate::optimizer::cost::SelectivityEstimator;
use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::optimizer::stats::StatsView;
use crate::planning::plan::PlanNodeEnum;

use super::physical::estimate_node_output_rows;

/// Normalized shape key of a plan node's output cardinality.
///
/// Plan-side shape key; delegates to the shared
/// `feedback::cardinality::format_shape_key` so plan and executor keys
/// cannot drift. Returns `None` for nodes whose cardinality is derived
/// (pass-through operators and filters — filters are corrected per
/// predicate by the selectivity feedback loop).
pub(super) fn cardinality_shape_key(space: Option<&str>, node: &PlanNodeEnum) -> Option<String> {
    use crate::optimizer::stats::feedback::cardinality::format_shape_key;
    use PlanNodeEnum::*;
    let key = |kind: &str, discriminator: Option<&str>| {
        Some(format_shape_key(space, kind, discriminator))
    };
    match node {
        ScanVertices(n) => key("ScanVertices", n.tag().map(String::as_str)),
        ScanEdges(n) => key("ScanEdges", n.edge_type().as_deref()),
        GetVertices(_) => key("GetVertices", None),
        GetEdges(n) => key("GetEdges", Some(n.edge_type())),
        GetNeighbors(n) => key("GetNeighbors", Some(n.direction())),
        IndexScan(n) => key("IndexScan", Some(n.index_name())),
        Expand(n) => key("Expand", Some(&n.edge_types().join(","))),
        ExpandAll(n) => key("ExpandAll", Some(&n.edge_types().join(","))),
        Traverse(n) => key("Traverse", Some(&n.edge_types().join(","))),
        BiExpand(n) => key("BiExpand", Some(&n.edge_types().join(","))),
        BiTraverse(n) => key("BiTraverse", Some(&n.edge_types().join(","))),
        AppendVertices(n) => key("AppendVertices", Some(n.vertex_tag())),
        PatternApply(_) => key("PatternApply", None),
        Apply(_) => key("Apply", None),
        RollUpApply(_) => key("RollUpApply", None),
        InnerJoin(_) => key("InnerJoin", None),
        LeftJoin(_) => key("LeftJoin", None),
        RightJoin(_) => key("RightJoin", None),
        FullOuterJoin(_) => key("FullOuterJoin", None),
        CrossJoin(_) => key("CrossJoin", None),
        SemiJoin(_) => key("SemiJoin", None),
        Union(_) => key("Union", None),
        Minus(_) => key("Minus", None),
        Intersect(_) => key("Intersect", None),
        Aggregate(_) => key("Aggregate", None),
        // Pass-through / derived operators and filters carry no shape key.
        _ => None,
    }
}

/// Apply the learned cardinality correction for `node`, if registered.
///
/// The raw estimate is registered as the feedback baseline on first sight
/// so execution feedback can correct it later; repeat visits refresh the
/// baseline without resetting learned factors.
pub(super) fn corrected_rows(
    node: &PlanNodeEnum,
    raw: u64,
    space: Option<&str>,
    cardinality: Option<&CardinalityFeedbackManager>,
) -> u64 {
    let Some(manager) = cardinality else {
        return raw;
    };
    let Some(key) = cardinality_shape_key(space, node) else {
        return raw;
    };
    manager.register_key(key.clone(), raw as f64);
    manager.refresh_estimated(&key, raw as f64);
    match manager.corrected_rows(&key) {
        Some(corrected) => (corrected.round().max(1.0)) as u64,
        None => raw,
    }
}

/// Register raw per-node estimates as feedback baselines for a whole plan.
///
/// Called once per optimization so every shape-keyed operator has a
/// baseline before execution feedback arrives. Idempotent: existing keys
/// keep their learned factors while baselines track fresh statistics.
pub(crate) fn register_plan_estimates(
    manager: &CardinalityFeedbackManager,
    root: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        for child in node.children() {
            stack.push(child);
        }
        if let Some(key) = cardinality_shape_key(stats.space(), node) {
            let raw = estimate_node_output_rows(node, stats, selectivity);
            manager.register_key(key.clone(), raw as f64);
            manager.refresh_estimated(&key, raw as f64);
        }
    }
}

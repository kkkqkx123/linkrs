//! Shape gates for partition-local scalar counting.
//!
//! The gates only admit chains whose per-partition output is a single integer
//! count, so the existing exchange and `SUM(_expand_count)` merge remain the
//! only numerical contract. Storage-level evidence is still checked at
//! execution time for the count frontier.

use crate::parser::ast::pattern::PathSemantic;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::PlanNodeEnum;

use super::ordered::{is_count_consumer, is_true_filter};

/// Whether `root` is a pure count over a count-only expand frontier whose
/// partial results can be produced independently per seed shard.
pub(crate) fn is_parallel_count_frontier(root: &PlanNodeEnum) -> bool {
    count_frontier_shape(root).is_some()
}

/// Whether `root` is narrow enough for shard-local metadata counting.
///
/// Storage ownership and layout identity are separate proofs and are not
/// inspected here; this gate only rules out plans that could never be served
/// by a single scalar per partition.
pub(crate) fn is_partition_metadata_count(root: &PlanNodeEnum) -> bool {
    count_frontier_shape(root).is_some()
}

fn count_frontier_shape(root: &PlanNodeEnum) -> Option<()> {
    let PlanNodeEnum::Aggregate(aggregate) = root else {
        return None;
    };
    if !is_count_consumer(aggregate, false) {
        return None;
    }

    // The count rewrite follows Projects only: a surviving Filter would make
    // `COUNT` observe the scalar count column instead of expand rows.
    let mut node = aggregate.input();
    while let PlanNodeEnum::Project(project) = node {
        node = project.input();
    }
    let PlanNodeEnum::ExpandAll(expand) = node else {
        return None;
    };

    if !expand.count_only()
        || expand.filter().is_some()
        || !expand.src_vids().is_empty()
        || expand.sample()
        || expand.any_edge_type()
        || expand.edge_types().len() != 1
        || !matches!(expand.path_semantic(), None | Some(PathSemantic::Walk))
        || expand.include_empty_paths()
        || !expand.closed_loop()
        || !matches!(expand.direction(), "out" | "in")
        || expand
            .edge_required_props()
            .is_some_and(|props| !props.is_empty())
        || expand
            .dst_required_props()
            .is_some_and(|props| !props.is_empty())
    {
        return None;
    }
    // Single-step counts already fold into the storage metadata count.
    // Partitioning them would replace a constant-time answer with N scans,
    // so the frontier only covers fused multi-step shapes.
    match expand.step_limits() {
        Some(limits) if matches!(limits.as_slice(), [2] | [1, 2]) => {}
        _ => return None,
    }

    let mut source = expand.inputs().first()?;
    loop {
        source = match source {
            PlanNodeEnum::Project(project) => project.input(),
            PlanNodeEnum::Filter(filter) if is_true_filter(filter) => filter.input(),
            _ => break,
        };
    }
    match source {
        PlanNodeEnum::ScanVertices(scan) if scan.tag().is_some() => Some(()),
        _ => None,
    }
}

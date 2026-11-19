//! Shape gate for the ordered parallel expand front end.
//!
//! The gate admits only the de-materialized single-hop chains whose typed
//! chunks survive the morsel pool and whose blocking consumer has a
//! deterministic global merge. Every other shape stays on the serial path.

use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::PlanNodeEnum;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::Value;

/// Whether `root` is a narrow rowless expand chain safe for the parallel
/// front end: one de-materialized hop, a count / group-count / ordered
/// distinct consumer, and no order-dependent operator below that consumer.
pub(crate) fn is_ordered_parallel_expand(root: &PlanNodeEnum) -> bool {
    let mut node: &PlanNodeEnum = root;
    let mut ordered = false;
    loop {
        node = match node {
            PlanNodeEnum::Limit(limit) => limit.input(),
            PlanNodeEnum::TopN(topn) => {
                ordered = true;
                topn.input()
            }
            PlanNodeEnum::Sort(sort) => {
                ordered = true;
                sort.input()
            }
            PlanNodeEnum::Filter(filter) if is_true_filter(filter) => filter.input(),
            _ => break,
        };
    }

    let (mut node, needs_order) = match node {
        PlanNodeEnum::Aggregate(aggregate) if is_count_consumer(aggregate) => {
            (aggregate.input(), false)
        }
        PlanNodeEnum::Dedup(dedup) if dedup.col_names().len() == 1 => (dedup.input(), true),
        _ => return false,
    };
    if needs_order && !ordered {
        return false;
    }

    node = skip_true_filters(node);
    let PlanNodeEnum::Project(project) = node else {
        return false;
    };
    if !project.emit_rowless() {
        return false;
    }

    let node = skip_true_filters(project.input());
    let PlanNodeEnum::ExpandAll(expand) = node else {
        return false;
    };
    // `count_only` hops already take the metadata/scalar count fast path;
    // partitioning them would replace a constant-time answer with N scans.
    if expand.count_only()
        || expand.filter().is_some()
        || expand.step_limit().unwrap_or(0) != 1
        || !expand.skip_rows()
    {
        return false;
    }
    let Some(source) = expand.inputs().first() else {
        return false;
    };
    matches!(source, PlanNodeEnum::ScanVertices(_))
}

fn skip_true_filters(mut node: &PlanNodeEnum) -> &PlanNodeEnum {
    while let PlanNodeEnum::Filter(filter) = node {
        if !is_true_filter(filter) {
            break;
        }
        node = filter.input();
    }
    node
}

fn is_count_consumer(aggregate: &AggregateNode) -> bool {
    let functions = aggregate.aggregation_functions();
    if functions.is_empty()
        || !functions
            .iter()
            .all(|function| matches!(function, AggregateFunction::Count))
        || aggregate
            .aggregation_distinct()
            .iter()
            .any(|distinct| *distinct)
        || aggregate
            .aggregation_filters()
            .iter()
            .any(|filter| filter.is_some())
        || aggregate.grouping_sets().iter().any(|set| !set.is_empty())
    {
        return false;
    }
    let args = aggregate.aggregation_args();
    if args.len() != functions.len()
        || !args.iter().all(|arg| {
            arg.is_empty()
                || (arg.len() == 1
                    && matches!(
                        arg.first(),
                        Some(linkrs_core::types::expr::Expression::Variable(_))
                    ))
        })
    {
        return false;
    }
    if aggregate.group_keys().is_empty() {
        functions.len() == 1
    } else {
        aggregate.group_keys().len() == 1 && functions.len() == 1
    }
}

fn is_true_filter(filter: &FilterNode) -> bool {
    filter
        .condition()
        .get_expression()
        .is_some_and(|expression| {
            matches!(
                expression,
                linkrs_core::types::expr::Expression::Literal(Value::Bool(true))
            )
        })
}

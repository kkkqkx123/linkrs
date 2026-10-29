//! Logical-plan variants of the PatternApply → SemiJoin unnesting.

use crate::planning::plan::logical::LogicalNodeEnum;

/// Structural conversion: logical `PatternApply` → `SemiJoin` / anti `SemiJoin`.
///
/// Mirrors [`super::optimizer::SubqueryUnnestingOptimizer::build_semi_join_from_pattern_apply`]
/// on the logical tree: the apply keeps left rows with (EXISTS) or without
/// (NOT EXISTS) a matching right row, which is exactly semi-join semantics.
pub fn build_semi_join_from_pattern_apply_logical(
    pattern_apply: crate::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode,
) -> LogicalNodeEnum {
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode;

    let left = (*pattern_apply.left).clone();
    let right = (*pattern_apply.right).clone();
    let col_names = left.col_names().to_vec();
    LogicalNodeEnum::SemiJoin(LogicalSemiJoinNode {
        id: next_node_id(),
        left: Box::new(left),
        right: Box::new(right),
        hash_keys: pattern_apply.hash_keys.clone(),
        probe_keys: pattern_apply.probe_keys.clone(),
        join_condition: None,
        anti: pattern_apply.is_anti_predicate,
        output_var: pattern_apply.output_var.clone(),
        col_names,
        column_types: vec![],
    })
}

/// Walk a logical tree and rewrite eligible `PatternApply` → `SemiJoin`.
///
/// Only provably-safe simple shapes are converted without statistics (a
/// single-table scan wrapped in filters, projections, or a constant limit;
/// no aggregation), mirroring the stat-free heuristic gate. Complex
/// subqueries keep the apply shape for the cost-based physical path.
pub fn unnest_pattern_applies_logical(
    node: &LogicalNodeEnum,
    notes: &mut Vec<String>,
) -> LogicalNodeEnum {
    use crate::optimizer::cost_based::traversal_logical::rewrite_children_logical;

    let rewritten = rewrite_children_logical(node, &mut |child| {
        unnest_pattern_applies_logical(child, notes)
    });
    if let LogicalNodeEnum::PatternApply(apply) = &rewritten {
        if is_simple_logical_subquery_shape(apply.right.as_ref()) {
            let kind = if apply.is_anti_predicate {
                "anti"
            } else {
                "semi"
            };
            notes.push(format!(
                "unnest pattern_apply -> {}_join (simple shape)",
                kind
            ));
            return build_semi_join_from_pattern_apply_logical(apply.clone());
        }
    }
    rewritten
}

/// Whether a logical subquery input has a provably-safe simple shape:
///
/// a single-table scan wrapped in equality filters, projections, skip, or a
/// constant limit. Aggregation or any other operator keeps the apply shape.
/// Filter conditions use the shared equality gate from `super::types` so the
/// logical and physical shape checks cannot drift.
fn is_simple_logical_subquery_shape(node: &LogicalNodeEnum) -> bool {
    match node {
        LogicalNodeEnum::ScanVertices(_)
        | LogicalNodeEnum::ScanEdges(_)
        | LogicalNodeEnum::GetVertices(_)
        | LogicalNodeEnum::GetEdges(_)
        | LogicalNodeEnum::GetNeighbors(_)
        | LogicalNodeEnum::Start(_)
        | LogicalNodeEnum::Argument(_)
        | LogicalNodeEnum::PassThrough(_) => true,
        LogicalNodeEnum::Filter(n) => {
            let condition_ok = n
                .condition
                .expression()
                .is_none_or(|meta| super::types::is_simple_equality_condition(meta.inner()));
            condition_ok
                && n.input
                    .as_deref()
                    .is_some_and(is_simple_logical_subquery_shape)
        }
        LogicalNodeEnum::Project(n) => n
            .input
            .as_deref()
            .is_some_and(is_simple_logical_subquery_shape),
        LogicalNodeEnum::Limit(n) => n
            .input
            .as_deref()
            .is_some_and(is_simple_logical_subquery_shape),
        LogicalNodeEnum::Skip(n) => n
            .input
            .as_deref()
            .is_some_and(is_simple_logical_subquery_shape),
        _ => false,
    }
}

//! Shared child traversal for logical cost-based walkers.
//!
//! `rewrite_children_logical` clones a logical node, rewrites every child
//! with the supplied closure, and returns the rebuilt node. It mirrors the
//! physical `rewrite_children` (`cost_based::traversal`) but operates on
//! `LogicalNodeEnum`, so logical decision walkers only implement their
//! per-node decision and delegate recursion here.
//!
//! The logical tree is pure (no physical operators), so only the operator
//! categories produced by `conversion::convert_plan` are traversed; all
//! other variants are returned unchanged.

use crate::query::planning::plan::logical::logical_node_traits::{
    LogicalMultipleInputNode, LogicalSingleInputNode,
};
use crate::query::planning::plan::logical::LogicalNodeEnum;

/// Rewrite all children of `node` with `f` and return the rebuilt node.
///
/// Node types without traversable children (leaves, control flow, search
/// nodes) are returned unchanged.
pub fn rewrite_children_logical(
    node: &LogicalNodeEnum,
    f: &mut impl FnMut(&LogicalNodeEnum) -> LogicalNodeEnum,
) -> LogicalNodeEnum {
    use LogicalNodeEnum::*;

    macro_rules! rewrite_single {
        ($n:expr) => {{
            let mut cloned = $n.clone();
            let new_input = f(cloned.input());
            cloned.set_input(new_input);
            cloned
        }};
    }
    macro_rules! rewrite_binary {
        ($n:expr) => {{
            let mut cloned = $n.clone();
            let new_left = f(cloned.left_input());
            let new_right = f(cloned.right_input());
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            cloned
        }};
    }
    macro_rules! rewrite_multiple {
        ($n:expr) => {{
            let mut cloned = $n.clone();
            for child in cloned.inputs_mut() {
                *child = f(child);
            }
            cloned
        }};
    }

    match node {
        // Single-input operation nodes
        Project(n) => Project(rewrite_single!(n)),
        Filter(n) => Filter(rewrite_single!(n)),
        Sort(n) => Sort(rewrite_single!(n)),
        Limit(n) => Limit(rewrite_single!(n)),
        TopN(n) => TopN(rewrite_single!(n)),
        Sample(n) => Sample(rewrite_single!(n)),
        Dedup(n) => Dedup(rewrite_single!(n)),
        Aggregate(n) => Aggregate(rewrite_single!(n)),
        Window(n) => Window(rewrite_single!(n)),

        // Binary join nodes
        InnerJoin(n) => InnerJoin(rewrite_binary!(n)),
        LeftJoin(n) => LeftJoin(rewrite_binary!(n)),
        RightJoin(n) => RightJoin(rewrite_binary!(n)),
        CrossJoin(n) => CrossJoin(rewrite_binary!(n)),
        FullOuterJoin(n) => FullOuterJoin(rewrite_binary!(n)),
        SemiJoin(n) => SemiJoin(rewrite_binary!(n)),

        // Multiple-input access nodes
        GetVertices(n) => GetVertices(rewrite_multiple!(n)),
        GetNeighbors(n) => GetNeighbors(rewrite_multiple!(n)),

        // Leaf / unsupported nodes: return unchanged.
        _ => node.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::logical::logical_nodes::access::{
        LogicalGetNeighborsNode, LogicalScanVerticesNode,
    };
    use crate::query::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;

    fn scan_node(id: i64, tag: &str) -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id,
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some(tag.to_string()),
            expression: None,
            limit: None,
            projected_properties: vec![],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    fn filter_over(node: LogicalNodeEnum) -> LogicalNodeEnum {
        use crate::core::types::expr::ExpressionId;

        let condition = crate::core::types::expr::ContextualExpression::new(
            ExpressionId(0),
            std::sync::Arc::new(
                crate::core::types::expr::expression_context::ExpressionAnalysisContext::new(),
            ),
        );
        LogicalNodeEnum::Filter(LogicalFilterNode {
            id: 100,
            input: Some(Box::new(node.clone())),
            deps: vec![node],
            condition,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    #[test]
    fn rewrite_children_logical_rewrites_single_input() {
        let root = filter_over(scan_node(1, "person"));
        let mut visited = 0usize;
        let rewritten = rewrite_children_logical(&root, &mut |child: &LogicalNodeEnum| {
            visited += 1;
            child.clone()
        });
        assert_eq!(visited, 1);
        assert!(matches!(rewritten, LogicalNodeEnum::Filter(_)));
    }

    #[test]
    fn rewrite_children_logical_rewrites_multiple_inputs() {
        let neighbors = LogicalNodeEnum::GetNeighbors(LogicalGetNeighborsNode {
            id: 2,
            space_id: 1,
            src_vids: "v".to_string(),
            edge_types: vec![],
            direction: "BOTH".to_string(),
            edge_props: vec![],
            tag_props: vec![],
            expression: None,
            dedup: false,
            limit: None,
            projected_properties: vec![],
            deps: vec![scan_node(3, "a"), scan_node(4, "b")],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        let mut visited = 0usize;
        let rewritten = rewrite_children_logical(&neighbors, &mut |child| {
            visited += 1;
            child.clone()
        });
        assert_eq!(visited, 2);
        assert!(matches!(rewritten, LogicalNodeEnum::GetNeighbors(_)));
    }

    #[test]
    fn rewrite_children_logical_leaves_leaf_nodes_unchanged() {
        let scan = scan_node(1, "person");
        let rewritten = rewrite_children_logical(&scan, &mut |child| child.clone());
        assert!(matches!(rewritten, LogicalNodeEnum::ScanVertices(_)));
    }
}

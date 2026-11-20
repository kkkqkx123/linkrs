//! Rules for pushing the TopN results down to the index scanning operation
//!
//! This rule identifies the TopN -> IndexScan mode.
//! Integrate the limitations of TopN and the sorting information into the IndexScan operation.

use crate::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::optimizer::heuristic::result::TransformResult;
use crate::planning::plan::core::nodes::access::{IndexScanNode, OrderByItem};
use crate::planning::plan::core::nodes::operation::sort_node::TopNNode;

define_rewrite_pushdown_rule! {
    /// Rules for pushing the TopN results down to the index scanning operation
    ///
    /// # Conversion example
    ///
    /// Before:
    /// ```text
    ///   TopN(count=100, sort_items=[age DESC])
    ///       |
    ///   IndexScan
    /// ```
    ///
    /// After:
    /// ```text
    ///   TopN(count=100, sort_items=[age DESC])
    ///       |
    ///   IndexScan(limit=100, order_by=[age DESC])
    /// ```
    ///
    /// # Applicable Conditions
    ///
    /// The current node is one of the TopN nodes.
    /// The child node is an IndexScan node.
    /// The TopN nodes each have only one child node.
    /// The `IndexScan` has not yet had a `limit` set, or the new `limit` is smaller than the existing `limit`.
    /// The `order_by` parameter has not been set for `IndexScan`.
    name: PushTopNDownIndexScanRule,
    parent_node: TopN,
    child_node: IndexScan,
    apply: |_ctx, topn_node: &TopNNode, index_scan_node: &IndexScanNode| {
        // Calculate the total number of rows that need to be retrieved (for TopN, there is no offset, only a limit).
        let limit_rows = topn_node.limit();
        if limit_rows <= 0 {
            return Ok(None::<TransformResult>);
        }

        // A residual filter between the index and the sort would break the
        // truncation: rejected rows must not consume the pushed limit.
        if index_scan_node.filter().is_some() {
            return Ok(None::<TransformResult>);
        }
        let Some(proof) = index_scan_node.order_proof() else {
            return Ok(None::<TransformResult>);
        };

        // Check whether there is a more stringent limit already in place for IndexScan.
        if let Some(existing_limit) = index_scan_node.limit() {
            if limit_rows >= existing_limit {
                // The existing restrictions are already more stringent; no conversion is required.
                return Ok(None::<TransformResult>);
            }
        }

        // Check whether IndexScan already has a sorting condition.
        if !index_scan_node.order_by().is_empty() {
            // Sorting criteria are already in place to prevent duplicate entries from being generated.
            return Ok(None::<TransformResult>);
        }

        // Convert the sorting items of TopN into the OrderByItem of IndexScan.
        // Only simple column references can be pushed down to IndexScan.
        // If any sort item is a complex expression (function call, etc.), skip the pushdown.
        let sort_items = topn_node.sort_items();
        if sort_items.len() != 1 {
            return Ok(None::<TransformResult>);
        }
        let item = &sort_items[0];
        let (Some(col_name), true) = (item.column_name(), item.direction == proof.direction) else {
            return Ok(None::<TransformResult>);
        };
        if col_name != proof.column {
            return Ok(None::<TransformResult>);
        }
        let order_by_items = vec![OrderByItem::new(col_name.to_string(), item.direction)];

        // Create a new IndexScan node and set the `limit` and `order_by` parameters.
        let mut new_index_scan = index_scan_node.clone();
        new_index_scan.set_offset(0);
        new_index_scan.set_limit(limit_rows);
        new_index_scan.set_order_by(order_by_items);

        // Create the translation result.
        let mut result = TransformResult::new();
        result.erase_all = true;
        result.add_new_node(PlanNodeEnum::IndexScan(new_index_scan));

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::heuristic::context::RewriteContext;
    use crate::optimizer::heuristic::rule::RewriteRule;
    use crate::planning::plan::core::nodes::access::{IndexOrderProof, ScanType};
    use crate::planning::plan::core::nodes::operation::sort_node::SortItem;
    use crate::planning::plan::PlanNodeEnum;
    use linkrs_core::types::expr::Expression;
    use linkrs_core::types::graph_schema::OrderDirection;

    #[test]
    fn test_rule_name() {
        let rule = PushTopNDownIndexScanRule::new();
        assert_eq!(rule.name(), "PushTopNDownIndexScanRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushTopNDownIndexScanRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    fn scan_with_proof(direction: OrderDirection) -> IndexScanNode {
        let mut scan = IndexScanNode::new(
            1,
            1,
            1,
            "idx_age".to_string(),
            "Person".to_string(),
            ScanType::Range,
        );
        scan.set_order_proof(IndexOrderProof {
            column: "age".to_string(),
            direction,
            reason: "test proof".to_string(),
        });
        scan
    }

    #[test]
    fn requires_matching_order_proof() {
        let rule = PushTopNDownIndexScanRule::new();
        let mut ctx = RewriteContext::new();
        let unproven = IndexScanNode::new(
            1,
            1,
            1,
            "idx_age".to_string(),
            "Person".to_string(),
            ScanType::Range,
        );
        let topn = TopNNode::new(
            PlanNodeEnum::IndexScan(unproven),
            vec![SortItem::asc(Expression::variable("age".to_string()))],
            10,
        )
        .expect("topn");
        assert!(rule
            .apply(&mut ctx, &PlanNodeEnum::TopN(topn))
            .expect("apply")
            .is_none());

        let scan = scan_with_proof(OrderDirection::Desc);
        let topn = TopNNode::new(
            PlanNodeEnum::IndexScan(scan),
            vec![SortItem::asc(Expression::variable("age".to_string()))],
            10,
        )
        .expect("topn");
        assert!(rule
            .apply(&mut ctx, &PlanNodeEnum::TopN(topn))
            .expect("apply")
            .is_none());
    }

    #[test]
    fn pushes_matching_proven_order() {
        let rule = PushTopNDownIndexScanRule::new();
        let mut ctx = RewriteContext::new();
        let scan = scan_with_proof(OrderDirection::Asc);
        let topn = TopNNode::new(
            PlanNodeEnum::IndexScan(scan),
            vec![SortItem::asc(Expression::variable("age".to_string()))],
            10,
        )
        .expect("topn");
        let result = rule
            .apply(&mut ctx, &PlanNodeEnum::TopN(topn))
            .expect("apply")
            .expect("pushed");
        let PlanNodeEnum::IndexScan(pushed) = &result.new_nodes[0] else {
            panic!("expected index scan");
        };
        assert_eq!(pushed.limit(), Some(10));
        assert_eq!(pushed.order_by().len(), 1);
    }
}

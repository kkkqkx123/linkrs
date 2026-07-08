//! Rules for pushing the TopN results down to the index scanning operation
//!
//! This rule identifies the TopN -> IndexScan mode.
//! Integrate the limitations of TopN and the sorting information into the IndexScan operation.

use crate::query::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::query::optimizer::heuristic::result::TransformResult;
use crate::query::planning::plan::core::nodes::access::{IndexScanNode, OrderByItem};
use crate::query::planning::plan::core::nodes::operation::sort_node::TopNNode;

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
        let mut order_by_items = Vec::with_capacity(topn_node.sort_items().len());
        for item in topn_node.sort_items() {
            match item.column_name() {
                Some(col_name) => {
                    order_by_items.push(OrderByItem::new(col_name.to_string(), item.direction));
                }
                None => {
                    // Complex expression - cannot push down to index scan
                    return Ok(None::<TransformResult>);
                }
            }
        }

        // Create a new IndexScan node and set the `limit` and `order_by` parameters.
        let mut new_index_scan = index_scan_node.clone();
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
    use crate::query::optimizer::heuristic::rule::RewriteRule;

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
}

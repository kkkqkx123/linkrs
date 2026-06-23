//! The rule for pushing down the LIMIT clause to the index scanning operation
//!
//! This rule identifies the mode in which the operation switches from “Limit” to “IndexScan”.
//! And integrate the LIMIT value into the IndexScan operation.

use crate::query::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::query::optimizer::heuristic::result::TransformResult;
use crate::query::planning::plan::core::nodes::access::IndexScanNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;

define_rewrite_pushdown_rule! {
    /// The rule for pushing the LIMIT clause down to the index scanning operation
    ///
    /// # Conversion example
    ///
    /// Before:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   IndexScan
    /// ```
    ///
    /// After:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   IndexScan(limit=110)
    /// ```
    ///
    /// # Applicable Conditions
    ///
    /// The current node is a Limit node.
    /// The child node is an IndexScan node.
    /// The Limit node has only one child node.
    /// The `IndexScan` has not had its `limit` set yet, or the new `limit` is smaller than the existing `limit`.
    name: PushLimitDownIndexScanRule,
    parent_node: Limit,
    child_node: IndexScan,
    apply: |_ctx, limit_node: &LimitNode, index_scan_node: &IndexScanNode| {
        // Calculate the total number of rows that need to be retrieved (offset + count).
        let limit_rows = limit_node.offset() + limit_node.count();

        // Check whether there is a more stringent limit for IndexScan already in place.
        if let Some(existing_limit) = index_scan_node.limit() {
            if limit_rows >= existing_limit {
                // The existing restrictions are already more stringent; no conversion is required.
                return Ok(None::<TransformResult>);
            }
        }

        // Create a new IndexScan node and set the limit.
        let mut new_index_scan = index_scan_node.clone();
        new_index_scan.set_limit(limit_rows);

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
        let rule = PushLimitDownIndexScanRule::new();
        assert_eq!(rule.name(), "PushLimitDownIndexScanRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushLimitDownIndexScanRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

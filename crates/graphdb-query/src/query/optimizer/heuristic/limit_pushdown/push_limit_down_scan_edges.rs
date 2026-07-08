//! The rule that pushes the LIMIT statement down to the operation level of the scanning process
//!
//! This rule identifies the “Limit -> ScanEdges” mode.
//! And integrate the LIMIT value into the ScanEdges operation.

use crate::query::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::query::optimizer::heuristic::result::TransformResult;
use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanEdgesNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;

define_rewrite_pushdown_rule! {
    /// The rule that pushes the LIMIT statement down to the operation level of the scan process
    ///
    /// # Translation example
    ///
    /// Before:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   ScanEdges
    /// ```
    ///
    /// After:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   ScanEdges(limit=110)
    /// ```
    ///
    /// # Applicable Conditions
    ///
    /// The current node is a Limit node.
    /// The child node is a ScanEdges node.
    /// The Limit node has only one child node.
    /// The `ScanEdges` object has not yet had its `limit` property set, or the new `limit` value is smaller than the existing `limit` value.
    name: PushLimitDownScanEdgesRule,
    parent_node: Limit,
    child_node: ScanEdges,
    apply: |_ctx, limit_node: &LimitNode, scan_edges_node: &ScanEdgesNode| {
        // Calculate the total number of rows that need to be retrieved (offset + count).
        let limit_rows = limit_node.offset() + limit_node.count();

        // Check whether there is a more stringent limit already in place for ScanEdges.
        if let Some(existing_limit) = scan_edges_node.limit() {
            if limit_rows >= existing_limit {
                // The existing restrictions are already more stringent; there is no need for any conversion.
                return Ok(None::<TransformResult>);
            }
        }

        // Create a new ScanEdges node and set the limit.
        let mut new_scan_edges = scan_edges_node.clone();
        new_scan_edges.set_limit(limit_rows);

        // Create the translation result.
        let mut result = TransformResult::new();
        result.erase_all = true;
        result.add_new_node(PlanNodeEnum::ScanEdges(new_scan_edges));

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_rule_name() {
        let rule = PushLimitDownScanEdgesRule::new();
        assert_eq!(rule.name(), "PushLimitDownScanEdgesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushLimitDownScanEdgesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

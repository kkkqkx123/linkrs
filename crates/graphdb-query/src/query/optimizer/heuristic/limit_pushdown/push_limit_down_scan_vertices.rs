//! The rule for pushing down the LIMIT statement to the operation that scans the vertices
//!
//! This rule identifies the “Limit -> ScanVertices” mode.
//! And integrate the LIMIT value into the ScanVertices operation.

use crate::query::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::query::optimizer::heuristic::result::TransformResult;
use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;

define_rewrite_pushdown_rule! {
    /// The rule for pushing down the LIMIT statement to the operation that scans the vertices
    ///
    /// # Conversion example
    ///
    /// Before:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   ScanVertices
    /// ```
    ///
    /// After:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   ScanVertices(limit=110)
    /// ```
    ///
    /// # Applicable Conditions
    ///
    /// The current node is a Limit node.
    /// The child node is a ScanVertices node.
    /// A Limit node has only one child node.
    /// The `ScanVertices` object has not yet had its `limit` property set, or the new `limit` value is less than the existing `limit` value.
    name: PushLimitDownScanVerticesRule,
    parent_node: Limit,
    child_node: ScanVertices,
    apply: |_ctx, limit_node: &LimitNode, scan_vertices_node: &ScanVerticesNode| {
        // Calculate the total number of rows that need to be retrieved (offset + count).
        let limit_rows = limit_node.offset() + limit_node.count();

        // Check whether ScanVertices already has a more stringent limit.
        if let Some(existing_limit) = scan_vertices_node.limit() {
            if limit_rows >= existing_limit {
                // The existing restrictions are already more stringent; no conversion is required.
                return Ok(None::<TransformResult>);
            }
        }

        // Create a new ScanVertices node and set the limit.
        let mut new_scan_vertices = scan_vertices_node.clone();
        new_scan_vertices.set_limit(limit_rows);

        // Create the translation result.
        let mut result = TransformResult::new();
        result.erase_all = true;
        result.add_new_node(PlanNodeEnum::ScanVertices(new_scan_vertices));

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_rule_name() {
        let rule = PushLimitDownScanVerticesRule::new();
        assert_eq!(rule.name(), "PushLimitDownScanVerticesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushLimitDownScanVerticesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

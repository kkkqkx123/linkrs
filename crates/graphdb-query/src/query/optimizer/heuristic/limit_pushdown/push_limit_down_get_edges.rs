//! Push the LIMIT down to the rule that retrieves the edge operations.
//!
//! This rule identifies the Limit -> GetEdges mode.
//! And integrate the LIMIT value into the GetEdges operation.

use crate::query::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::query::optimizer::heuristic::result::TransformResult;
use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetEdgesNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;

define_rewrite_pushdown_rule! {
    /// Push the LIMIT down to the rule that retrieves the edge operations.
    ///
    /// # Translation example
    ///
    /// Before:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   GetEdges
    /// ```
    ///
    /// After:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   GetEdges(limit=110)
    /// ```
    ///
    /// # Applicable Conditions
    ///
    /// The current node is a Limit node.
    /// The child node is a GetEdges node.
    /// The Limit node has only one child node.
    /// The `GetEdges` method has not yet had its `limit` parameter set, or the new `limit` value is less than the existing `limit` value.
    name: PushLimitDownGetEdgesRule,
    parent_node: Limit,
    child_node: GetEdges,
    apply: |_ctx, limit_node: &LimitNode, get_edges_node: &GetEdgesNode| {
        // Calculate the total number of rows that need to be retrieved (offset + count).
        let limit_rows = limit_node.offset() + limit_node.count();

        // Check whether there is a more stringent limit already in place for the GetEdges function.
        if let Some(existing_limit) = get_edges_node.limit() {
            if limit_rows >= existing_limit {
                // The existing restrictions are already more stringent; no conversion is necessary.
                return Ok(None::<TransformResult>);
            }
        }

        // Create a new GetEdges node and set the limit value.
        let mut new_get_edges = get_edges_node.clone();
        new_get_edges.set_limit(limit_rows);

        // Create the translation result.
        let mut result = TransformResult::new();
        result.erase_all = true;
        result.add_new_node(PlanNodeEnum::GetEdges(new_get_edges));

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_rule_name() {
        let rule = PushLimitDownGetEdgesRule::new();
        assert_eq!(rule.name(), "PushLimitDownGetEdgesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushLimitDownGetEdgesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

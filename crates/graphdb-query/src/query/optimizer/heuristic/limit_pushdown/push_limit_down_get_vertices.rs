//! Push the LIMIT down to the rules that govern the operation of retrieving vertices.
//!
//! This rule identifies the “Limit -> GetVertices” mode.
//! And integrate the LIMIT value into the GetVertices operation.

use crate::query::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::query::optimizer::heuristic::result::TransformResult;
use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetVerticesNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;

define_rewrite_pushdown_rule! {
    /// Push the LIMIT down to the rules that govern the operation of retrieving vertices.
    ///
    /// # Conversion example
    ///
    /// Before:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   GetVertices
    /// ```
    ///
    /// After:
    /// ```text
    ///   Limit(offset=10, count=100)
    ///       |
    ///   GetVertices(limit=110)
    /// ```
    ///
    /// # Applicable Conditions
    ///
    /// The current node is a Limit node.
    /// The child node is a GetVertices node.
    /// A Limit node has only one child node.
    /// The `GetVertices` method has not had its `limit` parameter set, or the new `limit` value is less than the existing `limit` value.
    name: PushLimitDownGetVerticesRule,
    parent_node: Limit,
    child_node: GetVertices,
    apply: |_ctx, limit_node: &LimitNode, get_vertices_node: &GetVerticesNode| {
        // Calculate the total number of rows that need to be retrieved (offset + count).
        let limit_rows = limit_node.offset() + limit_node.count();

        // Check whether GetVertices already has a more stringent limit in place.
        if let Some(existing_limit) = get_vertices_node.limit() {
            if limit_rows >= existing_limit {
                // The existing restrictions are already more stringent; no conversion is necessary.
                return Ok(None::<TransformResult>);
            }
        }

        // Create a new GetVertices node and set the limit value.
        let mut new_get_vertices = get_vertices_node.clone();
        new_get_vertices.set_limit(limit_rows);

        // Create the translation result.
        let mut result = TransformResult::new();
        result.erase_all = true;
        result.add_new_node(PlanNodeEnum::GetVertices(new_get_vertices));

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_rule_name() {
        let rule = PushLimitDownGetVerticesRule::new();
        assert_eq!(rule.name(), "PushLimitDownGetVerticesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushLimitDownGetVerticesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

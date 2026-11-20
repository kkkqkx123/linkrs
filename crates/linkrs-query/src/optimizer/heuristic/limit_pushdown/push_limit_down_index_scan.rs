//! The rule for pushing down the LIMIT clause to the index scanning operation
//!
//! This rule identifies the mode in which the operation switches from “Limit” to “IndexScan”.
//! And integrate the LIMIT value into the IndexScan operation.

use crate::optimizer::heuristic::macros::define_rewrite_pushdown_rule;
use crate::optimizer::heuristic::result::TransformResult;
use crate::planning::plan::core::nodes::access::IndexScanNode;
use crate::planning::plan::core::nodes::operation::sort_node::LimitNode;

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
        // LIMIT without ORDER BY may take any rows, but a residual filter or
        // an existing sort on the scan makes the row prefix non-equivalent.
        if index_scan_node.filter().is_some() || !index_scan_node.order_by().is_empty() {
            return Ok(None::<TransformResult>);
        }
        let offset = limit_node.offset();
        let count = limit_node.count();
        if offset < 0 || count < 0 || count == LimitNode::UNBOUNDED {
            return Ok(None::<TransformResult>);
        }
        let (Ok(offset_rows), Ok(limit_rows)) =
            (usize::try_from(offset), usize::try_from(count))
        else {
            return Ok(None::<TransformResult>);
        };
        if limit_rows == 0 {
            return Ok(None::<TransformResult>);
        }

        // Check whether there is a more stringent limit for IndexScan already in place.
        if let Some(existing_limit) = index_scan_node.limit() {
            let Ok(existing_rows) = usize::try_from(existing_limit) else {
                return Ok(None::<TransformResult>);
            };
            if offset_rows >= index_scan_node.offset() && limit_rows >= existing_rows {
                // The existing restrictions are already more stringent; no conversion is required.
                return Ok(None::<TransformResult>);
            }
        }

        // Create a new IndexScan node and set the limit.
        let mut new_index_scan = index_scan_node.clone();
        new_index_scan.set_offset(offset_rows);
        new_index_scan.set_limit(count);

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
    use crate::planning::plan::core::nodes::access::{IndexScanNode, OrderByItem, ScanType};
    use crate::planning::plan::PlanNodeEnum;

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

    fn scan() -> IndexScanNode {
        IndexScanNode::new(
            1,
            1,
            1,
            "idx_age".to_string(),
            "Person".to_string(),
            ScanType::Range,
        )
    }

    #[test]
    fn unbounded_count_is_not_pushed() {
        let rule = PushLimitDownIndexScanRule::new();
        let mut ctx = RewriteContext::new();
        let limit = LimitNode::new(PlanNodeEnum::IndexScan(scan()), 1, LimitNode::UNBOUNDED)
            .expect("limit");
        assert!(rule
            .apply(&mut ctx, &PlanNodeEnum::Limit(limit))
            .expect("apply")
            .is_none());
    }

    #[test]
    fn ordered_scan_refuses_limit_push() {
        let rule = PushLimitDownIndexScanRule::new();
        let mut ctx = RewriteContext::new();
        let mut scan = scan();
        scan.set_order_by(vec![OrderByItem::asc("age".to_string())]);
        let limit = LimitNode::new(PlanNodeEnum::IndexScan(scan), 0, 10).expect("limit");
        assert!(rule
            .apply(&mut ctx, &PlanNodeEnum::Limit(limit))
            .expect("apply")
            .is_none());
    }

    #[test]
    fn pushes_offset_and_emit_count_separately() {
        let rule = PushLimitDownIndexScanRule::new();
        let mut ctx = RewriteContext::new();
        let limit = LimitNode::new(PlanNodeEnum::IndexScan(scan()), 5, 10).expect("limit");
        let result = rule
            .apply(&mut ctx, &PlanNodeEnum::Limit(limit))
            .expect("apply")
            .expect("pushed");
        let PlanNodeEnum::IndexScan(pushed) = &result.new_nodes[0] else {
            panic!("expected index scan");
        };
        assert_eq!(pushed.limit(), Some(10));
        assert_eq!(pushed.offset(), 5);
    }
}

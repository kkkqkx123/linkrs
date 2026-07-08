//! Rule for converting Sort + Limit to TopN
//!
//! This rule identifies the pattern "Limit -> Sort" and converts it to a single TopN node.
//! TopN is more efficient than Sort + Limit because it only needs to keep track of the top N rows
//! instead of sorting all data.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteError, RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::TopNNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rule for converting Sort + Limit to TopN
///
/// # Conversion Example
///
/// Before:
/// ```text
///   Limit(offset=0, count=10)
///       |
///   Sort(sort_items=[price DESC])
///       |
///   ...
/// ```
///
/// After:
/// ```text
///   TopN(sort_items=[price DESC], limit=10)
///       |
///   ...
/// ```
///
/// # Applicable Conditions
///
/// - Current node is a Limit node
/// - Child node is a Sort node
/// - Limit offset is 0 (TopN does not support offset)
/// - Limit count is reasonable (> 0)
#[derive(Debug)]
pub struct ConvertSortLimitToTopNRule;

impl ConvertSortLimitToTopNRule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ConvertSortLimitToTopNRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for ConvertSortLimitToTopNRule {
    fn name(&self) -> &'static str {
        "ConvertSortLimitToTopNRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Limit").with_dependency_name("Sort")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let limit_node = match node {
            PlanNodeEnum::Limit(n) => n,
            _ => return Ok(None),
        };

        let input = limit_node.input();
        let sort_node = match input {
            PlanNodeEnum::Sort(n) => n,
            _ => return Ok(None),
        };

        if limit_node.offset() != 0 {
            return Ok(None);
        }

        let limit_count = limit_node.count();
        if limit_count <= 0 {
            return Ok(None);
        }

        let sort_items = sort_node.sort_items().to_vec();
        let sort_input = sort_node.input().clone();

        let topn_node = TopNNode::new(sort_input, sort_items, limit_count).map_err(|e| {
            RewriteError::rewrite_failed(format!("Failed to create TopNNode: {}", e))
        })?;

        let mut result = TransformResult::new();
        result.erase_all = true;
        result.add_new_node(PlanNodeEnum::TopN(topn_node));

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::operation::sort_node::{
        LimitNode, SortItem, SortNode,
    };

    #[test]
    fn test_rule_name() {
        let rule = ConvertSortLimitToTopNRule::new();
        assert_eq!(rule.name(), "ConvertSortLimitToTopNRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = ConvertSortLimitToTopNRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_convert_sort_limit_to_topn() {
        let start_node = PlanNodeEnum::Start(StartNode::new());
        let sort_items = vec![SortItem::column_desc("price".to_string())];
        let sort_node = SortNode::new(start_node, sort_items).expect("Failed to create SortNode");
        let limit_node = LimitNode::new(PlanNodeEnum::Sort(sort_node), 0, 10)
            .expect("Failed to create LimitNode");

        let rule = ConvertSortLimitToTopNRule::new();
        let mut ctx = RewriteContext::new();

        let result = rule
            .apply(&mut ctx, &PlanNodeEnum::Limit(limit_node))
            .expect("Rule application failed");

        assert!(result.is_some());
        let transform_result = result.unwrap();
        assert!(transform_result.erase_all);

        let new_nodes = &transform_result.new_nodes;
        assert_eq!(new_nodes.len(), 1);
        assert!(matches!(&new_nodes[0], PlanNodeEnum::TopN(_)));
    }

    #[test]
    fn test_no_conversion_with_offset() {
        let start_node = PlanNodeEnum::Start(StartNode::new());
        let sort_items = vec![SortItem::column_desc("price".to_string())];
        let sort_node = SortNode::new(start_node, sort_items).expect("Failed to create SortNode");
        let limit_node = LimitNode::new(PlanNodeEnum::Sort(sort_node), 5, 10)
            .expect("Failed to create LimitNode");

        let rule = ConvertSortLimitToTopNRule::new();
        let mut ctx = RewriteContext::new();

        let result = rule
            .apply(&mut ctx, &PlanNodeEnum::Limit(limit_node))
            .expect("Rule application failed");

        assert!(result.is_none());
    }
}

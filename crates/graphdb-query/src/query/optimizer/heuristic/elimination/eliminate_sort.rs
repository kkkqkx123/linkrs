//! Sorting elimination rules
//!
//! Heuristic rule: When the input data is already sorted, eliminate unnecessary sorting operations.
//! This includes:
//! The data returned by the index scan (sorted according to the index key)
//! The results of the subquery, which have already been sorted.
//! Ordered scanning of data sources
//!
//! # Translation example
//!
//! Before:
//! ```text
//!   Sort(name ASC)
//!       |
//!   IndexScan(idx_name)  -- 索引已按 name 排序
//! ```
//!
//! After:
//! ```text
//!   IndexScan(idx_name)  -- 直接消除 Sort
//! ```
//!
//! # Note
//!
//! This rule is heuristic in nature and **does not rely on cost calculations**.
//! As soon as it is detected that the input is in order and matches the sorting requirements, the sorting process is directly canceled.
//! The cost-based TopN conversion decision-making mechanism is still implemented in the strategy::sort_elimination module.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{EliminationRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::{SortItem, SortNode};
use crate::query::planning::plan::PlanNodeEnum;

/// Sorting elimination rules
///
/// When it is detected that the input data is already sorted and meets the sorting requirements, the Sort node is removed.
/// These are heuristic rules that do not rely on cost calculations.
#[derive(Debug)]
pub struct EliminateSortRule;

impl EliminateSortRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }

    /// Check whether it is possible to eliminate the sorting.
    ///
    /// Return `true` if:
    /// The input involves an index scan, and the order of the index matches the sorting requirements.
    /// The input is already sorted (for example, it comes from another sorting process or an ordered scan).
    fn can_eliminate_sort(&self, sort_node: &SortNode, input: &PlanNodeEnum) -> bool {
        let sort_items = sort_node.sort_items();

        match input {
            // Index scan: Checking whether the index meets the sorting requirements.
            PlanNodeEnum::IndexScan(index_scan) => {
                // Obtain the sorted columns of the index
                // Assume that the index is sorted in ascending order according to the first attribute.
                // In actual implementation, the sorting information should be obtained from the index metadata.
                let index_columns = vec![index_scan.index_name().to_string()];
                self.check_order_match(sort_items, &index_columns)
            }
            // Another Sort node – checking whether the sorting keys are compatible
            PlanNodeEnum::Sort(inner_sort) => {
                self.check_sort_compatibility(sort_items, inner_sort.sort_items())
            }
            // TopN nodes – The output is sorted.
            PlanNodeEnum::TopN(topn) => {
                self.check_sort_compatibility(sort_items, topn.sort_items())
            }
            // Other situations: Cannot be resolved for the time being.
            _ => false,
        }
    }

    /// Check whether the sorting requirements match the order of the index.
    ///
    /// Check whether the sorting key is a prefix of the index column.
    fn check_order_match(&self, sort_items: &[SortItem], index_columns: &[String]) -> bool {
        if sort_items.is_empty() {
            return true;
        }

        // Check whether the sort key is a prefix of the index column.
        for (i, sort_item) in sort_items.iter().enumerate() {
            if i >= index_columns.len() {
                return false;
            }
            // Only check column names for simple variable references.
            // Complex expressions (function calls, etc.) cannot be optimized.
            match sort_item.column_name() {
                Some(col_name) if col_name == index_columns[i] => {}
                _ => return false,
            }
        }

        true
    }

    /// Check whether the two sorts are compatible.
    ///
    /// If the outer sorting is a prefix of the inner sorting, then the outer sorting can be eliminated.
    fn check_sort_compatibility(&self, outer_items: &[SortItem], inner_items: &[SortItem]) -> bool {
        if outer_items.is_empty() {
            return true;
        }

        // The outer sort must be a prefix of the inner sort.
        if outer_items.len() > inner_items.len() {
            return false;
        }

        for (i, outer_item) in outer_items.iter().enumerate() {
            let inner_item = &inner_items[i];
            // Both the column names and the directions must match.
            // Only check for simple column references.
            let outer_col = outer_item.column_name();
            let inner_col = inner_item.column_name();
            if outer_col != inner_col || outer_item.direction != inner_item.direction {
                return false;
            }
        }

        true
    }
}

impl Default for EliminateSortRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for EliminateSortRule {
    fn name(&self) -> &'static str {
        "EliminateSortRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Sort")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Obtaining the Sort node
        let sort_node = match node {
            PlanNodeEnum::Sort(n) => n,
            _ => return Ok(None),
        };

        // Obtain the input node
        let input = sort_node.input();

        // Check whether it is possible to eliminate the sorting.
        if self.can_eliminate_sort(sort_node, input) {
            // Remove the Sort node and return the input directly.
            let mut result = TransformResult::new();
            result.new_nodes.push(input.clone());
            return Ok(Some(result));
        }

        Ok(None)
    }
}

impl EliminationRule for EliminateSortRule {
    fn can_eliminate(&self, node: &PlanNodeEnum) -> bool {
        let sort_node = match node {
            PlanNodeEnum::Sort(n) => n,
            _ => return false,
        };

        let input = sort_node.input();
        self.can_eliminate_sort(sort_node, input)
    }

    fn eliminate(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let sort_node = match node {
            PlanNodeEnum::Sort(n) => n,
            _ => return Ok(None),
        };

        // Remove the Sort node and return the input directly.
        let input = sort_node.input();
        let mut result = TransformResult::new();
        result.new_nodes.push(input.clone());
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::{SortItem, SortNode, StartNode};

    #[test]
    fn test_eliminate_sort_rule_name() {
        let rule = EliminateSortRule::new();
        assert_eq!(rule.name(), "EliminateSortRule");
    }

    #[test]
    fn test_eliminate_sort_rule_pattern() {
        let rule = EliminateSortRule::new();
        assert!(rule.pattern().matches(&PlanNodeEnum::Sort(
            SortNode::new(
                PlanNodeEnum::Start(StartNode::new()),
                vec![SortItem::column_asc("name".to_string())],
            )
            .expect("Failed to create SortNode")
        )));
    }

    #[test]
    fn test_check_sort_compatibility_exact_match() {
        let rule = EliminateSortRule::new();

        let outer = vec![
            SortItem::column_asc("name".to_string()),
            SortItem::column_asc("age".to_string()),
        ];
        let inner = vec![
            SortItem::column_asc("name".to_string()),
            SortItem::column_asc("age".to_string()),
        ];

        assert!(rule.check_sort_compatibility(&outer, &inner));
    }

    #[test]
    fn test_check_sort_compatibility_prefix_match() {
        let rule = EliminateSortRule::new();

        let outer = vec![SortItem::column_asc("name".to_string())];
        let inner = vec![
            SortItem::column_asc("name".to_string()),
            SortItem::column_asc("age".to_string()),
        ];

        assert!(rule.check_sort_compatibility(&outer, &inner));
    }

    #[test]
    fn test_check_sort_compatibility_direction_mismatch() {
        let rule = EliminateSortRule::new();

        let outer = vec![SortItem::column_desc("name".to_string())];
        let inner = vec![SortItem::column_asc("name".to_string())];

        assert!(!rule.check_sort_compatibility(&outer, &inner));
    }

    #[test]
    fn test_check_sort_compatibility_column_mismatch() {
        let rule = EliminateSortRule::new();

        let outer = vec![SortItem::column_asc("name".to_string())];
        let inner = vec![SortItem::column_asc("age".to_string())];

        assert!(!rule.check_sort_compatibility(&outer, &inner));
    }

    #[test]
    fn test_check_sort_compatibility_empty() {
        let rule = EliminateSortRule::new();

        let outer: Vec<SortItem> = vec![];
        let inner = vec![SortItem::column_asc("name".to_string())];

        assert!(rule.check_sort_compatibility(&outer, &inner));
    }
}

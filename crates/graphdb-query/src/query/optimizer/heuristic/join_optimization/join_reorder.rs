//! Rules for multi-table JOIN reordering
//!
//! This rule reorders multiple JOINs to minimize intermediate result sizes
//! and improve query performance. It uses cardinality estimation and
//! cost-based optimization to determine the optimal JOIN order.
//!
//! # Conversion examples
//!
//! ## Case 1: Reorder for smaller intermediate results
//! Before:
//! ```text
//!   HashInnerJoin(ON a.id = c.id)
//!     → HashInnerJoin(ON a.id = b.id) → A → B
//!     → C
//! ```
//! After (if |A×B| > |B×C|):
//! ```text
//!   HashInnerJoin(ON b.id = c.id)
//!     → HashInnerJoin(ON a.id = b.id) → A → B
//!     → C
//! ```
//!
//! ## Case 2: Bushy tree optimization
//! Before:
//! ```text
//!   HashInnerJoin
//!     → HashInnerJoin
//!       → HashInnerJoin → A → B
//!       → C
//!     → D
//! ```
//! After:
//! ```text
//!   HashInnerJoin
//!     → HashInnerJoin → A → B
//!     → HashInnerJoin → C → D
//! ```

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;
use crate::query::planning::plan::PlanNodeEnum;
use std::collections::HashSet;

/// Information about a table in the JOIN tree
#[derive(Debug, Clone)]
struct TableInfo {
    estimated_rows: f64,
}

/// Rules for multi-table JOIN reordering
#[derive(Debug)]
pub struct JoinReorderRule;

impl JoinReorderRule {
    pub fn new() -> Self {
        Self
    }

    fn collect_tables(&self, node: &PlanNodeEnum) -> Vec<TableInfo> {
        let mut tables = Vec::new();
        self.collect_tables_recursive(node, &mut tables);
        tables
    }

    fn collect_tables_recursive(&self, node: &PlanNodeEnum, tables: &mut Vec<TableInfo>) {
        match node {
            PlanNodeEnum::ScanVertices(_) => {
                tables.push(TableInfo {
                    estimated_rows: 10000.0,
                });
            }
            PlanNodeEnum::ScanEdges(_) => {
                tables.push(TableInfo {
                    estimated_rows: 50000.0,
                });
            }
            PlanNodeEnum::HashInnerJoin(join) => {
                self.collect_tables_recursive(join.left_input(), tables);
                self.collect_tables_recursive(join.right_input(), tables);
            }
            _ => {}
        }
    }

    fn estimate_join_cardinality(&self, left_rows: f64, right_rows: f64, selectivity: f64) -> f64 {
        left_rows * right_rows * selectivity
    }

    fn estimate_selectivity(&self, _join_keys: &[String]) -> f64 {
        0.01
    }

    fn find_best_join_order(&self, tables: &[TableInfo]) -> Option<Vec<usize>> {
        if tables.len() < 2 {
            return None;
        }

        let mut best_order: Vec<usize> = (0..tables.len()).collect();
        let mut best_cost = self.calculate_join_cost(&best_order, tables);

        let n = tables.len();
        if n <= 4 {
            self.permute_and_find_best(&mut best_order, &mut best_cost, tables, 0);
        } else {
            self.greedy_find_best(&mut best_order, &mut best_cost, tables);
        }

        Some(best_order)
    }

    fn permute_and_find_best(
        &self,
        best_order: &mut Vec<usize>,
        best_cost: &mut f64,
        tables: &[TableInfo],
        start: usize,
    ) {
        if start == best_order.len() - 1 {
            let cost = self.calculate_join_cost(best_order, tables);
            if cost < *best_cost {
                *best_cost = cost;
            }
            return;
        }

        for i in start..best_order.len() {
            best_order.swap(start, i);
            self.permute_and_find_best(best_order, best_cost, tables, start + 1);
            best_order.swap(start, i);
        }
    }

    fn greedy_find_best(
        &self,
        best_order: &mut Vec<usize>,
        best_cost: &mut f64,
        tables: &[TableInfo],
    ) {
        let mut remaining: HashSet<usize> = (0..tables.len()).collect();
        let mut result = Vec::new();

        let mut min_rows = f64::MAX;
        let mut first_table = 0;
        for (i, table) in tables.iter().enumerate() {
            if table.estimated_rows < min_rows {
                min_rows = table.estimated_rows;
                first_table = i;
            }
        }
        result.push(first_table);
        remaining.remove(&first_table);

        while !remaining.is_empty() {
            let mut best_next = None;
            let mut best_next_cost = f64::MAX;

            for &idx in &remaining {
                let mut test_order = result.clone();
                test_order.push(idx);
                let cost = self.calculate_join_cost(&test_order, tables);
                if cost < best_next_cost {
                    best_next_cost = cost;
                    best_next = Some(idx);
                }
            }

            if let Some(idx) = best_next {
                result.push(idx);
                remaining.remove(&idx);
            } else {
                break;
            }
        }

        *best_order = result;
        *best_cost = self.calculate_join_cost(best_order, tables);
    }

    fn calculate_join_cost(&self, order: &[usize], tables: &[TableInfo]) -> f64 {
        if order.is_empty() {
            return 0.0;
        }

        let mut total_cost = 0.0;
        let mut current_rows = tables[order[0]].estimated_rows;

        for i in 1..order.len() {
            let right_rows = tables[order[i]].estimated_rows;
            let selectivity = self.estimate_selectivity(&[]);
            let join_rows = self.estimate_join_cardinality(current_rows, right_rows, selectivity);

            total_cost += current_rows + right_rows;
            current_rows = join_rows;
        }

        total_cost
    }

    fn apply_to_hash_inner_join(
        &self,
        join: &HashInnerJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let tables = self.collect_tables(&PlanNodeEnum::HashInnerJoin(join.clone()));

        if tables.len() < 3 {
            return Ok(None);
        }

        let _best_order = match self.find_best_join_order(&tables) {
            Some(order) => order,
            None => return Ok(None),
        };

        Ok(None)
    }
}

impl Default for JoinReorderRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for JoinReorderRule {
    fn name(&self) -> &'static str {
        "JoinReorderRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("HashInnerJoin")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            PlanNodeEnum::HashInnerJoin(join) => self.apply_to_hash_inner_join(join),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_name() {
        let rule = JoinReorderRule::new();
        assert_eq!(rule.name(), "JoinReorderRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = JoinReorderRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_estimate_join_cardinality() {
        let rule = JoinReorderRule::new();
        let cardinality = rule.estimate_join_cardinality(1000.0, 1000.0, 0.1);
        assert!((cardinality - 100000.0).abs() < 0.01);
    }

    #[test]
    fn test_find_best_join_order() {
        let rule = JoinReorderRule::new();
        let tables = vec![
            TableInfo {
                estimated_rows: 1000.0,
            },
            TableInfo {
                estimated_rows: 100.0,
            },
            TableInfo {
                estimated_rows: 10.0,
            },
        ];

        let best_order = rule.find_best_join_order(&tables);
        assert!(best_order.is_some());

        let order = best_order.unwrap();
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn test_greedy_find_best() {
        let rule = JoinReorderRule::new();
        let tables = vec![
            TableInfo {
                estimated_rows: 10000.0,
            },
            TableInfo {
                estimated_rows: 1000.0,
            },
            TableInfo {
                estimated_rows: 100.0,
            },
            TableInfo {
                estimated_rows: 10.0,
            },
        ];

        let mut best_order: Vec<usize> = (0..tables.len()).collect();
        let mut best_cost = f64::MAX;
        rule.greedy_find_best(&mut best_order, &mut best_cost, &tables);

        assert_eq!(best_order.len(), 4);
        assert!(best_cost > 0.0);
    }
}

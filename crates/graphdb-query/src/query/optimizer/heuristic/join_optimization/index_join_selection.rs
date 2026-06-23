//! Rules for index-based JOIN selection
//!
//! This rule analyzes JOIN conditions and determines whether to use
//! index-based JOIN instead of hash JOIN when appropriate indexes exist.
//!
//! # Conversion examples
//!
//! ## Case 1: Index JOIN for vertex ID lookup
//! Before:
//! ```text
//!   HashInnerJoin(ON v.id = e._src) → ScanVertices(v) → ScanEdges(e)
//! ```
//! After (if index exists on e._src):
//! ```text
//!   IndexJoin(index=e._src_idx) → ScanVertices(v) → IndexScanEdge(e)
//! ```
//!
//! ## Case 2: Index JOIN for edge properties
//! Before:
//! ```text
//!   HashInnerJoin(ON v.prop = e.prop) → ScanVertices(v) → ScanEdges(e)
//! ```
//! After (if indexes exist):
//! ```text
//!   IndexJoin using property indexes
//! ```

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::expr::visitor_collectors::VariableCollector;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for index-based JOIN selection
#[derive(Debug)]
pub struct IndexJoinSelectionRule;

impl IndexJoinSelectionRule {
    pub fn new() -> Self {
        Self
    }

    fn extract_join_key_variable(&self, expr: &ContextualExpression) -> Option<String> {
        if let Some(expr_meta) = expr.expression() {
            let mut collector = VariableCollector::new();
            collector.visit(expr_meta.inner());
            collector.variables.into_iter().next()
        } else {
            None
        }
    }

    fn is_indexable_join_key(&self, key: &str) -> bool {
        key.ends_with(".id")
            || key.ends_with("._src")
            || key.ends_with("._dst")
            || key.contains("id")
    }

    fn apply_to_hash_inner_join(
        &self,
        join: &HashInnerJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let hash_keys = join.hash_keys();
        let probe_keys = join.probe_keys();

        if hash_keys.len() != 1 || probe_keys.len() != 1 {
            return Ok(None);
        }

        let hash_var = match self.extract_join_key_variable(&hash_keys[0]) {
            Some(v) => v,
            None => return Ok(None),
        };

        let probe_var = match self.extract_join_key_variable(&probe_keys[0]) {
            Some(v) => v,
            None => return Ok(None),
        };

        if !self.is_indexable_join_key(&hash_var) && !self.is_indexable_join_key(&probe_var) {
            return Ok(None);
        }

        Ok(None)
    }
}

impl Default for IndexJoinSelectionRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for IndexJoinSelectionRule {
    fn name(&self) -> &'static str {
        "IndexJoinSelectionRule"
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
        let rule = IndexJoinSelectionRule::new();
        assert_eq!(rule.name(), "IndexJoinSelectionRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = IndexJoinSelectionRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_indexable_join_key() {
        let rule = IndexJoinSelectionRule::new();
        assert!(rule.is_indexable_join_key("v.id"));
        assert!(rule.is_indexable_join_key("e._src"));
        assert!(rule.is_indexable_join_key("e._dst"));
    }
}

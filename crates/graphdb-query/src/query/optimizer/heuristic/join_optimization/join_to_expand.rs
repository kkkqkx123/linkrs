//! Rules for converting Vertex-Edge JOIN to ExpandAll
//!
//! This rule converts a JOIN between vertices and edges into an ExpandAll operation,
//! which is more efficient for graph traversal patterns.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   ScanVertices(v) → HashInnerJoin(ON v.id = e._src) → ScanEdges(e)
//! ```
//!
//! After:
//! ```text
//!   ScanVertices(v) → ExpandAll(edge_types, direction=OUT)
//! ```
//!
//! # Applicable Conditions
//!
//! One side is ScanVertices, the other is ScanEdges
//! JOIN condition connects vertex ID to edge source/destination
//! The edge types can be determined from the ScanEdges

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::expr::visitor_collectors::VariableCollector;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;
use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;
use crate::query::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for converting Vertex-Edge JOIN to ExpandAll
#[derive(Debug)]
pub struct JoinToExpandRule;

impl JoinToExpandRule {
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

    fn analyze_join_condition(&self, probe_keys: &[ContextualExpression]) -> Option<String> {
        if probe_keys.len() != 1 {
            return None;
        }

        self.extract_join_key_variable(&probe_keys[0])
    }

    fn determine_direction(&self, _edge_var: &str, join_key: &str) -> Option<&'static str> {
        if join_key.ends_with("._src") || join_key.contains("src") {
            Some("OUT")
        } else if join_key.ends_with("._dst") || join_key.contains("dst") {
            Some("IN")
        } else {
            None
        }
    }

    fn apply_to_hash_inner_join(
        &self,
        join: &HashInnerJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let left = join.left_input();
        let right = join.right_input();

        let (scan_vertices, scan_edges, vertex_on_left) = match (left, right) {
            (PlanNodeEnum::ScanVertices(v), PlanNodeEnum::ScanEdges(e)) => (v, e, true),
            (PlanNodeEnum::ScanEdges(e), PlanNodeEnum::ScanVertices(v)) => (v, e, false),
            _ => return Ok(None),
        };

        let probe_keys = if vertex_on_left {
            join.probe_keys()
        } else {
            join.hash_keys()
        };

        let probe_var = match self.analyze_join_condition(probe_keys) {
            Some(var) => var,
            None => return Ok(None),
        };

        let direction = self.determine_direction(&probe_var, &probe_var);
        let direction = match direction {
            Some(d) => d,
            None => return Ok(None),
        };

        let edge_types = scan_edges
            .edge_type()
            .map(|et| vec![et])
            .unwrap_or_default();

        let mut expand_all = ExpandAllNode::new(scan_vertices.space_id(), edge_types, direction);

        expand_all.add_input(left.clone());

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::ExpandAll(expand_all));

        Ok(Some(result))
    }
}

impl Default for JoinToExpandRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for JoinToExpandRule {
    fn name(&self) -> &'static str {
        "JoinToExpandRule"
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
        let rule = JoinToExpandRule::new();
        assert_eq!(rule.name(), "JoinToExpandRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = JoinToExpandRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

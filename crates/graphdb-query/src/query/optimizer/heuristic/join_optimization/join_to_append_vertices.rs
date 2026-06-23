//! Rules for converting Edge-Vertex JOIN to AppendVertices
//!
//! This rule converts a JOIN between edges and vertices into an AppendVertices operation,
//! which is more efficient for fetching vertex properties.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   ScanEdges(e) → HashInnerJoin(ON e._dst = v.id) → ScanVertices(v)
//! ```
//!
//! After:
//! ```text
//!   ScanEdges(e) → AppendVertices(vertex_tag)
//! ```
//!
//! # Applicable Conditions
//!
//! One side is ScanEdges, the other is ScanVertices
//! JOIN condition connects edge destination/source to vertex ID
//! The vertex tag can be determined from the ScanVertices

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::expr::visitor_collectors::VariableCollector;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;
use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;
use crate::query::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for converting Edge-Vertex JOIN to AppendVertices
#[derive(Debug)]
pub struct JoinToAppendVerticesRule;

impl JoinToAppendVerticesRule {
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

    fn analyze_join_condition(
        &self,
        hash_keys: &[ContextualExpression],
        probe_keys: &[ContextualExpression],
    ) -> Option<(String, String)> {
        if hash_keys.len() != 1 || probe_keys.len() != 1 {
            return None;
        }

        let hash_var = self.extract_join_key_variable(&hash_keys[0])?;
        let probe_var = self.extract_join_key_variable(&probe_keys[0])?;

        Some((hash_var, probe_var))
    }

    fn is_edge_to_vertex_join(&self, edge_key: &str, vertex_key: &str) -> bool {
        (edge_key.ends_with("._dst")
            || edge_key.ends_with("._src")
            || edge_key.contains("dst")
            || edge_key.contains("src"))
            && (vertex_key.ends_with(".id") || vertex_key == "id" || vertex_key.contains("id"))
    }

    fn apply_to_hash_inner_join(
        &self,
        join: &HashInnerJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let left = join.left_input();
        let right = join.right_input();

        let (scan_vertices, edge_on_left) = match (left, right) {
            (PlanNodeEnum::ScanEdges(_), PlanNodeEnum::ScanVertices(v)) => (v, true),
            (PlanNodeEnum::ScanVertices(v), PlanNodeEnum::ScanEdges(_)) => (v, false),
            _ => return Ok(None),
        };

        let (hash_keys, probe_keys) = if edge_on_left {
            (join.hash_keys(), join.probe_keys())
        } else {
            (join.probe_keys(), join.hash_keys())
        };

        let (hash_var, probe_var) = match self.analyze_join_condition(hash_keys, probe_keys) {
            Some(vars) => vars,
            None => return Ok(None),
        };

        let (edge_key, vertex_key) = if edge_on_left {
            (&hash_var, &probe_var)
        } else {
            (&probe_var, &hash_var)
        };

        if !self.is_edge_to_vertex_join(edge_key, vertex_key) {
            return Ok(None);
        }

        let vertex_tag = scan_vertices.tag().cloned().unwrap_or_default();
        let mut append_vertices = AppendVerticesNode::new(scan_vertices.space_id(), &vertex_tag);

        append_vertices.add_input(if edge_on_left {
            left.clone()
        } else {
            right.clone()
        });

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::AppendVertices(append_vertices));

        Ok(Some(result))
    }
}

impl Default for JoinToAppendVerticesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for JoinToAppendVerticesRule {
    fn name(&self) -> &'static str {
        "JoinToAppendVerticesRule"
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
        let rule = JoinToAppendVerticesRule::new();
        assert_eq!(rule.name(), "JoinToAppendVerticesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = JoinToAppendVerticesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

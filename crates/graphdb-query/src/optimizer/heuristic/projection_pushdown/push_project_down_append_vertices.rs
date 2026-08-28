//! AppendVertices: typed projection pushdown rule
//!
//! Pushes the projection requirement down to the `AppendVertices` node by
//! narrowing its `vertex_props` to the properties its consumers provably
//! need.
//!
//! # Safety
//!
//! The narrowing is driven by the typed [`RequiredPropertyAnalyzer`]: only
//! `Expression::Property { object: Variable(v), .. }` references count, and
//! any bare / opaque consumption of the vertex binding marks it as
//! full-value and blocks the rewrite entirely. The `Project` node itself is
//! preserved — only the columns read by the graph operator are narrowed.

use crate::optimizer::analysis::RequiredPropertyAnalyzer;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::PlanNodeEnum;

/// AppendVertices typed projection pushdown rule.
#[derive(Debug)]
pub struct PushProjectDownAppendVerticesRule;

impl PushProjectDownAppendVerticesRule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushProjectDownAppendVerticesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownAppendVerticesRule {
    fn name(&self) -> &'static str {
        "PushProjectDownAppendVerticesRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Project")
    }

    fn apply(
        &self,
        _ctx: &mut crate::optimizer::heuristic::context::RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let project_node = match node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        // Walk the chain of Filters down to the AppendVertices node, borrowing
        // the tree nodes so the requirement lookup uses the tree node ids
        // (clones mint fresh ids and would miss the analysis map).
        let mut chain: Vec<PlanNodeEnum> = Vec::new();
        let mut current = project_node.input();
        let append_node_id = loop {
            match current {
                PlanNodeEnum::Filter(f) => {
                    chain.push(current.clone());
                    current = f.input();
                }
                PlanNodeEnum::AppendVertices(a) => break a.id(),
                _ => return Ok(None),
            }
        };
        let append_node = match current {
            PlanNodeEnum::AppendVertices(a) => a,
            _ => unreachable!(),
        };

        let Some(var) = append_node.input_var() else {
            return Ok(None);
        };
        let map = RequiredPropertyAnalyzer::new().analyze(node);
        let Some(props) = map.narrowable_properties(append_node_id, var) else {
            return Ok(None);
        };

        // Narrow each tag's property list to the demanded set.  When every
        // tag's props become empty the node reads the full vertex (same
        // "empty means everything" convention as the source operators), so
        // keep the tag entries but with the narrowed lists.
        let mut narrowed = false;
        let vertex_props: Vec<_> = append_node
            .vertex_props()
            .iter()
            .map(|tp| {
                let kept: Vec<String> = tp
                    .props
                    .iter()
                    .filter(|p| props.contains(p))
                    .cloned()
                    .collect();
                if kept.len() != tp.props.len() {
                    narrowed = true;
                }
                crate::planning::plan::core::common::TagProp {
                    tag: tp.tag.clone(),
                    props: kept,
                }
            })
            .collect();
        if !narrowed {
            return Ok(None);
        }

        let mut new_append = append_node.clone();
        new_append.set_vertex_props(vertex_props);

        let mut input = PlanNodeEnum::AppendVertices(new_append);
        for level in chain.into_iter().rev() {
            let mut f = match level {
                PlanNodeEnum::Filter(f) => f,
                _ => unreachable!(),
            };
            f.set_input(input);
            input = PlanNodeEnum::Filter(f);
        }

        let mut new_project = project_node.clone();
        new_project.set_input(input);
        let new_node = PlanNodeEnum::Project(new_project);

        let mut result = TransformResult::new();
        result.add_new_node(new_node);
        Ok(Some(result))
    }
}

impl crate::optimizer::heuristic::rule::PushDownRule for PushProjectDownAppendVerticesRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(node, PlanNodeEnum::Project(_))
            && matches!(target, PlanNodeEnum::AppendVertices(_))
    }

    fn push_down(
        &self,
        ctx: &mut crate::optimizer::heuristic::context::RewriteContext,
        node: &PlanNodeEnum,
        _target: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::heuristic::context::RewriteContext;
    use crate::planning::plan::core::common::TagProp;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::types::ContextualExpression;
    use graphdb_core::{Expression, YieldColumn};
    use std::sync::Arc;

    fn contextual(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    fn prop(var: &str, name: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable(var.to_string())),
            property: name.to_string(),
        }
    }

    fn yield_column(expr: Expression, alias: &str) -> YieldColumn {
        YieldColumn {
            expression: contextual(expr),
            alias: alias.to_string(),
            is_matched: false,
        }
    }

    fn append_vertices(var: &str) -> PlanNodeEnum {
        let mut node = AppendVerticesNode::new(1, "person");
        node.set_input_var(var.to_string());
        node.set_vertex_props(vec![TagProp {
            tag: "person".to_string(),
            props: vec!["age".to_string(), "name".to_string()],
        }]);
        PlanNodeEnum::AppendVertices(node)
    }

    #[test]
    fn test_rule_name_and_pattern() {
        let rule = PushProjectDownAppendVerticesRule::new();
        assert_eq!(rule.name(), "PushProjectDownAppendVerticesRule");
        assert!(rule.pattern().node.is_some());
    }

    #[test]
    fn test_narrows_append_vertices_vertex_props() {
        let rule = PushProjectDownAppendVerticesRule::new();
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                append_vertices("v"),
                vec![yield_column(prop("v", "name"), "name")],
            )
            .expect("project node"),
        );

        let result = rule
            .apply(&mut RewriteContext::new(), &project)
            .expect("rewrite should succeed")
            .expect("projection must be narrowed");
        let PlanNodeEnum::Project(p) = &result.new_nodes[0] else {
            panic!("expected Project preserved");
        };
        let PlanNodeEnum::AppendVertices(a) = p.input() else {
            panic!("expected AppendVertices below Project");
        };
        let props = a.vertex_props();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].props, vec!["name".to_string()]);
    }

    #[test]
    fn test_bare_variable_blocks_narrowing() {
        let rule = PushProjectDownAppendVerticesRule::new();
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                append_vertices("v"),
                vec![yield_column(Expression::Variable("v".to_string()), "v")],
            )
            .expect("project node"),
        );

        let result = rule
            .apply(&mut RewriteContext::new(), &project)
            .expect("rewrite should succeed");
        assert!(result.is_none(), "bare variable must block narrowing");
    }

    #[test]
    fn test_noop_when_unchanged() {
        let rule = PushProjectDownAppendVerticesRule::new();
        let mut node = AppendVerticesNode::new(1, "person");
        node.set_input_var("v".to_string());
        node.set_vertex_props(vec![TagProp {
            tag: "person".to_string(),
            props: vec!["name".to_string()],
        }]);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::AppendVertices(node),
                vec![yield_column(prop("v", "name"), "name")],
            )
            .expect("project node"),
        );

        let result = rule
            .apply(&mut RewriteContext::new(), &project)
            .expect("rewrite should succeed");
        assert!(result.is_none(), "no-op when already narrowed");
    }
}

//! GetVertices: typed projection pushdown rule
//!
//! Pushes the projection requirement down to the `GetVertices` node by
//! narrowing its `projected_properties` to the properties its consumers
//! provably need.
//!
//! # Safety
//!
//! The narrowing is driven by the typed [`RequiredPropertyAnalyzer`]: only
//! `Expression::Property { object: Variable(v), .. }` references count, and
//! any bare / opaque consumption of the vertex variable (`RETURN v`,
//! `id(v)`, computed objects) marks the binding as full-value and blocks
//! the rewrite entirely. The `Project` node itself is preserved — only the
//! columns read by the graph operator are narrowed.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Project(v.age, v.name)
//!         |
//!     GetVertices (reads all properties)
//! ```
//!
//! After:
//! ```text
//! Project(v.age, v.name)
//!         |
//!     GetVertices (projected: [age, name])
//! ```

use crate::optimizer::analysis::{binding_var, RequiredPropertyAnalyzer};
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::PlanNodeEnum;

/// GetVertices typed projection pushdown rule.
#[derive(Debug)]
pub struct PushProjectDownGetVerticesRule;

impl PushProjectDownGetVerticesRule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushProjectDownGetVerticesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownGetVerticesRule {
    fn name(&self) -> &'static str {
        "PushProjectDownGetVerticesRule"
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

        // Walk the chain of Filters down to the GetVertices node, borrowing
        // the tree nodes so the requirement lookup uses the tree node ids
        // (clones mint fresh ids and would miss the analysis map).
        let mut chain: Vec<PlanNodeEnum> = Vec::new();
        let mut current = project_node.input();
        let get_node_id = loop {
            match current {
                PlanNodeEnum::Filter(f) => {
                    chain.push(current.clone());
                    current = f.input();
                }
                PlanNodeEnum::GetVertices(g) => break g.id(),
                _ => return Ok(None),
            }
        };
        let get_node = match current {
            PlanNodeEnum::GetVertices(g) => g,
            _ => unreachable!(),
        };

        // Typed demand analysis of the consumed subtree: the Project is the
        // only consumer boundary, so the requirement recorded at the
        // GetVertices leaf covers every prunable reference.
        let Some(var) = binding_var(get_node.output_var(), get_node.src_vids()) else {
            return Ok(None);
        };
        let map = RequiredPropertyAnalyzer::new().analyze(node);
        let Some(props) = map.narrowable_properties(get_node_id, var) else {
            return Ok(None);
        };
        if props == get_node.projected_properties() {
            return Ok(None);
        }

        let mut new_get = get_node.clone();
        new_get.set_projected_properties(props);

        let mut input = PlanNodeEnum::GetVertices(new_get);
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

impl crate::optimizer::heuristic::rule::PushDownRule for PushProjectDownGetVerticesRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(node, PlanNodeEnum::Project(_)) && matches!(target, PlanNodeEnum::GetVertices(_))
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
    use crate::planning::plan::core::nodes::access::graph_scan_node::GetVerticesNode;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::types::ContextualExpression;
    use graphdb_core::{Expression, Value, YieldColumn};
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

    fn get_vertices(var: &str) -> PlanNodeEnum {
        let mut node = GetVerticesNode::new(1, "test", var);
        node.set_output_var(var.to_string());
        PlanNodeEnum::GetVertices(node)
    }

    #[test]
    fn test_rule_name_and_pattern() {
        let rule = PushProjectDownGetVerticesRule::new();
        assert_eq!(rule.name(), "PushProjectDownGetVerticesRule");
        assert!(rule.pattern().node.is_some());
    }

    #[test]
    fn test_narrows_get_vertices_projected_properties() {
        let rule = PushProjectDownGetVerticesRule::new();
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![
                    yield_column(prop("v", "age"), "age"),
                    yield_column(prop("v", "name"), "name"),
                ],
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
        let PlanNodeEnum::GetVertices(g) = p.input() else {
            panic!("expected GetVertices below Project");
        };
        assert_eq!(g.projected_properties(), &["age", "name"]);
    }

    #[test]
    fn test_walks_filter_chain() {
        let rule = PushProjectDownGetVerticesRule::new();
        let filter = PlanNodeEnum::Filter(
            FilterNode::new(
                get_vertices("v"),
                contextual(Expression::Binary {
                    left: Box::new(prop("v", "age")),
                    op: graphdb_core::types::operators::BinaryOperator::GreaterThan,
                    right: Box::new(Expression::Literal(Value::Int(30))),
                }),
            )
            .expect("filter node"),
        );
        let project = PlanNodeEnum::Project(
            ProjectNode::new(filter, vec![yield_column(prop("v", "name"), "name")])
                .expect("project node"),
        );

        let result = rule
            .apply(&mut RewriteContext::new(), &project)
            .expect("rewrite should succeed")
            .expect("projection must be narrowed");
        let PlanNodeEnum::Project(p) = &result.new_nodes[0] else {
            panic!("expected Project preserved");
        };
        let PlanNodeEnum::Filter(f) = p.input() else {
            panic!("expected Filter preserved");
        };
        let PlanNodeEnum::GetVertices(g) = f.input() else {
            panic!("expected GetVertices below filter");
        };
        let props = g.projected_properties();
        assert_eq!(props.len(), 2);
        assert!(props.contains(&"age".to_string()));
        assert!(props.contains(&"name".to_string()));
    }

    #[test]
    fn test_bare_variable_blocks_narrowing() {
        let rule = PushProjectDownGetVerticesRule::new();
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
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
        let rule = PushProjectDownGetVerticesRule::new();
        let mut node = GetVerticesNode::new(1, "test", "v");
        node.set_output_var("v".to_string());
        node.set_projected_properties(vec!["age".to_string()]);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::GetVertices(node),
                vec![yield_column(prop("v", "age"), "age")],
            )
            .expect("project node"),
        );

        let result = rule
            .apply(&mut RewriteContext::new(), &project)
            .expect("rewrite should succeed");
        assert!(result.is_none(), "no-op when already narrowed");
    }
}

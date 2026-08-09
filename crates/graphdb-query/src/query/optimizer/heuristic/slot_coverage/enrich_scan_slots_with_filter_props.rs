//! Rule: enrich scan `projected_properties` with filter predicate columns.
//!
//! This rule looks for a `Filter` node sitting directly on top of a scan
//! source (`ScanVertices` / `GetVertices` / `ScanEdges`, possibly with
//! intervening `Filter` nodes), collects every property referenced through
//! an `Expression::Property { object: Variable, .. }` inside the filter
//! conditions, and merges those names into the scan's
//! `projected_properties`.
//!
//! The scan output layout (`source_output_layout`) is therefore widened and
//! the compound slot (`var.prop`) becomes available, so the columnar
//! evaluator can serve the residual predicate without per-row fallback.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Filter(n.value > 1000.0)
//!         |
//!     ScanVertices(projected: [name])
//! ```
//!
//! After:
//! ```text
//! Filter(n.value > 1000.0)
//!         |
//!     ScanVertices(projected: [name, value])
//! ```
//!
//! # Constraints
//!
//! - Only properties whose object is a `Variable` are merged (the compound
//!   slot scheme relies on `{var}.{prop}` names).
//! - Only the residual-filter pattern is handled: `Filter` must be the
//!   direct consumer chain of the scan. Other shapes (e.g. filters above
//!   joins/expands) are left untouched.
//! - The rewrite is a pure no-op when the predicate columns are already
//!   projected, so fixed-point iteration terminates immediately.

use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::Expression;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::{GetVerticesNode, ScanEdgesNode, ScanVerticesNode};
use crate::query::planning::plan::PlanNodeEnum;

/// Collects property names whose object expression is a `Variable`.
#[derive(Debug, Default)]
struct VariableObjectPropertyCollector {
    properties: Vec<String>,
}

impl VariableObjectPropertyCollector {
    fn new() -> Self {
        Self::default()
    }
}

impl ExpressionVisitor for VariableObjectPropertyCollector {
    fn visit_property(&mut self, object: &Expression, property: &str) {
        if matches!(object, Expression::Variable(_)) {
            let name = property.to_string();
            if !self.properties.contains(&name) {
                self.properties.push(name);
            }
        }
    }
}

/// Scan source nodes that carry `projected_properties`.
#[derive(Debug, Clone)]
enum ScanWithProjection {
    ScanVertices(ScanVerticesNode),
    GetVertices(GetVerticesNode),
    ScanEdges(ScanEdgesNode),
}

impl ScanWithProjection {
    fn projected_properties(&self) -> &[String] {
        match self {
            ScanWithProjection::ScanVertices(s) => s.projected_properties(),
            ScanWithProjection::GetVertices(s) => s.projected_properties(),
            ScanWithProjection::ScanEdges(s) => s.projected_properties(),
        }
    }

    fn set_projected_properties(&mut self, properties: Vec<String>) {
        match self {
            ScanWithProjection::ScanVertices(s) => s.set_projected_properties(properties),
            ScanWithProjection::GetVertices(s) => s.set_projected_properties(properties),
            ScanWithProjection::ScanEdges(s) => s.set_projected_properties(properties),
        }
    }

    fn into_plan_node(self) -> PlanNodeEnum {
        match self {
            ScanWithProjection::ScanVertices(s) => PlanNodeEnum::ScanVertices(s),
            ScanWithProjection::GetVertices(s) => PlanNodeEnum::GetVertices(s),
            ScanWithProjection::ScanEdges(s) => PlanNodeEnum::ScanEdges(s),
        }
    }
}

/// Rule that enriches the scan output layout with filter predicate columns.
#[derive(Debug)]
pub struct EnrichScanSlotsWithFilterPropsRule;

impl EnrichScanSlotsWithFilterPropsRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for EnrichScanSlotsWithFilterPropsRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for EnrichScanSlotsWithFilterPropsRule {
    fn name(&self) -> &'static str {
        "EnrichScanSlotsWithFilterPropsRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter")
    }

    fn apply(
        &self,
        _ctx: &mut crate::query::optimizer::heuristic::context::RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let _filter = match node {
            PlanNodeEnum::Filter(f) => f,
            _ => return Ok(None),
        };

        // Walk the filter chain down to the scan source, collecting every
        // filter level along the way (the planner may interleave tag
        // filters between the WHERE filter and the scan).
        let mut chain: Vec<PlanNodeEnum> = Vec::new();
        let mut current = node.clone();
        let mut scan = loop {
            match &current {
                PlanNodeEnum::Filter(f) => {
                    chain.push(current.clone());
                    current = f.input().clone();
                }
                PlanNodeEnum::ScanVertices(s) => {
                    break ScanWithProjection::ScanVertices(s.clone())
                }
                PlanNodeEnum::GetVertices(s) => {
                    break ScanWithProjection::GetVertices(s.clone())
                }
                PlanNodeEnum::ScanEdges(s) => break ScanWithProjection::ScanEdges(s.clone()),
                _ => return Ok(None),
            }
        };

        // Collect new predicate columns (preserving the existing order).
        let mut extra: Vec<String> = Vec::new();
        for level in &chain {
            let PlanNodeEnum::Filter(f) = level else { unreachable!() };
            let Some(meta) = f.condition().expression() else {
                continue;
            };
            let mut collector = VariableObjectPropertyCollector::new();
            ExpressionVisitor::visit(&mut collector, meta.inner());
            for property in collector.properties {
                if !scan.projected_properties().contains(&property) && !extra.contains(&property)
                {
                    extra.push(property);
                }
            }
        }
        if extra.is_empty() {
            return Ok(None);
        }

        let mut properties = scan.projected_properties().to_vec();
        properties.extend(extra);

        scan.set_projected_properties(properties);

        let mut input = scan.into_plan_node();
        for level in chain.into_iter().rev() {
            let mut f = match level {
                PlanNodeEnum::Filter(f) => f,
                _ => unreachable!(),
            };
            f.set_input(input);
            input = PlanNodeEnum::Filter(f);
        }

        let mut result = TransformResult::new();
        result.add_new_node(input);
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::operators::BinaryOperator;
    use crate::core::types::ContextualExpression;
    use crate::core::Value;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
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

    fn filter_node(
        condition: ContextualExpression,
        input: PlanNodeEnum,
    ) -> PlanNodeEnum {
        PlanNodeEnum::Filter(
            FilterNode::new(input, condition).expect("filter node"),
        )
    }

    #[test]
    fn test_rule_name_and_pattern() {
        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        assert_eq!(rule.name(), "EnrichScanSlotsWithFilterPropsRule");
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_enriches_scan_vertices_with_predicate_column() {
        let scan =
            PlanNodeEnum::ScanVertices(ScanVerticesNode::new(0, "default"));
        let condition = Expression::Binary {
            left: Box::new(prop("n", "value")),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Int(1000))),
        };
        let node = filter_node(contextual(condition), scan);

        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite should succeed")
            .expect("some result");
        let new_node = result.new_nodes.first().expect("node");

        let PlanNodeEnum::Filter(filter) = new_node else {
            panic!("expected filter");
        };
        let PlanNodeEnum::ScanVertices(scan) = filter.input() else {
            panic!("expected scan vertices");
        };
        assert_eq!(scan.projected_properties(), &["value"]);
    }

    #[test]
    fn test_expands_existing_properties_and_dedups() {
        let mut scan_node = ScanVerticesNode::new(0, "default");
        scan_node.set_projected_properties(vec!["value".to_string()]);
        let scan = PlanNodeEnum::ScanVertices(scan_node);
        let condition = Expression::Binary {
            left: Box::new(prop("n", "value")),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Binary {
                left: Box::new(prop("n", "name")),
                op: BinaryOperator::Equal,
                right: Box::new(Expression::Literal(Value::string("bob"))),
            }),
        };
        let node = filter_node(contextual(condition), scan);

        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite should succeed")
            .expect("some result");
        let new_node = result.new_nodes.first().expect("node");

        let PlanNodeEnum::Filter(filter) = new_node else {
            panic!("expected filter");
        };
        let PlanNodeEnum::ScanVertices(scan) = filter.input() else {
            panic!("expected scan vertices");
        };
        let props = scan.projected_properties();
        assert_eq!(props.len(), 2);
        assert!(props.contains(&"value".to_string()));
        assert!(props.contains(&"name".to_string()));
    }

    #[test]
    fn test_walks_filter_chain() {
        let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(0, "default"));
        let inner = filter_node(
            contextual(Expression::Binary {
                left: Box::new(prop("n", "age")),
                op: BinaryOperator::GreaterThan,
                right: Box::new(Expression::Literal(Value::Int(18))),
            }),
            scan,
        );
        let node = filter_node(
            contextual(Expression::Binary {
                left: Box::new(prop("n", "name")),
                op: BinaryOperator::Equal,
                right: Box::new(Expression::Literal(Value::string("bob"))),
            }),
            inner,
        );

        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite should succeed")
            .expect("some result");
        let new_node = result.new_nodes.first().expect("node");

        // Both filter levels must survive, and the scan must carry both props.
        let PlanNodeEnum::Filter(outer) = new_node else {
            panic!("expected outer filter");
        };
        let PlanNodeEnum::Filter(inner) = outer.input() else {
            panic!("expected inner filter");
        };
        let PlanNodeEnum::ScanVertices(scan) = inner.input() else {
            panic!("expected scan vertices");
        };
        let props = scan.projected_properties();
        assert!(props.contains(&"age".to_string()));
        assert!(props.contains(&"name".to_string()));
    }

    #[test]
    fn test_enriches_scan_edges() {
        let scan = PlanNodeEnum::ScanEdges(ScanEdgesNode::new(0, "Link"));
        let node = filter_node(
            contextual(Expression::Binary {
                left: Box::new(prop("r", "weight")),
                op: BinaryOperator::GreaterThan,
                right: Box::new(Expression::Literal(Value::Int(1))),
            }),
            scan,
        );

        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite should succeed")
            .expect("some result");
        let new_node = result.new_nodes.first().expect("node");

        let PlanNodeEnum::Filter(filter) = new_node else {
            panic!("expected filter");
        };
        let PlanNodeEnum::ScanEdges(scan) = filter.input() else {
            panic!("expected scan edges");
        };
        assert_eq!(scan.projected_properties(), &["weight"]);
    }

    #[test]
    fn test_noop_when_columns_already_projected() {
        let mut scan_node = ScanVerticesNode::new(0, "default");
        scan_node.set_projected_properties(vec!["value".to_string()]);
        let scan = PlanNodeEnum::ScanVertices(scan_node);
        let node =
            filter_node(contextual(prop("n", "value")), scan);

        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite should succeed");
        assert!(result.is_none());
    }

    #[test]
    fn test_ignores_non_variable_objects() {
        let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(0, "default"));
        // Property whose object is not a Variable must not be merged.
        let condition = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Property {
                    object: Box::new(Expression::Variable("n".to_string())),
                    property: "inner".to_string(),
                }),
                property: "outer".to_string(),
            }),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::Int(1))),
        };
        let node = filter_node(contextual(condition), scan);

        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite should succeed");
        assert!(result.is_none());
    }

    #[test]
    fn test_skips_non_scan_input() {
        let input = PlanNodeEnum::Project(
            crate::query::planning::plan::core::nodes::ProjectNode::new(
                PlanNodeEnum::ScanVertices(ScanVerticesNode::new(0, "default")),
                Vec::new(),
            )
            .expect("project node"),
        );
        let node = filter_node(
            contextual(Expression::Variable("x".to_string())),
            input,
        );

        let rule = EnrichScanSlotsWithFilterPropsRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite should succeed");
        assert!(result.is_none());
    }
}
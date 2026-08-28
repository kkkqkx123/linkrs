//! Rule that removes tag-membership conjuncts which are already implied by
//! the scan's tag.
//!
//! The planner emits `contains(labels(var), 'Tag')` as a regular `Filter`
//! conjunct above a `ScanVertices(tag = Tag)`.  The scan only ever produces
//! vertices of that tag, so the conjunct is logically redundant — keeping it
//! in the residual filter forces the expression evaluator to fall back to
//! per-row evaluation (labels are not columnar slots), which defeats the
//! columnar fast path.
//!
//! The rule rewrites `Filter(ScanVertices)` by dropping every conjunct that
//! matches `contains(labels(var), literal)` with `literal` equal to the
//! scan's tag and `var` equal to the scan's emitted variable.  If no
//! conjunct remains, the whole Filter node is erased.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Filter(contains(labels(n), 'Node') AND n.value > 1000)
//!         |
//!     ScanVertices(Node)
//! ```
//!
//! After:
//! ```text
//! Filter(n.value > 1000)
//!         |
//!     ScanVertices(Node)
//! ```

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::scan_predicate::and_of;
use graphdb_core::types::expr::ContextualExpression;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::types::Expression;
use graphdb_core::Value;

/// Rule that drops tag-membership conjuncts already implied by the scan's tag.
#[derive(Debug)]
pub struct EliminateRedundantTagFilterRule;

impl EliminateRedundantTagFilterRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for EliminateRedundantTagFilterRule {
    fn default() -> Self {
        Self::new()
    }
}

/// Match `contains(labels(var), 'tag')`.
fn tag_membership(expr: &Expression) -> Option<(String, String)> {
    let Expression::Function { name, args } = expr else {
        return None;
    };
    if name != "contains" || args.len() != 2 {
        return None;
    }
    let Expression::Function {
        name: labels_name,
        args: labels_args,
    } = &args[0]
    else {
        return None;
    };
    if labels_name != "labels" || labels_args.len() != 1 {
        return None;
    }
    let Expression::Variable(var) = &labels_args[0] else {
        return None;
    };
    let Expression::Literal(Value::String(tag)) = &args[1] else {
        return None;
    };
    Some((var.clone(), tag.to_string()))
}

impl RewriteRule for EliminateRedundantTagFilterRule {
    fn name(&self) -> &'static str {
        "EliminateRedundantTagFilterRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("ScanVertices")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let filter = match node {
            PlanNodeEnum::Filter(f) => f,
            _ => return Ok(None),
        };
        let scan = match filter.input() {
            PlanNodeEnum::ScanVertices(s) => s,
            _ => return Ok(None),
        };
        let Some(tag) = scan.tag() else {
            return Ok(None);
        };
        let Some(meta) = filter.condition().expression() else {
            return Ok(None);
        };

        // Collect the conjuncts that are implied by the scan's tag.
        let mut remaining: Vec<Expression> = Vec::new();
        let mut removed = 0usize;
        for conjunct in split_conjuncts(meta.inner()) {
            if let Some((var, maybe_tag)) = tag_membership(conjunct) {
                if scan.output_var() == Some(var.as_str()) && maybe_tag == *tag {
                    removed += 1;
                    continue;
                }
            }
            remaining.push(conjunct.clone());
        }
        if removed == 0 {
            return Ok(None);
        }

        let mut result = TransformResult::new();
        if remaining.is_empty() {
            // All conjuncts are redundant: the filter is a no-op above the
            // tagged scan.
            result.erase_curr = true;
            result.add_new_node(filter.input().clone());
            return Ok(Some(result));
        }
        let Some(new_expr) = and_of(remaining) else {
            return Ok(None);
        };
        let ctx = filter.condition().context().clone();
        let new_id = ctx.register_expression(ExpressionMeta::new(new_expr));
        let new_condition = ContextualExpression::new(new_id, ctx);
        let mut new_filter = filter.clone();
        new_filter.set_condition(new_condition);
        result.add_new_node(PlanNodeEnum::Filter(new_filter));
        Ok(Some(result))
    }
}

impl crate::optimizer::heuristic::rule::PushDownRule for EliminateRedundantTagFilterRule {
    fn can_push_down(&self, node: &PlanNodeEnum, _target: &PlanNodeEnum) -> bool {
        matches!(node, PlanNodeEnum::Filter(_))
    }

    fn push_down(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
        _target: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(ctx, node)
    }
}

/// Split an expression into top-level `AND` conjuncts.
fn split_conjuncts(expr: &Expression) -> Vec<&Expression> {
    let mut conjuncts = Vec::new();
    collect_conjuncts(expr, &mut conjuncts);
    conjuncts
}

fn collect_conjuncts<'a>(expr: &'a Expression, out: &mut Vec<&'a Expression>) {
    if let Expression::Binary {
        left,
        op: BinaryOperator::And,
        right,
    } = expr
    {
        collect_conjuncts(left, out);
        collect_conjuncts(right, out);
    } else {
        out.push(expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::ScanVerticesNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::ContextualExpression;
    use std::sync::Arc;

    fn contextual(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    fn scan_with_tag() -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "default");
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());
        scan.set_tag("Node");
        PlanNodeEnum::ScanVertices(scan)
    }

    fn filter(condition: Expression, input: PlanNodeEnum) -> PlanNodeEnum {
        PlanNodeEnum::Filter(FilterNode::new(input, contextual(condition)).expect("filter node"))
    }

    fn labels_conjunct() -> Expression {
        Expression::Function {
            name: "contains".to_string(),
            args: vec![
                Expression::Function {
                    name: "labels".to_string(),
                    args: vec![Expression::Variable("n".to_string())],
                },
                Expression::Literal(Value::string("Node")),
            ],
        }
    }

    fn predicate() -> Expression {
        Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "value".to_string(),
            }),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Double(2.0))),
        }
    }

    fn pushdown(
        rule: &EliminateRedundantTagFilterRule,
        node: &PlanNodeEnum,
    ) -> Option<TransformResult> {
        rule.apply(&mut RewriteContext::new(), node).expect("apply")
    }

    #[test]
    fn rule_name() {
        assert_eq!(
            EliminateRedundantTagFilterRule::new().name(),
            "EliminateRedundantTagFilterRule"
        );
    }

    #[test]
    fn drops_matching_tag_conjunct_but_keeps_filter() {
        let rule = EliminateRedundantTagFilterRule::new();
        let condition = Expression::Binary {
            left: Box::new(labels_conjunct()),
            op: BinaryOperator::And,
            right: Box::new(predicate()),
        };
        let node = filter(condition, scan_with_tag());

        let result = pushdown(&rule, &node).expect("some result");
        let PlanNodeEnum::Filter(new_filter) = &result.new_nodes[0] else {
            panic!("expected filter");
        };
        let expr = new_filter.condition().expression().expect("expr");
        assert_eq!(
            expr.inner(),
            &predicate(),
            "the redundant tag conjunct must be removed"
        );
    }

    #[test]
    fn erases_filter_when_all_conjuncts_dropped() {
        let rule = EliminateRedundantTagFilterRule::new();
        let node = filter(labels_conjunct(), scan_with_tag());

        let result = rule
            .apply(&mut RewriteContext::new(), &node)
            .expect("apply")
            .expect("some result");
        assert!(result.erase_curr);
        assert_eq!(result.new_nodes.len(), 1);
    }

    #[test]
    fn skips_mismatched_tag_and_var() {
        let rule = EliminateRedundantTagFilterRule::new();
        // Tag on the scan differs from the conjunct's tag.
        let mut other = ScanVerticesNode::new(1, "default");
        other.set_col_names(vec!["n".to_string()]);
        other.set_output_var("n".to_string());
        other.set_tag("Place");
        let node = filter(labels_conjunct(), PlanNodeEnum::ScanVertices(other));
        assert!(pushdown(&rule, &node).is_none());
    }

    #[test]
    fn skips_tag_conjunct_on_mismatched_variable() {
        let rule = EliminateRedundantTagFilterRule::new();
        let mut scan = ScanVerticesNode::new(1, "default");
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());
        scan.set_tag("Node");
        let condition = Expression::Function {
            name: "contains".to_string(),
            args: vec![
                Expression::Function {
                    name: "labels".to_string(),
                    args: vec![Expression::Variable("m".to_string())],
                },
                Expression::Literal(Value::string("Node")),
            ],
        };
        let node = filter(condition, PlanNodeEnum::ScanVertices(scan));
        assert!(pushdown(&rule, &node).is_none());
    }
}

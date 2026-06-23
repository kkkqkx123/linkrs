//! Rules for eliminating the "always false" filter operation
//!
//! 根据 nebula-graph 的参考实现，此规则处理 Filter(false) 或 Filter(null) 的情况，
//! Replace it with a ValueNode that returns an empty set.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   Filter(false)
//!       |
//!   ScanVertices
//! ```
//!
//! After:
//! ```text
//!   Value(空集)
//!       |
//!   Start
//! ```
//!
//! # Applicable Conditions
//!
//! The filtering condition is a permanently false value (such as FALSE, null, etc.).

use crate::core::types::operators::BinaryOperator;
use crate::core::{Expression, Value};
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{EliminationRule, RewriteRule};
use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for eliminating the "always false" filtering operation
///
/// When the condition of the Filter node is false or null, it should be directly replaced with a node that returns an empty set.
#[derive(Debug)]
pub struct EliminateFilterRule;

impl EliminateFilterRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }

    /// Check whether the expression is a permanently false value (false or null).
    fn is_contradiction(&self, expression: &Expression) -> bool {
        match expression {
            // Boolean literal: false
            Expression::Literal(Value::Bool(false)) => true,
            // The “null” value
            Expression::Literal(Value::Null(_)) => true,
            // Binary expression: Checks expressions of the form 1 = 0 or 0 = 1, etc.
            Expression::Binary { left, op, right } => {
                match (left.as_ref(), op, right.as_ref()) {
                    // 1 = 0 or 0 = 1
                    (
                        Expression::Literal(Value::Int(1)),
                        BinaryOperator::Equal,
                        Expression::Literal(Value::Int(0)),
                    ) => true,
                    (
                        Expression::Literal(Value::Int(0)),
                        BinaryOperator::Equal,
                        Expression::Literal(Value::Int(1)),
                    ) => true,
                    // a != a
                    (
                        Expression::Variable(a),
                        BinaryOperator::NotEqual,
                        Expression::Variable(b),
                    ) if a == b => true,
                    // a = b, where a and b are different constants.
                    (Expression::Literal(a), BinaryOperator::Equal, Expression::Literal(b))
                        if a != b =>
                    {
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

impl Default for EliminateFilterRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for EliminateFilterRule {
    fn name(&self) -> &'static str {
        "EliminateFilterRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a Filter node.
        let filter_node = match node {
            PlanNodeEnum::Filter(n) => n,
            _ => return Ok(None),
        };

        // Obtain the filtering criteria
        let condition = filter_node.condition();

        // Obtain the underlying expressions
        let expr = match condition.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };

        // Check whether the condition is a permanently false statement.
        if !self.is_contradiction(&expr) {
            return Ok(None);
        }

        // Replace the current Filter node with the StartNode.
        // Referencing the implementation of nebula-graph, return an empty set.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::Start(StartNode::new()));

        Ok(Some(result))
    }
}

impl EliminationRule for EliminateFilterRule {
    fn can_eliminate(&self, node: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Filter(n) => {
                let condition = n.condition();
                match condition.expression() {
                    Some(meta) => self.is_contradiction(meta.inner()),
                    None => false,
                }
            }
            _ => false,
        }
    }

    fn eliminate(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_eliminate_filter_rule_name() {
        let rule = EliminateFilterRule::new();
        assert_eq!(rule.name(), "EliminateFilterRule");
    }

    #[test]
    fn test_eliminate_filter_rule_pattern() {
        let rule = EliminateFilterRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_contradiction() {
        let rule = EliminateFilterRule::new();

        // Test: false
        assert!(rule.is_contradiction(&Expression::Literal(Value::Bool(false))));

        // Test whether it is true (it is not a永假式, i.e., a statement that is always false).
        assert!(!rule.is_contradiction(&Expression::Literal(Value::Bool(true))));

        // Testing null
        assert!(rule.is_contradiction(&Expression::Literal(Value::Null(
            crate::core::value::NullType::Null
        ))));

        // Test 1 = 0
        assert!(rule.is_contradiction(&Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::Int(0))),
        }));

        // Test 1 = 1 (not a永假式, i.e., not a statement that is always false)
        assert!(!rule.is_contradiction(&Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::Int(1))),
        }));
    }
}

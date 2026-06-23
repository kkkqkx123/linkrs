//! Rules that push the edge filtering conditions to the Traverse node
//!
//! This rule identifies the eFilter within the Traverse node.
//! Rewrite it as a specific expression for the edge attributes.

use crate::core::types::expr::visitor_checkers::WildcardReplacer;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Rules that push the edge filtering conditions to the Traverse node
///
/// # Conversion example
///
/// Before:
/// ```text
///   Traverse(eFilter: *.likeness > 78)
/// ```
///
/// After:
/// ```text
///   Traverse(filter: e.likeness > 78)
/// ```
///
/// # Applicable Conditions
///
/// The Traverse node contains an eFilter.
/// The eFilter contains wildcard-edge attribute expressions.
/// A non-zero-step traversal of the “ Traverse” structure.
#[derive(Debug)]
pub struct PushEFilterDownRule;

impl PushEFilterDownRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushEFilterDownRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushEFilterDownRule {
    fn name(&self) -> &'static str {
        "PushEFilterDownRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Traverse")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a Traverse node.
        let traverse = match node {
            PlanNodeEnum::Traverse(t) => t,
            _ => return Ok(None),
        };

        // Obtain the eFilter
        let e_filter = match traverse.e_filter() {
            Some(filter) => filter,
            None => return Ok(None),
        };

        // Obtaining edge aliases
        let edge_alias = match traverse.edge_alias() {
            Some(alias) => alias.clone(),
            None => return Ok(None),
        };

        // Obtain the underlying expressions
        let e_expr = match e_filter.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };

        // Rewrite the expression by replacing the wildcard with a specific edge alias.
        let rewritten_expr = rewrite_wildcard_to_alias(&e_expr, &edge_alias);

        // Obtain the context
        let ctx = e_filter.context().clone();

        // Register the rewritten expression in the context.
        let rewritten_meta = crate::core::types::ExpressionMeta::new(rewritten_expr);
        let rewritten_id = ctx.register_expression(rewritten_meta);
        let rewritten_filter = crate::core::types::ContextualExpression::new(rewritten_id, ctx);

        // Create a new Traverse node.
        let mut new_traverse = traverse.clone();

        // Set up a new eFilter
        new_traverse.set_e_filter(rewritten_filter);

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::Traverse(new_traverse));

        Ok(Some(result))
    }
}

impl PushDownRule for PushEFilterDownRule {
    fn can_push_down(&self, node: &PlanNodeEnum, _target: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Traverse(traverse) => {
                traverse.e_filter().is_some() && traverse.min_steps() > 0
            }
            _ => false,
        }
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

/// Replace the wildcards in the expression with specific edge aliases.
///
/// Wildcards are usually represented by `*` or `_` and indicate any edge when accessing attributes.
fn rewrite_wildcard_to_alias(expr: &Expression, edge_alias: &str) -> Expression {
    let replacer = WildcardReplacer::new(edge_alias);
    replacer.replace(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_rule_name() {
        let rule = PushEFilterDownRule::new();
        assert_eq!(rule.name(), "PushEFilterDownRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushEFilterDownRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_rewrite_wildcard_to_alias() {
        // Testing the access to wildcard attribute properties
        let wildcard_expr = Expression::Property {
            object: Box::new(Expression::Variable("*".to_string())),
            property: "likeness".to_string(),
        };

        let rewritten = rewrite_wildcard_to_alias(&wildcard_expr, "e");

        match rewritten {
            Expression::Property { object, property } => {
                assert_eq!(property, "likeness");
                match object.as_ref() {
                    Expression::Variable(name) => assert_eq!(name, "e"),
                    _ => panic!("Expected variable expression"),
                }
            }
            _ => panic!("Expected attribute expression"),
        }
    }

    #[test]
    fn test_rewrite_binary_expr() {
        // Testing for wildcards in binary expressions
        let binary_expr = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("*".to_string())),
                property: "likeness".to_string(),
            }),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(78.into())),
        };

        let rewritten = rewrite_wildcard_to_alias(&binary_expr, "e");

        match rewritten {
            Expression::Binary { left, op, right } => {
                assert!(matches!(op, BinaryOperator::GreaterThan));
                match left.as_ref() {
                    Expression::Property { object, property } => {
                        assert_eq!(property, "likeness");
                        match object.as_ref() {
                            Expression::Variable(name) => assert_eq!(name, "e"),
                            _ => panic!("Expected variable expression"),
                        }
                    }
                    _ => panic!("Expected attribute expression"),
                }
                match right.as_ref() {
                    Expression::Literal(val) => assert_eq!(val, &78.into()),
                    _ => panic!("Expected literal expression"),
                }
            }
            _ => panic!("Expected binary expression"),
        }
    }
}

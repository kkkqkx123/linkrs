//! The rule that pushes the vertex filtering conditions to the ScanVertices node
//!
//! This rule identifies the vFilter within the Traverse node.
//! And rewrite it as a specific expression for the vertex attributes.

use crate::core::types::expr::visitor_checkers::WildcardReplacer;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// The rule that pushes the vertex filtering conditions to the ScanVertices node
///
/// # Translation example
///
/// Before:
/// ```text
///   Traverse(vFilter: *.age > 18)
/// ```
///
/// After:
/// ```text
///   Traverse(filter: v.age > 18)
/// ```
///
/// # Applicable Conditions
///
/// The Traverse node contains a vFilter.
/// The `vFilter` contains wildcard vertex attribute expressions.
/// A non-zero-step traversal of the “ Traverse” structure.
#[derive(Debug)]
pub struct PushVFilterDownScanVerticesRule;

impl PushVFilterDownScanVerticesRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushVFilterDownScanVerticesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushVFilterDownScanVerticesRule {
    fn name(&self) -> &'static str {
        "PushVFilterDownScanVerticesRule"
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

        // Obtain the vFilter
        let v_filter = match traverse.v_filter() {
            Some(filter) => filter,
            None => return Ok(None),
        };

        // Obtaining vertex aliases
        let vertex_alias = match traverse.vertex_alias() {
            Some(alias) => alias.clone(),
            None => return Ok(None),
        };

        // Obtain the underlying expressions
        let v_expr = match v_filter.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };

        // Rewrite the expression by replacing the wildcard with a specific vertex alias.
        let rewritten_expr = rewrite_wildcard_to_alias(&v_expr, &vertex_alias);

        // Obtain the context.
        let ctx = v_filter.context().clone();

        // Register the rewritten expression in the context.
        let rewritten_meta = crate::core::types::ExpressionMeta::new(rewritten_expr);
        let rewritten_id = ctx.register_expression(rewritten_meta);
        let rewritten_filter = crate::core::types::ContextualExpression::new(rewritten_id, ctx);

        // Create a new Traverse node.
        let mut new_traverse = traverse.clone();

        // Setting a new vFilter
        new_traverse.set_v_filter(rewritten_filter);

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::Traverse(new_traverse));

        Ok(Some(result))
    }
}

impl PushDownRule for PushVFilterDownScanVerticesRule {
    fn can_push_down(&self, node: &PlanNodeEnum, _target: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Traverse(traverse) => {
                traverse.v_filter().is_some() && traverse.min_steps() > 0
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

/// Replace the wildcards in the expression with specific vertex aliases.
fn rewrite_wildcard_to_alias(expr: &Expression, vertex_alias: &str) -> Expression {
    let replacer = WildcardReplacer::new(vertex_alias);
    replacer.replace(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_rule_name() {
        let rule = PushVFilterDownScanVerticesRule::new();
        assert_eq!(rule.name(), "PushVFilterDownScanVerticesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushVFilterDownScanVerticesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_rewrite_wildcard_to_alias() {
        // Testing the access to wildcard attribute properties
        let wildcard_expr = Expression::Property {
            object: Box::new(Expression::Variable("*".to_string())),
            property: "age".to_string(),
        };

        let rewritten = rewrite_wildcard_to_alias(&wildcard_expr, "v");

        match rewritten {
            Expression::Property { object, property } => {
                assert_eq!(property, "age");
                match object.as_ref() {
                    Expression::Variable(name) => assert_eq!(name, "v"),
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
                property: "age".to_string(),
            }),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(18.into())),
        };

        let rewritten = rewrite_wildcard_to_alias(&binary_expr, "v");

        match rewritten {
            Expression::Binary { left, op, right } => {
                assert!(matches!(op, BinaryOperator::GreaterThan));
                match left.as_ref() {
                    Expression::Property { object, property } => {
                        assert_eq!(property, "age");
                        match object.as_ref() {
                            Expression::Variable(name) => assert_eq!(name, "v"),
                            _ => panic!("Expected variable expression"),
                        }
                    }
                    _ => panic!("Expected attribute expression"),
                }
                match right.as_ref() {
                    Expression::Literal(val) => assert_eq!(val, &18.into()),
                    _ => panic!("Expected literal expression"),
                }
            }
            _ => panic!("Expected binary expression"),
        }
    }
}

//! Push the filtering conditions up to the rules that are used in the traversal operation.
//!
//! This rule identifies the Filter -> Traverse mode.
//! And push the filtering conditions for the edge attributes down to the Traverse node.

use crate::core::types::expr::{ExpressionVisitor, VariableCollector};
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::expression_utils::split_filter;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Push the filtering conditions up to the rules that are used in the traversal operation.
///
/// # Conversion example
///
/// Before:
/// ```text
///   Filter(e.likeness > 78)
///           |
///   AppendVertices
///           |
///   Traverse
/// ```
///
/// After:
/// ```text
///   AppendVertices
///           |
///   Traverse(eFilter: *.likeness > 78)
/// ```
///
/// # Applicable Conditions
///
/// The filtering criteria include edge attribute expressions.
/// The Traverse node is used for single-step traversal.
#[derive(Debug)]
pub struct PushFilterDownTraverseRule;

impl PushFilterDownTraverseRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushFilterDownTraverseRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownTraverseRule {
    fn name(&self) -> &'static str {
        "PushFilterDownTraverseRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("Traverse")
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

        // Obtain the input node
        let input = filter_node.input();

        // Check whether the input node is of the Traverse type.
        let traverse = match input {
            PlanNodeEnum::Traverse(t) => t,
            _ => return Ok(None),
        };

        // Check whether it is a single-step iteration.
        if !traverse.is_one_step() {
            return Ok(None);
        }

        // Obtaining edge aliases
        let edge_alias = match traverse.edge_alias() {
            Some(alias) => alias,
            None => return Ok(None),
        };

        // Obtain the filtering criteria
        let filter_condition = filter_node.condition();

        // Obtaining the context is necessary for creating a ContextualExpression.
        let ctx = filter_condition.context().clone();

        // Define a selector function: Check whether an expression contains edge attributes.
        let picker = |expr: &Expression| -> bool { is_edge_property_expression(edge_alias, expr) };

        // Split filter criteria
        let (filter_picked, filter_unpicked) = split_filter(filter_condition, picker);

        // If there are no available options to choose from, then no conversion will be performed.
        let picked = match filter_picked {
            Some(f) => f,
            None => return Ok(None),
        };

        // Create a new Traverse node.
        let mut new_traverse = traverse.clone();

        // Set up or merge the eFilter.
        if let Some(existing_ctx) = traverse.e_filter() {
            // Combining expressions using the and method of ExpressionContext
            if let Some(combined_ctx_expr) = ctx.and(&picked, existing_ctx) {
                new_traverse.set_e_filter(combined_ctx_expr);
            }
        } else {
            // Clone the expression and set it accordingly.
            if let Some(picked_ctx_expr) = ctx.clone_expression(&picked) {
                new_traverse.set_e_filter(picked_ctx_expr);
            }
        }

        // Construct the translation result.
        let mut result = TransformResult::new();

        // If there are any filter criteria that have not been selected, retain the Filter node.
        if let Some(unpicked) = filter_unpicked {
            result.erase_curr = false;
            // Update the conditions for the Filter node.
            let mut new_filter = filter_node.clone();
            // Clone the expression and set it accordingly.
            if let Some(unpicked_ctx_expr) = ctx.clone_expression(&unpicked) {
                new_filter.set_condition(unpicked_ctx_expr);
            }
            result.add_new_node(PlanNodeEnum::Filter(new_filter));
        } else {
            // Push everything completely down; remove the Filter node.
            result.erase_curr = true;
        }

        result.add_new_node(PlanNodeEnum::Traverse(new_traverse));

        Ok(Some(result))
    }
}

impl PushDownRule for PushFilterDownTraverseRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        match (node, target) {
            (PlanNodeEnum::Filter(_), PlanNodeEnum::Traverse(traverse)) => {
                traverse.is_one_step() && traverse.edge_alias().is_some()
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

/// Check whether the expression is an edge attribute expression.
fn is_edge_property_expression(edge_alias: &str, expr: &Expression) -> bool {
    let mut collector = VariableCollector::new();
    ExpressionVisitor::visit(&mut collector, expr);
    collector.variables.contains(&edge_alias.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_name() {
        let rule = PushFilterDownTraverseRule::new();
        assert_eq!(rule.name(), "PushFilterDownTraverseRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushFilterDownTraverseRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_edge_property_expression() {
        let edge_alias = "e";

        // Testing edge attribute expressions
        let prop_expr = Expression::Property {
            object: Box::new(Expression::Variable("e".to_string())),
            property: "likeness".to_string(),
        };
        assert!(is_edge_property_expression(edge_alias, &prop_expr));

        // Testing non-edge attribute expressions
        let var_expr = Expression::Variable("v".to_string());
        assert!(!is_edge_property_expression(edge_alias, &var_expr));
    }
}

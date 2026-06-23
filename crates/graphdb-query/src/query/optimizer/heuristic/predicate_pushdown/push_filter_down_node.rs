//! Rules that push the filtering conditions to the Traverse/AppendVertices node
//!
//! This rule identifies the `vFilter` element within the Traverse/AppendVertices nodes.
//! And push the filter conditions that can be applied “downward” (i.e., applied to the data at a lower level in the system) back to the data source.

use crate::core::types::ContextualExpression;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::expression_utils::{check_col_name, split_filter};
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::traversal::traversal_node::TraverseNode;
use crate::query::planning::plan::core::nodes::AppendVerticesNode;

/// Rules that push the filtering conditions to the Traverse/AppendVertices node
///
/// # Conversion example
///
/// Before:
/// ```text
///   Traverse(vFilter: v.age > 18)
/// ```
///
/// After:
/// ```text
///   Traverse(vFilter: <remained>, firstStepFilter: v.age > 18)
/// ```
///
/// # Applicable Conditions
///
/// The Traverse or AppendVertices node has a vFilter attribute.
/// The `vFilter` component can be partially delegated (or “pushed down”) to the `firstStepFilter` component.
#[derive(Debug)]
pub struct PushFilterDownNodeRule;

impl PushFilterDownNodeRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushFilterDownNodeRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownNodeRule {
    fn name(&self) -> &'static str {
        "PushFilterDownNodeRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Traverse")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            PlanNodeEnum::Traverse(traverse) => self.apply_to_traverse(traverse),
            PlanNodeEnum::AppendVertices(append) => self.apply_to_append_vertices(append),
            _ => Ok(None),
        }
    }
}

impl PushFilterDownNodeRule {
    /// Apply to the Traverse node
    fn apply_to_traverse(&self, traverse: &TraverseNode) -> RewriteResult<Option<TransformResult>> {
        // Check whether a vFilter exists.
        let v_filter = match traverse.v_filter() {
            Some(filter) => filter,
            None => return Ok(None),
        };

        // Obtaining column names is used to determine the expressions that can be pushed down (i.e., processed further in the system).
        let col_names = traverse.col_names().to_vec();

        // Define a selector: Check whether the expression only refers to the columns of the current node.
        let picker = |expr: &Expression| -> bool { check_col_name(&col_names, expr) };

        // Split filter criteria
        let (filter_picked, filter_remained) = split_filter(v_filter, picker);

        // If there are no conditions that allow for the transformation to be carried out, then no conversion will take place.
        let picked = match filter_picked {
            Some(f) => f,
            None => return Ok(None),
        };

        // Create a new Traverse node.
        let mut new_traverse = traverse.clone();

        // Obtaining the context is necessary for creating a ContextualExpression.
        let ctx = v_filter.context().clone();

        // Set the firstStepFilter
        if let Some(existing) = traverse.first_step_filter() {
            // Combining expressions using the and method of ExpressionContext
            if let Some(combined_ctx_expr) = ctx.and(&picked, existing) {
                new_traverse.set_first_step_filter(combined_ctx_expr);
            }
        } else {
            // Clone the expression and set it accordingly.
            if let Some(picked_ctx_expr) = ctx.clone_expression(&picked) {
                new_traverse.set_first_step_filter(picked_ctx_expr);
            }
        }

        // Update vFilter
        if let Some(remained) = filter_remained {
            // Clone the expression and set it accordingly.
            if let Some(remained_ctx_expr) = ctx.clone_expression(&remained) {
                new_traverse.set_v_filter(remained_ctx_expr);
            }
        } else {
            new_traverse.set_v_filter(ContextualExpression::new(
                crate::core::types::expr::ExpressionId::new(0),
                ctx,
            ));
        }

        // Build the conversion result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::Traverse(new_traverse));

        Ok(Some(result))
    }

    /// Apply to the AppendVertices node
    fn apply_to_append_vertices(
        &self,
        append: &AppendVerticesNode,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether the vFilter exists.
        let v_filter = match append.v_filter() {
            Some(filter) => filter,
            None => return Ok(None),
        };

        // Obtaining the column names is necessary to determine the expressions that can be pushed down (i.e., processed at a lower level of the system).
        let col_names = append.col_names().to_vec();

        // Define a selector: Check whether the expression only refers to the columns of the current node.
        let picker = |expr: &Expression| -> bool { check_col_name(&col_names, expr) };

        // Split filter criteria
        let (filter_picked, filter_remained) = split_filter(v_filter, picker);

        // If there are no conditions that allow for the transformation to be carried out, then no conversion will take place.
        let picked = match filter_picked {
            Some(f) => f,
            None => return Ok(None),
        };

        // Create a new AppendVertices node.
        let mut new_append = append.clone();

        // Obtaining the context is necessary for creating a ContextualExpression.
        let ctx = v_filter.context().clone();

        // Set the filter
        if let Some(existing) = append.filter() {
            // Combining expressions using the and method of ExpressionContext
            if let Some(combined_ctx_expr) = ctx.and(&picked, existing) {
                new_append.set_filter(combined_ctx_expr);
            }
        } else {
            // Clone the expression and set it accordingly.
            if let Some(picked_ctx_expr) = ctx.clone_expression(&picked) {
                new_append.set_filter(picked_ctx_expr);
            }
        }

        // Update vFilter
        if let Some(remained) = filter_remained {
            // Clone the expression and set it accordingly.
            if let Some(remained_ctx_expr) = ctx.clone_expression(&remained) {
                new_append.set_v_filter(remained_ctx_expr);
            }
        }

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::AppendVertices(new_append));

        Ok(Some(result))
    }
}

impl PushDownRule for PushFilterDownNodeRule {
    fn can_push_down(&self, node: &PlanNodeEnum, _target: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::Traverse(_) | PlanNodeEnum::AppendVertices(_)
        )
    }

    fn push_down(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
        _target: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(_ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_name() {
        let rule = PushFilterDownNodeRule::new();
        assert_eq!(rule.name(), "PushFilterDownNodeRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushFilterDownNodeRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}

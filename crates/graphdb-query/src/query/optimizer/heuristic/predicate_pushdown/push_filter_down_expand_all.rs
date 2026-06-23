//! Rules that push the filtering conditions to the ExpandAll operation
//!
//! This rule identifies the "Filter -> ExpandAll" mode.
//! And push the filtering criteria up to the ExpandAll node.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    PlanNode, SingleInputNode,
};
use crate::query::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;

/// Rules that push the filtering criteria forward to the ExpandAll operation
///
/// # Conversion example
///
/// Before:
/// ```text
///   Filter(e.likeness > 78)
///           |
///   ExpandAll
/// ```
///
/// After:
/// ```text
///   ExpandAll(filter: e.likeness > 78)
/// ```
///
/// # Applicable Conditions
///
/// The “ExpandAll” node is used to retrieve the properties of the edges.
/// The minimum step size for “ExpandAll” is equal to the maximum step size.
/// The filtering criteria can be pushed down to the storage layer.
#[derive(Debug)]
pub struct PushFilterDownExpandAllRule;

impl PushFilterDownExpandAllRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushFilterDownExpandAllRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownExpandAllRule {
    fn name(&self) -> &'static str {
        "PushFilterDownExpandAllRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("ExpandAll")
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

        // Check whether the input node is of the ExpandAll type.
        let expand_all = match input {
            PlanNodeEnum::ExpandAll(n) => n,
            _ => return Ok(None),
        };

        // Obtain the filtering criteria
        let filter_condition = filter_node.condition();

        // Check if the filter references columns that are available in the ExpandAll's output
        // This is important for multi-hop MATCH queries where a filter on an earlier variable
        // should not be pushed down to a later ExpandAll that doesn't produce that variable
        if !Self::can_push_filter_to_expand(filter_condition, expand_all) {
            return Ok(None);
        }

        // Create a new ExpandAll node.
        let mut new_expand_all = expand_all.clone();

        // Set the filter
        new_expand_all.set_filter(filter_condition.clone());

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::ExpandAll(new_expand_all));

        Ok(Some(result))
    }
}

impl PushFilterDownExpandAllRule {
    /// Check if a filter can be pushed down to an ExpandAll node.
    ///
    /// The filter can only be pushed down if all variables it references
    /// are available in the ExpandAll's output columns.
    fn can_push_filter_to_expand(
        filter_condition: &crate::core::types::ContextualExpression,
        expand_all: &ExpandAllNode,
    ) -> bool {
        // Get the expression from the contextual expression
        let Some(expression) = filter_condition.get_expression() else {
            // If we can't get the expression, don't push down
            return false;
        };

        // Get all variables referenced in the filter
        let referenced_vars = expression.get_variables();

        // If no variables are referenced, we can push down
        if referenced_vars.is_empty() {
            return true;
        }

        // Get the output columns of the ExpandAll
        let output_cols = expand_all.col_names();

        // Check if all referenced variables are in the output columns
        // Also allow special variables like "$$", "$^", "edge", "src", "dst"
        let special_vars = ["$$", "$^", "edge", "src", "dst", "target"];

        for var in &referenced_vars {
            // Skip special variables that are handled specially
            if special_vars.contains(&var.as_str()) {
                continue;
            }
            // Check if the variable is in the output columns
            if !output_cols.contains(var) {
                return false;
            }
        }

        true
    }
}

impl PushDownRule for PushFilterDownExpandAllRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(
            (node, target),
            (PlanNodeEnum::Filter(_), PlanNodeEnum::ExpandAll(_))
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;

    #[test]
    fn test_rule_name() {
        let rule = PushFilterDownExpandAllRule::new();
        assert_eq!(rule.name(), "PushFilterDownExpandAllRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushFilterDownExpandAllRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_can_push_down() {
        let rule = PushFilterDownExpandAllRule::new();
        use crate::query::validator::context::ExpressionAnalysisContext;
        use std::sync::Arc;

        let start = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start);

        let condition = Expression::Variable("test".to_string());
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(condition);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);
        let filter =
            crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
                start_enum.clone(),
                ctx_expr,
            )
            .expect("Failed to create FilterNode");
        let filter_enum = PlanNodeEnum::Filter(filter);

        let expand_all = ExpandAllNode::new(1, vec![], "OUT");
        let expand_enum = PlanNodeEnum::ExpandAll(expand_all);

        assert!(rule.can_push_down(&filter_enum, &expand_enum));
    }
}

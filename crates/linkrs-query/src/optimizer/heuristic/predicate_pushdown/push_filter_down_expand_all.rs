//! Rules that push the filtering conditions to the ExpandAll operation
//!
//! This rule identifies the "Filter -> ExpandAll" mode.
//! And push the filtering criteria up to the ExpandAll node.

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNode, SingleInputNode,
};
use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;

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

        // If the filter references only the anchor (input) columns, push it
        // BELOW the expand so only matching anchor vertices are expanded.
        // Example: `WHERE a.value < 100` on `(a)-[:R]->(b)` should filter the
        // anchor scan, not expand all 100k anchors and filter afterwards.
        //
        // This is attempted before the output-column guard: a multi-hop filter
        // over an earlier variable (`WHERE id(a)==0` above the second expand)
        // references `a`, which is absent from the second expand's own output
        // but present in its anchor input. The output guard would reject it
        // outright, so the anchor-only rewrite must run first.
        if Self::filter_references_only_input(filter_condition, expand_all) {
            let new_filter = filter_node.clone();
            let mut new_expand_all = expand_all.clone();
            if let Some(anchor) = expand_all.inputs().first() {
                // Descend through row-preserving `Flatten` wrappers so the
                // filter lands directly above the node that produces the
                // anchor variable. Otherwise the filter stops above the
                // wrapper and the earlier hop still expands every scanned
                // anchor. `Flatten` replays child rows without evaluating
                // columns, so a filter below it observes the same rows.
                let rebuilt = insert_filter_below_row_preserving(anchor, new_filter);
                new_expand_all.inputs_mut().clear();
                new_expand_all.add_input(rebuilt);
                let mut result = TransformResult::new();
                result.erase_curr = true;
                result.add_new_node(PlanNodeEnum::ExpandAll(new_expand_all));
                return Ok(Some(result));
            }
        }

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
    /// Check whether the filter references only variables produced by the
    /// expand's anchor input. Such filters are safe to evaluate on the anchor
    /// before expansion, avoiding full expansion of non-matching anchors.
    fn filter_references_only_input(
        filter_condition: &linkrs_core::types::ContextualExpression,
        expand_all: &ExpandAllNode,
    ) -> bool {
        let Some(expression) = filter_condition.get_expression() else {
            return false;
        };
        let referenced_vars = expression.get_variables();
        let Some(first_input) = expand_all.inputs().first() else {
            return false;
        };
        // Use the first real anchor node's own output columns, not the whole
        // subtree: a deeper node may produce a variable that an intervening
        // column-dropping operator (aggregate/project) removes before it
        // reaches the expand input. `Flatten` is row-preserving but may carry
        // no `col_names` of its own, so it is transparent here.
        let input_cols = available_columns(first_input);
        referenced_vars.iter().all(|var| input_cols.contains(var))
    }
    /// Check if a filter can be pushed down to an ExpandAll node.
    ///
    /// The filter can only be pushed down if all variables it references
    /// are available in the ExpandAll's output columns.
    fn can_push_filter_to_expand(
        filter_condition: &linkrs_core::types::ContextualExpression,
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

/// Columns visible at `node`'s output, looking through row-preserving
/// `Flatten` wrappers. A `Flatten` replays its child's rows and may carry no
/// `col_names`, so it is transparent; the first non-wrapper node's output
/// columns are the variables a filter may safely reference at this point.
fn available_columns(node: &PlanNodeEnum) -> &[String] {
    match node {
        PlanNodeEnum::Flatten(flatten) => available_columns(flatten.input()),
        _ => node.col_names(),
    }
}

/// Insert `filter` directly above the first non-`Flatten` node in `node`,
/// rebuilding any row-preserving `Flatten` wrappers on top.
fn insert_filter_below_row_preserving(
    node: &PlanNodeEnum,
    mut filter: crate::planning::plan::core::nodes::operation::filter_node::FilterNode,
) -> PlanNodeEnum {
    match node {
        PlanNodeEnum::Flatten(flatten) => {
            let mut new_flatten = flatten.clone();
            let below = insert_filter_below_row_preserving(flatten.input(), filter);
            new_flatten.set_input(below);
            PlanNodeEnum::Flatten(new_flatten)
        }
        _ => {
            filter.set_input(node.clone());
            PlanNodeEnum::Filter(filter)
        }
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
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
    use linkrs_core::Expression;

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
        use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
        use std::sync::Arc;

        let start = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start);

        let condition = Expression::Variable("test".to_string());
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(condition);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = linkrs_core::types::ContextualExpression::new(id, ctx);
        let filter = crate::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
            start_enum.clone(),
            ctx_expr,
        )
        .expect("Failed to create FilterNode");
        let filter_enum = PlanNodeEnum::Filter(filter);

        let expand_all = ExpandAllNode::new(1, vec![], "OUT");
        let expand_enum = PlanNodeEnum::ExpandAll(expand_all);

        assert!(rule.can_push_down(&filter_enum, &expand_enum));
    }

    fn anchor_scan(var: &str) -> PlanNodeEnum {
        use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
        let mut scan = ScanVerticesNode::new(1, "space");
        scan.set_tag("Node");
        scan.set_col_names(vec![var.to_string()]);
        PlanNodeEnum::ScanVertices(scan)
    }

    fn anchor_filter_expr(var: &str, property: &str) -> linkrs_core::types::ContextualExpression {
        use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
        use linkrs_core::types::operators::BinaryOperator;
        use linkrs_core::Value;
        use std::sync::Arc;

        let expr = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable(var.to_string())),
                property: property.to_string(),
            }),
            op: BinaryOperator::LessThan,
            right: Box::new(Expression::Literal(Value::Int(100))),
        };
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(linkrs_core::types::expr::ExpressionMeta::new(expr));
        linkrs_core::types::ContextualExpression::new(id, ctx)
    }

    fn filter_above(
        condition: linkrs_core::types::ContextualExpression,
        input: PlanNodeEnum,
    ) -> PlanNodeEnum {
        use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
        PlanNodeEnum::Filter(FilterNode::new(input, condition).expect("filter node"))
    }

    #[test]
    fn pushes_anchor_only_filter_below_expand() {
        let mut expand_all = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        expand_all.set_col_names(vec!["a".to_string(), "edge".to_string(), "b".to_string()]);
        expand_all.add_input(anchor_scan("a"));
        let filter = filter_above(
            anchor_filter_expr("a", "value"),
            PlanNodeEnum::ExpandAll(expand_all),
        );

        let rule = PushFilterDownExpandAllRule::new();
        let result = rule
            .apply(
                &mut crate::optimizer::heuristic::context::RewriteContext::new(),
                &filter,
            )
            .expect("rewrite");
        let result = result.expect("some result");

        let new_node = result.new_nodes.first().expect("node");
        let PlanNodeEnum::ExpandAll(expanded) = new_node else {
            panic!("expected ExpandAll root, got {new_node:?}");
        };
        // The filter must now be below the expand (on the anchor input).
        let PlanNodeEnum::Filter(pushed) = expanded.inputs().first().expect("anchor input") else {
            panic!("expected Filter below expand");
        };
        assert!(matches!(pushed.input(), PlanNodeEnum::ScanVertices(_)));
        // The expand itself carries no filter anymore.
        assert!(expanded.filter().is_none());
    }

    #[test]
    fn pushes_earlier_variable_filter_through_flatten_to_anchor() {
        use crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode;

        let mut expand1 = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        expand1.set_col_names(vec!["a".to_string(), "edge".to_string(), "b".to_string()]);
        expand1.add_input(anchor_scan("a"));

        let mut flatten = FlattenNode::new(PlanNodeEnum::ExpandAll(expand1), 0).expect("flatten");
        // Production `Flatten` nodes are built from a logical node whose
        // `col_names` may be empty; the rule must see through the wrapper to
        // the first hop's output columns.
        flatten.set_col_names(Vec::new());
        let flatten_enum = PlanNodeEnum::Flatten(flatten);

        let mut expand2 = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        expand2.set_col_names(vec![
            "a".to_string(),
            "edge".to_string(),
            "b".to_string(),
            "c".to_string(),
        ]);
        expand2.add_input(flatten_enum);

        let filter = filter_above(
            anchor_filter_expr("a", "value"),
            PlanNodeEnum::ExpandAll(expand2),
        );

        let rule = PushFilterDownExpandAllRule::new();
        let result = rule
            .apply(
                &mut crate::optimizer::heuristic::context::RewriteContext::new(),
                &filter,
            )
            .expect("rewrite")
            .expect("some result");

        let new_node = result.new_nodes.first().expect("node");
        let PlanNodeEnum::ExpandAll(expanded) = new_node else {
            panic!("expected ExpandAll root, got {new_node:?}");
        };
        // The filter must descend through the Flatten and land directly above
        // the first hop, so the earlier-variable predicate runs before the
        // second expansion.
        let PlanNodeEnum::Flatten(rebuilt_flatten) = expanded.inputs().first().expect("input")
        else {
            panic!("expected Flatten below expand");
        };
        let PlanNodeEnum::Filter(pushed) = rebuilt_flatten.input() else {
            panic!("expected Filter below Flatten");
        };
        assert!(matches!(pushed.input(), PlanNodeEnum::ExpandAll(_)));
    }

    #[test]
    fn does_not_push_filter_over_column_dropping_anchor() {
        use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
        use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
        use linkrs_core::types::expr::ExpressionMeta;
        use linkrs_core::types::ContextualExpression;
        use linkrs_core::Expression;
        use linkrs_core::YieldColumn;
        use std::sync::Arc;

        // Anchor subtree produces `x`, but the projection drops it: only `a`
        // reaches the expand input. A filter on `x` must not be pushed below
        // the expand, because `x` is unavailable there.
        let scan = anchor_scan("x");
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = expr_ctx
            .register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
        let project = ProjectNode::new(
            scan,
            vec![YieldColumn {
                expression: ContextualExpression::new(id, expr_ctx),
                alias: "a".to_string(),
            }],
        )
        .expect("project");
        let anchor = PlanNodeEnum::Project(project);

        let mut expand = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        expand.set_col_names(vec!["a".to_string(), "edge".to_string(), "b".to_string()]);
        expand.add_input(anchor);

        let filter = filter_above(
            anchor_filter_expr("x", "value"),
            PlanNodeEnum::ExpandAll(expand),
        );

        let rule = PushFilterDownExpandAllRule::new();
        let result = rule
            .apply(
                &mut crate::optimizer::heuristic::context::RewriteContext::new(),
                &filter,
            )
            .expect("rewrite");
        assert!(
            result.is_none(),
            "filter over a dropped variable must not be pushed"
        );
    }

    #[test]
    fn keeps_neighbor_filter_absorbed_in_expand() {
        let mut expand_all = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        expand_all.set_col_names(vec!["a".to_string(), "edge".to_string(), "b".to_string()]);
        expand_all.add_input(anchor_scan("a"));
        // Filter on the neighbor variable `b` must stay on the expand.
        let filter = filter_above(
            anchor_filter_expr("b", "value"),
            PlanNodeEnum::ExpandAll(expand_all),
        );

        let rule = PushFilterDownExpandAllRule::new();
        let result = rule
            .apply(
                &mut crate::optimizer::heuristic::context::RewriteContext::new(),
                &filter,
            )
            .expect("rewrite");
        let result = result.expect("some result");
        let new_node = result.new_nodes.first().expect("node");
        let PlanNodeEnum::ExpandAll(expanded) = new_node else {
            panic!("expected ExpandAll root");
        };
        assert!(
            expanded.filter().is_some(),
            "neighbor filter must stay absorbed on the expand"
        );
    }
}

//! Optimization rules that are filtered and pushed down to the aggregation nodes
//!
//! This rule pushes the filtering operations to be executed before the aggregation nodes, in order to reduce the amount of data that enters the aggregation process.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Filter(condition)
//!       |
//!   Aggregate(group_keys, agg_funcs)
//!       |
//!     Input
//! ```
//!
//! After:
//! ```text
//! Aggregate(group_keys, agg_funcs)
//!             |
//!       Filter(condition)
//!             |
//!           Input
//! ```
//!
//! # Applicable Conditions
//!
//! The child nodes of the Filter node are Aggregate nodes.
//! The Filter criteria do not involve aggregate functions (they only relate to the input columns that are being aggregated).

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor_checkers::AggregateFunctionChecker;
use crate::core::types::operators::AggregateFunction;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules that filter the data before it is aggregated
#[derive(Debug)]
pub struct PushFilterDownAggregateRule;

impl PushFilterDownAggregateRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }

    /// Check whether the condition contains references to aggregate functions.
    ///
    /// If the condition references the result of an aggregate function (e.g., COUNT(*), SUM(amount)),
    /// the filter cannot be applied "downstream" (to subsequent processing steps), because these
    /// aggregated results do not exist until the aggregation process is completed.
    fn has_aggregate_function_reference(
        condition: &Expression,
        group_keys: &[String],
        agg_funcs: &[AggregateFunction],
        agg_col_names: &[String],
    ) -> bool {
        fn check_expr(
            expr: &Expression,
            group_keys: &[String],
            agg_funcs: &[AggregateFunction],
            agg_col_names: &[String],
        ) -> bool {
            // First, check whether there are any expressions containing aggregate function statements.
            if AggregateFunctionChecker::check(expr) {
                return true;
            }

            // Check whether the variable is the name of an output column of an aggregate function.
            if let Expression::Variable(name) = expr {
                // If it is a grouping key, it can be pushed down (i.e., used for grouping data).
                if group_keys.contains(name) {
                    return false;
                }
                // Check whether it is the name of the output column of an aggregate function.
                for agg_func in agg_funcs {
                    if agg_func.name() == name
                        || agg_func.field_name().map(|f| f == name).unwrap_or(false)
                    {
                        return true;
                    }
                }
                // Check if the variable name is an aggregate alias (in col_names but not in group_keys)
                if agg_col_names.contains(name) {
                    return true;
                }
                // For the other variables, it is assumed that they can be determined based on the input data (i.e., they are dependent on the columns of input data).
                return false;
            }

            // Check whether the function call is an aggregate function.
            if let Expression::Function { name, .. } = expr {
                let func_name = name.to_lowercase();
                // Check whether it is the name of an aggregate function.
                if matches!(
                    func_name.as_str(),
                    "sum"
                        | "avg"
                        | "count"
                        | "max"
                        | "min"
                        | "collect"
                        | "collect_set"
                        | "distinct"
                        | "std"
                ) {
                    return true;
                }
            }

            // Recursively checking the subexpressions of a binary expression
            if let Expression::Binary { left, right, .. } = expr {
                if check_expr(left, group_keys, agg_funcs, agg_col_names) {
                    return true;
                }
                if check_expr(right, group_keys, agg_funcs, agg_col_names) {
                    return true;
                }
            }

            // Recursively check other composite expressions.
            if let Expression::Unary { operand, .. } = expr {
                return check_expr(operand, group_keys, agg_funcs, agg_col_names);
            }

            false
        }

        check_expr(condition, group_keys, agg_funcs, agg_col_names)
    }

    /// Rewrite the variable references in the filter conditions.
    ///
    /// Convert the variable references in the Filter to column references that represent the aggregated input data.
    ///
    /// # Why there's no need to rewrite
    ///
    /// The current implementation directly returns the original condition, which is correct, because:
    ///
    /// 1. **Grouping key**: The name of the grouping key remains the same before and after the aggregation, so there is no need to rewrite it.
    /// 2. **Columns output by aggregate functions**: These are blocked by the `has_aggregate_function_reference` check and will not be pushed down (i.e., their computation will not be performed at the lower levels of the data processing pipeline).
    /// 3. **Other input columns**: The names of the input columns remain the same before and after the aggregation, so there is no need to rewrite them.
    ///
    /// # When it’s necessary to rewrite
    ///
    /// If support for the following scenarios is needed in the future, `expression_utils::rewrite_expression` can be used:
    ///
    /// The names of the output columns in the aggregated result are different from the names of the input columns.
    /// It is necessary to map the aggregated output columns back to the input columns.
    /// It is necessary to handle the conversion of complex column names.
    fn rewrite_filter_condition(condition: &Expression, _group_keys: &[String]) -> Expression {
        condition.clone()
    }
}

impl Default for PushFilterDownAggregateRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownAggregateRule {
    fn name(&self) -> &'static str {
        "PushFilterDownAggregateRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("Aggregate")
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
        let filter_condition = filter_node.condition();

        // The obtained expression is used for processing.
        let filter_expr = match filter_condition.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };

        // Obtaining the context is necessary for creating a ContextualExpression.
        let ctx = filter_condition.context().clone();

        // Obtain the input node
        let input = filter_node.input();

        // Check whether the input node is an Aggregate.
        let agg_node = match input {
            PlanNodeEnum::Aggregate(n) => n,
            _ => return Ok(None),
        };

        // Obtain the aggregated group keys and the aggregate functions.
        let group_keys = agg_node.group_keys();
        let agg_funcs = agg_node.aggregation_functions();

        // Check whether the filter conditions contain references to aggregate functions.
        // 如果条件引用了聚合结果（如 HAVING COUNT(*) > 10），则不能下推
        if Self::has_aggregate_function_reference(
            &filter_expr,
            group_keys,
            agg_funcs,
            agg_node.col_names(),
        ) {
            return Ok(None);
        }

        // Obtain the aggregated input nodes.
        let agg_input = agg_node.input();

        // Rewrite the filter conditions (convert the references to the output columns into references to the input columns).
        let rewritten_condition = Self::rewrite_filter_condition(&filter_expr, group_keys);

        // Create metadata for the merged expression.
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(rewritten_condition);
        let id = ctx.register_expression(expr_meta);
        let rewritten_ctx_expr = ContextualExpression::new(id, ctx);

        // Create a new Filter node and place it before the Aggregate node.
        let new_filter = FilterNode::new(agg_input.clone(), rewritten_ctx_expr).map_err(|e| {
            crate::query::optimizer::heuristic::result::RewriteError::rewrite_failed(format!(
                "Failed to create FilterNode: {:?}",
                e
            ))
        })?;

        // Create a new Aggregate node; the input for this new node will be the new Filter node.
        let new_aggregate = AggregateNode::new(
            PlanNodeEnum::Filter(new_filter),
            group_keys.to_vec(),
            agg_funcs.to_vec(),
        )
        .map_err(|e| {
            crate::query::optimizer::heuristic::result::RewriteError::rewrite_failed(format!(
                "Failed to create AggregateNode: {:?}",
                e
            ))
        })?;

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true; // Delete the original Filter node.
        result.add_new_node(PlanNodeEnum::Aggregate(new_aggregate));

        Ok(Some(result))
    }
}

impl PushDownRule for PushFilterDownAggregateRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(
            (node, target),
            (PlanNodeEnum::Filter(_), PlanNodeEnum::Aggregate(_))
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
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_rule_name() {
        let rule = PushFilterDownAggregateRule::new();
        assert_eq!(rule.name(), "PushFilterDownAggregateRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushFilterDownAggregateRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_has_aggregate_function_reference_with_aggregate() {
        let condition = Expression::Aggregate {
            func: AggregateFunction::Count(None),
            arg: Box::new(Expression::Variable("amount".to_string())),
            distinct: false,
        };

        assert!(
            PushFilterDownAggregateRule::has_aggregate_function_reference(
                &condition,
                &[],
                &[AggregateFunction::Count(None)],
                &[]
            )
        );
    }

    #[test]
    fn test_no_aggregate_function_reference() {
        let condition = Expression::Binary {
            op: crate::core::types::operators::BinaryOperator::Equal,
            left: Box::new(Expression::Variable("name".to_string())),
            right: Box::new(Expression::Literal(crate::core::Value::String(
                "test".to_string(),
            ))),
        };

        assert!(
            !PushFilterDownAggregateRule::has_aggregate_function_reference(
                &condition,
                &["name".to_string()],
                &[],
                &[]
            )
        );
    }

    #[test]
    fn test_has_aggregate_function_reference_with_function() {
        let condition = Expression::Function {
            name: "sum".to_string(),
            args: vec![Expression::Variable("amount".to_string())],
        };

        assert!(
            PushFilterDownAggregateRule::has_aggregate_function_reference(
                &condition,
                &[],
                &[AggregateFunction::Sum("amount".to_string())],
                &[]
            )
        );
    }

    #[test]
    fn test_rewrite_filter_condition() {
        let condition = Expression::Binary {
            op: crate::core::types::operators::BinaryOperator::Equal,
            left: Box::new(Expression::Variable("name".to_string())),
            right: Box::new(Expression::Literal(crate::core::Value::String(
                "test".to_string(),
            ))),
        };

        let rewritten = PushFilterDownAggregateRule::rewrite_filter_condition(
            &condition,
            &["name".to_string()],
        );

        assert_eq!(rewritten, condition);
    }

    #[test]
    fn test_apply_with_group_key_filter() {
        // Create the Start node.
        let start_node = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        // Create an Aggregate node.
        let group_keys = vec!["category".to_string()];
        let agg_funcs = vec![AggregateFunction::Count(None)];
        let aggregate = AggregateNode::new(start_enum.clone(), group_keys, agg_funcs)
            .expect("Failed to create AggregateNode");
        let aggregate_enum = PlanNodeEnum::Aggregate(aggregate);

        // Create a Filter node (the condition only involves the grouping key).
        let condition = Expression::Binary {
            op: crate::core::types::operators::BinaryOperator::Equal,
            left: Box::new(Expression::Variable("category".to_string())),
            right: Box::new(Expression::Literal(crate::core::Value::String(
                "A".to_string(),
            ))),
        };
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(condition);
        let id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let filter =
            FilterNode::new(aggregate_enum, ctx_expr).expect("Failed to create FilterNode");
        let filter_enum = PlanNodeEnum::Filter(filter);

        // Application rules
        let rule = PushFilterDownAggregateRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &filter_enum)
            .expect("Failed to apply rule");

        // Verify that the conversion was successful.
        assert!(result.is_some());
        let transform_result = result.expect("Failed to apply rewrite rule");
        assert!(transform_result.erase_curr);
        assert_eq!(transform_result.new_nodes.len(), 1);
    }

    #[test]
    fn test_apply_with_aggregate_filter() {
        // Create the Start node.
        let start_node = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        // Create an Aggregate node.
        let group_keys = vec!["category".to_string()];
        let agg_funcs = vec![AggregateFunction::Count(None)];
        let aggregate = AggregateNode::new(start_enum.clone(), group_keys, agg_funcs)
            .expect("Failed to create AggregateNode");
        let aggregate_enum = PlanNodeEnum::Aggregate(aggregate);

        // 创建 Filter 节点（条件涉及聚合函数结果，如 HAVING COUNT(*) > 10）
        let condition = Expression::Binary {
            op: crate::core::types::operators::BinaryOperator::GreaterThan,
            left: Box::new(Expression::Variable("COUNT".to_string())),
            right: Box::new(Expression::Literal(crate::core::Value::Int(10))),
        };
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(condition);
        let id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let filter =
            FilterNode::new(aggregate_enum, ctx_expr).expect("Failed to create FilterNode");
        let filter_enum = PlanNodeEnum::Filter(filter);

        // Application rules
        let rule = PushFilterDownAggregateRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &filter_enum)
            .expect("Failed to apply rule");

        // The conversion was not executed (because the conditions involved aggregated results).
        assert!(result.is_none());
    }

    #[test]
    fn test_apply_with_non_aggregate_input() {
        // Create the Start node.
        let start_node = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        // Create a Filter node, but the input is not of the Aggregate type.
        let condition = Expression::Binary {
            op: crate::core::types::operators::BinaryOperator::Equal,
            left: Box::new(Expression::Variable("name".to_string())),
            right: Box::new(Expression::Literal(crate::core::Value::String(
                "test".to_string(),
            ))),
        };
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(condition);
        let id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);
        let filter = FilterNode::new(start_enum, ctx_expr).expect("Failed to create FilterNode");
        let filter_enum = PlanNodeEnum::Filter(filter);

        // Apply the rules
        let rule = PushFilterDownAggregateRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &filter_enum)
            .expect("Failed to apply rule");

        // The conversion was not performed (because the input is not of the “Aggregate” type).
        assert!(result.is_none());
    }
}

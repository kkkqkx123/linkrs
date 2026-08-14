//! Constant folding heuristic rule.
//!
//! Replaces expressions that can be evaluated without a runtime context
//! (pure functions over literal values) with their literal result. Runs in
//! the Normalize batch before predicate pushdown so downstream rules see
//! canonical literal forms (e.g. `1 + 1 = 2` becomes `2 = 2`).
//!
//! Safety gates:
//! - `is_evaluable`: the expression references no variables / properties /
//!   parameters / subqueries (analysis_utils).
//! - purity: a function is folded only when the function registry reports it
//!   pure (`BuiltinFunction::is_pure`); unregistered functions are never
//!   folded (conservative).
//! - aggregates and window functions are never folded (they need a row
//!   group context even with constant arguments).
//! - evaluation failures keep the original expression (conservative).

use crate::core::types::expr::analysis_utils::is_evaluable;
use crate::core::types::expr::ExpressionMeta;
use crate::core::types::ContextualExpression;
use crate::core::Expression;
use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::expression::functions::global_registry_ref;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::PlanNodeEnum;

/// Constant folding rule.
#[derive(Debug)]
pub struct FoldConstantsRule;

impl FoldConstantsRule {
    /// Create a new rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Whether the expression tree contains a non-pure function call.
    fn is_pure(expression: &Expression) -> bool {
        match expression {
            // Aggregates and window functions need a row-group context even
            // with constant arguments — never fold them.
            Expression::Aggregate { .. } | Expression::WindowFunction { .. } => false,
            Expression::Function { name, args, .. } => {
                // Purity comes from the function registry; unregistered
                // functions are conservatively treated as non-pure.
                global_registry_ref()
                    .get_builtin(name.as_str())
                    .is_some_and(|f| f.is_pure())
                    && args.iter().all(Self::is_pure)
            }
            Expression::Binary { left, right, .. } => Self::is_pure(left) && Self::is_pure(right),
            Expression::Unary { operand, .. } => Self::is_pure(operand),
            Expression::List(items) => items.iter().all(Self::is_pure),
            Expression::Map(pairs) => pairs.iter().all(|(_, v)| Self::is_pure(v)),
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                test_expr.as_ref().is_none_or(|e| Self::is_pure(e))
                    && conditions
                        .iter()
                        .all(|(c, v)| Self::is_pure(c) && Self::is_pure(v))
                    && default.as_ref().is_none_or(|e| Self::is_pure(e))
            }
            Expression::TypeCast { expression, .. } => Self::is_pure(expression),
            Expression::Subscript {
                collection, index, ..
            } => Self::is_pure(collection) && Self::is_pure(index),
            Expression::Range {
                collection,
                start,
                end,
            } => {
                Self::is_pure(collection)
                    && start.as_ref().is_none_or(|e| Self::is_pure(e))
                    && end.as_ref().is_none_or(|e| Self::is_pure(e))
            }
            Expression::Path(items) => items.iter().all(Self::is_pure),
            Expression::PathBuild(items) => items.iter().all(Self::is_pure),
            _ => true,
        }
    }

    /// Recursively fold constant sub-expressions into literals.
    ///
    /// Children are folded first; a node is then replaced by its evaluated
    /// literal when the folded whole is evaluable, pure and not an aggregate
    /// / window function. Any evaluation error keeps the folded (or original)
    /// expression — folding never changes query semantics.
    fn fold_expression(expression: &Expression) -> Expression {
        let folded = match expression {
            Expression::Binary { left, op, right } => Expression::Binary {
                left: Box::new(Self::fold_expression(left)),
                op: *op,
                right: Box::new(Self::fold_expression(right)),
            },
            Expression::Unary { op, operand } => Expression::Unary {
                op: *op,
                operand: Box::new(Self::fold_expression(operand)),
            },
            Expression::Function { name, args } => Expression::Function {
                name: name.clone(),
                args: args.iter().map(Self::fold_expression).collect(),
            },
            Expression::List(items) => {
                Expression::List(items.iter().map(Self::fold_expression).collect())
            }
            Expression::Map(pairs) => Expression::Map(
                pairs
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::fold_expression(v)))
                    .collect(),
            ),
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => Expression::Case {
                test_expr: test_expr
                    .as_ref()
                    .map(|e| Box::new(Self::fold_expression(e))),
                conditions: conditions
                    .iter()
                    .map(|(c, v)| (Self::fold_expression(c), Self::fold_expression(v)))
                    .collect(),
                default: default.as_ref().map(|e| Box::new(Self::fold_expression(e))),
            },
            Expression::TypeCast {
                expression,
                target_type,
            } => Expression::TypeCast {
                expression: Box::new(Self::fold_expression(expression)),
                target_type: target_type.clone(),
            },
            Expression::Subscript { collection, index } => Expression::Subscript {
                collection: Box::new(Self::fold_expression(collection)),
                index: Box::new(Self::fold_expression(index)),
            },
            Expression::Range {
                collection,
                start,
                end,
            } => Expression::Range {
                collection: Box::new(Self::fold_expression(collection)),
                start: start.as_ref().map(|e| Box::new(Self::fold_expression(e))),
                end: end.as_ref().map(|e| Box::new(Self::fold_expression(e))),
            },
            Expression::Path(items) => {
                Expression::Path(items.iter().map(Self::fold_expression).collect())
            }
            Expression::PathBuild(items) => {
                Expression::PathBuild(items.iter().map(Self::fold_expression).collect())
            }
            // Aggregates, window functions, subqueries and runtime-bound
            // expressions are never folded here.
            Expression::Aggregate { .. }
            | Expression::WindowFunction { .. }
            | Expression::Exists { .. }
            | Expression::In { .. } => expression.clone(),
            _ => expression.clone(),
        };

        if is_evaluable(&folded) && Self::is_pure(&folded) {
            let mut context = DefaultExpressionContext::new();
            match ExpressionEvaluator::evaluate(&folded, &mut context) {
                Ok(value) => Expression::Literal(value),
                Err(_) => folded,
            }
        } else {
            folded
        }
    }

    /// Register a folded expression in the rewrite context.
    fn register_folded(ctx: &RewriteContext, folded: Expression) -> ContextualExpression {
        let meta = ExpressionMeta::new(folded);
        let id = ctx.expr_context().register_expression(meta);
        ContextualExpression::new(id, ctx.expr_context())
    }
}

impl Default for FoldConstantsRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for FoldConstantsRule {
    fn name(&self) -> &'static str {
        "FoldConstantsRule"
    }

    fn pattern(&self) -> Pattern {
        // Wildcard: the rule inspects every node's expressions.
        Pattern::new()
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            PlanNodeEnum::Filter(filter) => {
                let condition = filter.condition();
                let Some(meta) = condition.expression() else {
                    return Ok(None);
                };
                let folded = Self::fold_expression(meta.inner());
                if matches!(folded, Expression::Literal(_)) {
                    let new_condition = Self::register_folded(ctx, folded);
                    let mut new_filter = filter.clone();
                    new_filter.set_condition(new_condition);
                    let mut result = TransformResult::new();
                    result.add_new_node(PlanNodeEnum::Filter(new_filter));
                    return Ok(Some(result));
                }
                Ok(None)
            }
            PlanNodeEnum::Project(project) => {
                let columns = project.columns();
                let mut changed = false;
                let mut new_columns = columns.to_vec();
                for column in new_columns.iter_mut() {
                    let Some(meta) = column.expression.expression() else {
                        continue;
                    };
                    let folded = Self::fold_expression(meta.inner());
                    if matches!(folded, Expression::Literal(_)) {
                        column.expression = Self::register_folded(ctx, folded);
                        changed = true;
                    }
                }
                if !changed {
                    return Ok(None);
                }
                let mut new_project = project.clone();
                new_project.set_columns(new_columns);
                let mut result = TransformResult::new();
                result.add_new_node(PlanNodeEnum::Project(new_project));
                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::analysis_utils::is_evaluable;
    use crate::core::types::expr::ExpressionAnalysisContext;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::operators::BinaryOperator;
    use crate::core::types::ContextualExpression;
    use crate::core::Value;
    use crate::core::YieldColumn;
    use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::operation::FilterNode;
    use crate::query::planning::plan::core::nodes::operation::ProjectNode;
    use std::sync::Arc;

    fn contextual(
        expr_ctx: &Arc<ExpressionAnalysisContext>,
        expr: Expression,
    ) -> ContextualExpression {
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        ContextualExpression::new(id, expr_ctx.clone())
    }

    #[test]
    fn test_fold_rule_name() {
        let rule = FoldConstantsRule::new();
        assert_eq!(rule.name(), "FoldConstantsRule");
    }

    #[test]
    fn test_fold_binary_constant() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        assert!(is_evaluable(&expr));
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, Expression::Literal(Value::Int(3)));
    }

    #[test]
    fn test_fold_with_variable_is_not_folded() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("x".to_string())),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "expressions with variables must not fold");
    }

    #[test]
    fn test_fold_partial_constant_subexpression() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(1))),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Literal(Value::Int(2))),
            }),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Variable("x".to_string())),
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(
            folded,
            Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(3))),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Variable("x".to_string())),
            },
            "constant sub-expressions fold, the variable part stays"
        );
    }

    #[test]
    fn test_fold_impure_function_is_not_folded() {
        let expr = Expression::Function {
            name: "rand".to_string(),
            args: vec![],
        };
        assert!(is_evaluable(&expr), "rand has no context dependency");
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "impure functions must never fold");
    }

    #[test]
    fn test_fold_unregistered_function_is_not_folded() {
        // A function name that is not in the registry must conservatively
        // stay unfolded even though it carries no context dependency.
        let expr = Expression::Function {
            name: "future_nondeterministic_fn".to_string(),
            args: vec![Expression::Literal(Value::Int(1))],
        };
        assert!(is_evaluable(&expr), "no context dependency");
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "unregistered functions must never fold");
    }

    #[test]
    fn test_fold_pure_builtin_function() {
        let expr = Expression::Function {
            name: "abs".to_string(),
            args: vec![Expression::Literal(Value::Int(-5))],
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(
            folded,
            Expression::Literal(Value::Int(5)),
            "pure builtin functions fold"
        );
    }

    #[test]
    fn test_fold_aggregate_is_not_folded() {
        let expr = Expression::Aggregate {
            func: crate::core::AggregateFunction::Count(None),
            args: vec![Expression::Literal(Value::Int(1))],
            distinct: false,
            filter: None,
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "aggregates need a row-group context");
    }

    #[test]
    fn test_fold_filter_node_condition() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let start = PlanNodeEnum::Start(StartNode::new());
        let condition = contextual(
            &expr_ctx,
            Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(1))),
                op: BinaryOperator::Equal,
                right: Box::new(Expression::Literal(Value::Int(1))),
            },
        );
        let filter = FilterNode::new(start, condition).expect("filter node");
        let node = PlanNodeEnum::Filter(filter);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("filter with constant condition must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Filter(f) => {
                let folded = f
                    .condition()
                    .expression()
                    .expect("expression")
                    .inner()
                    .clone();
                assert_eq!(
                    folded,
                    Expression::Literal(Value::Bool(true)),
                    "1 = 1 folds to TRUE"
                );
            }
            other => panic!("expected Filter node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_project_node_columns() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let start = PlanNodeEnum::Start(StartNode::new());
        let column = YieldColumn {
            expression: contextual(
                &expr_ctx,
                Expression::Binary {
                    left: Box::new(Expression::Literal(Value::Int(10))),
                    op: BinaryOperator::Divide,
                    right: Box::new(Expression::Literal(Value::Int(2))),
                },
            ),
            alias: "half".to_string(),
            is_matched: false,
        };
        let project = ProjectNode::new(start, vec![column]).expect("project node");
        let node = PlanNodeEnum::Project(project);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("project with constant column must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Project(p) => {
                let folded = p.columns()[0]
                    .expression
                    .expression()
                    .expect("expression")
                    .inner()
                    .clone();
                assert_eq!(folded, Expression::Literal(Value::Int(5)));
                assert_eq!(p.columns()[0].alias, "half", "alias is preserved");
            }
            other => panic!("expected Project node, got {:?}", other),
        }
    }
}

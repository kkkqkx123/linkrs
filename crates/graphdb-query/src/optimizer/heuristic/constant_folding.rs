//! Constant folding heuristic rule.
//!
//! Replaces sub-expressions that can be evaluated without a runtime context
//! (pure functions over literal values) with their folded result. Folds both
//! whole expressions (e.g. `1 + 1 = 2` becomes `TRUE`) and constant
//! sub-expressions of larger expressions (e.g. `x + (1 + 2)` becomes `x +
//! 3`). Runs in the Normalize batch before predicate pushdown so downstream
//! rules see canonical literal forms.
//!
//! Covered nodes: Filter conditions, Project columns, Assign assignments,
//! Sort items, Window function specs (args / partition / order), Aggregate
//! filter expressions, and join hash/probe keys.
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

use crate::executor::expression::evaluation_context::DefaultExpressionContext;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::expression::functions::global_registry_ref;
use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::AssignNode;
use crate::planning::plan::core::nodes::graph_operations::window_node::{
    WindowFunctionSpec, WindowNode,
};
use crate::planning::plan::core::nodes::join::join_node::{
    FullOuterJoinNode, InnerJoinNode, LeftJoinNode, RightJoinNode, SemiJoinNode,
};
use crate::planning::plan::core::nodes::operation::sort_node::{SortItem, SortNode};
use crate::planning::plan::PlanNodeEnum;
use graphdb_core::types::expr::analysis_utils::is_evaluable;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::expr::FunctionArg;
use graphdb_core::types::ContextualExpression;
use graphdb_core::Expression;

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
            // Subqueries need a streaming runtime context — never fold them.
            Expression::ScalarSubquery { .. } => false,
            Expression::Function { name, args, .. } => {
                // Purity comes from the function registry; unregistered
                // functions are conservatively treated as non-pure.
                global_registry_ref()
                    .get_builtin(name.as_str())
                    .is_some_and(|f| f.is_pure())
                    && args.iter().all(|a| Self::is_pure(a.as_expr()))
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
                args: args
                    .iter()
                    .map(|a| FunctionArg::Positional(Self::fold_expression(a.as_expr())))
                    .collect(),
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
                Ok(value) => {
                    log::debug!(
                        "Constant folding: {} => {}",
                        folded.to_expression_string(),
                        value
                    );
                    Expression::Literal(value)
                }
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

impl FoldConstantsRule {
    /// Rule name (kept inherent so the trait impl can delegate).
    fn name(&self) -> &'static str {
        "FoldConstantsRule"
    }

    fn pattern(&self) -> Pattern {
        // Wildcard: the rule inspects every node's expressions.
        Pattern::new()
    }

    /// Fold a raw `Expression`, returning the folded form when it differs
    /// from the original.
    fn fold_raw(expression: &Expression) -> Option<Expression> {
        let folded = Self::fold_expression(expression);
        (folded != *expression).then_some(folded)
    }

    /// Fold a `ContextualExpression`, re-registering the result in the
    /// rewrite context when it differs from the original.
    fn fold_contextual(
        ctx: &RewriteContext,
        expression: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        let meta = expression.expression()?;
        let folded = Self::fold_expression(meta.inner());
        (folded != *meta.inner()).then(|| Self::register_folded(ctx, folded))
    }

    /// Fold a window function spec's `args`, `partition_by` and `order_by`
    /// expressions, returning the new spec when anything changed.
    fn fold_window_spec(spec: &WindowFunctionSpec) -> Option<WindowFunctionSpec> {
        let mut new_spec = spec.clone();
        let mut changed = false;
        for expr in new_spec.args.iter_mut() {
            if let Some(folded) = Self::fold_raw(expr) {
                *expr = folded;
                changed = true;
            }
        }
        for expr in new_spec.partition_by.iter_mut() {
            if let Some(folded) = Self::fold_raw(expr) {
                *expr = folded;
                changed = true;
            }
        }
        for expr in new_spec.order_by.iter_mut() {
            if let Some(folded) = Self::fold_raw(expr) {
                *expr = folded;
                changed = true;
            }
        }
        changed.then_some(new_spec)
    }

    /// Fold a list of join keys, returning the new list when anything
    /// changed.
    fn fold_join_keys(
        ctx: &RewriteContext,
        keys: &[ContextualExpression],
    ) -> Option<Vec<ContextualExpression>> {
        let mut changed = false;
        let new_keys: Vec<ContextualExpression> = keys
            .iter()
            .map(|key| {
                if let Some(folded) = Self::fold_contextual(ctx, key) {
                    changed = true;
                    folded
                } else {
                    key.clone()
                }
            })
            .collect();
        changed.then_some(new_keys)
    }

    /// Fold a list of join hash/probe key pairs on a join node.
    ///
    /// Returns the new keys when either side changed.
    fn fold_join_node_keys(
        ctx: &RewriteContext,
        hash_keys: &[ContextualExpression],
        probe_keys: &[ContextualExpression],
    ) -> Option<(Vec<ContextualExpression>, Vec<ContextualExpression>)> {
        let hash = Self::fold_join_keys(ctx, hash_keys);
        let probe = Self::fold_join_keys(ctx, probe_keys);
        match (hash, probe) {
            (None, None) => None,
            (Some(h), None) => Some((h, probe_keys.to_vec())),
            (None, Some(p)) => Some((hash_keys.to_vec(), p)),
            (Some(h), Some(p)) => Some((h, p)),
        }
    }

    /// Build a `TransformResult` that replaces `node` with `new_node`.
    fn replace_node(new_node: PlanNodeEnum) -> RewriteResult<Option<TransformResult>> {
        let mut result = TransformResult::new();
        result.add_new_node(new_node);
        Ok(Some(result))
    }

    /// Mark the replacement node (and the result) as having folded constant
    /// expressions so EXPLAIN can surface `folded: true`.
    fn mark_folded(result: &mut Option<TransformResult>) {
        let Some(result) = result.as_mut() else {
            return;
        };
        result.mark_folded();
        if let Some(node) = result.new_nodes.first_mut() {
            use crate::planning::plan::PlanNodeEnum::*;
            match node {
                Filter(n) => n.set_has_folded_expressions(true),
                Project(n) => n.set_has_folded_expressions(true),
                Assign(n) => n.set_has_folded_expressions(true),
                Sort(n) => n.set_has_folded_expressions(true),
                Window(n) => n.set_has_folded_expressions(true),
                Aggregate(n) => n.set_has_folded_expressions(true),
                InnerJoin(n) => n.set_has_folded_expressions(true),
                LeftJoin(n) => n.set_has_folded_expressions(true),
                FullOuterJoin(n) => n.set_has_folded_expressions(true),
                RightJoin(n) => n.set_has_folded_expressions(true),
                SemiJoin(n) => n.set_has_folded_expressions(true),
                _ => {}
            }
        }
    }

    fn apply_filter(
        &self,
        ctx: &RewriteContext,
        filter: &crate::planning::plan::core::nodes::operation::filter_node::FilterNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let condition = filter.condition();
        let Some(new_condition) = Self::fold_contextual(ctx, condition) else {
            return Ok(None);
        };
        let mut new_filter = filter.clone();
        new_filter.set_condition(new_condition);
        let mut result = Self::replace_node(PlanNodeEnum::Filter(new_filter))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_project(
        &self,
        ctx: &RewriteContext,
        project: &crate::planning::plan::core::nodes::operation::project_node::ProjectNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let mut changed = false;
        let mut new_columns = project.columns().to_vec();
        for column in new_columns.iter_mut() {
            if let Some(folded) = Self::fold_contextual(ctx, &column.expression) {
                column.expression = folded;
                changed = true;
            }
        }
        if !changed {
            return Ok(None);
        }
        let mut new_project = project.clone();
        new_project.set_columns(new_columns);
        let mut result = Self::replace_node(PlanNodeEnum::Project(new_project))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_assign(
        &self,
        ctx: &RewriteContext,
        assign: &AssignNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let mut changed = false;
        let mut new_assignments = assign.assignments().to_vec();
        for (_, expr) in new_assignments.iter_mut() {
            if let Some(folded) = Self::fold_contextual(ctx, expr) {
                *expr = folded;
                changed = true;
            }
        }
        if !changed {
            return Ok(None);
        }
        let mut new_assign = assign.clone();
        new_assign.set_assignments(new_assignments);
        let mut result = Self::replace_node(PlanNodeEnum::Assign(new_assign))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_sort(&self, sort: &SortNode) -> RewriteResult<Option<TransformResult>> {
        let mut changed = false;
        let mut new_sort_items: Vec<SortItem> = sort.sort_items().to_vec();
        for item in new_sort_items.iter_mut() {
            if let Some(folded) = Self::fold_raw(&item.expression) {
                item.expression = folded;
                changed = true;
            }
        }
        if !changed {
            return Ok(None);
        }
        let mut new_sort = sort.clone();
        new_sort.set_sort_items(new_sort_items);
        let mut result = Self::replace_node(PlanNodeEnum::Sort(new_sort))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_window(&self, window: &WindowNode) -> RewriteResult<Option<TransformResult>> {
        let mut changed = false;
        let mut new_specs = window.window_functions().to_vec();
        for spec in new_specs.iter_mut() {
            if let Some(folded) = Self::fold_window_spec(spec) {
                *spec = folded;
                changed = true;
            }
        }
        if !changed {
            return Ok(None);
        }
        let mut new_window = window.clone();
        new_window.set_window_functions(new_specs);
        let mut result = Self::replace_node(PlanNodeEnum::Window(new_window))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_aggregate(&self, aggregate: &AggregateNode) -> RewriteResult<Option<TransformResult>> {
        let mut changed = false;
        let mut new_filters = aggregate.aggregation_filters().to_vec();
        for filter in new_filters.iter_mut() {
            let Some(expr) = filter.as_mut() else {
                continue;
            };
            if let Some(folded) = Self::fold_raw(expr) {
                *expr = folded;
                changed = true;
            }
        }
        if !changed {
            return Ok(None);
        }
        let mut new_aggregate = aggregate.clone();
        new_aggregate.set_aggregation_filters(new_filters);
        let mut result = Self::replace_node(PlanNodeEnum::Aggregate(new_aggregate))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_inner_join(
        &self,
        ctx: &RewriteContext,
        join: &InnerJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let Some((hash_keys, probe_keys)) =
            Self::fold_join_node_keys(ctx, join.hash_keys(), join.probe_keys())
        else {
            return Ok(None);
        };
        let mut new_join = join.clone();
        new_join.set_hash_keys(hash_keys);
        new_join.set_probe_keys(probe_keys);
        let mut result = Self::replace_node(PlanNodeEnum::InnerJoin(new_join))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_left_join(
        &self,
        ctx: &RewriteContext,
        join: &LeftJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let Some((hash_keys, probe_keys)) =
            Self::fold_join_node_keys(ctx, join.hash_keys(), join.probe_keys())
        else {
            return Ok(None);
        };
        let mut new_join = join.clone();
        new_join.set_hash_keys(hash_keys);
        new_join.set_probe_keys(probe_keys);
        let mut result = Self::replace_node(PlanNodeEnum::LeftJoin(new_join))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_full_outer_join(
        &self,
        ctx: &RewriteContext,
        join: &FullOuterJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let Some((hash_keys, probe_keys)) =
            Self::fold_join_node_keys(ctx, join.hash_keys(), join.probe_keys())
        else {
            return Ok(None);
        };
        let mut new_join = join.clone();
        new_join.set_hash_keys(hash_keys);
        new_join.set_probe_keys(probe_keys);
        let mut result = Self::replace_node(PlanNodeEnum::FullOuterJoin(new_join))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_right_join(
        &self,
        ctx: &RewriteContext,
        join: &RightJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let Some((hash_keys, probe_keys)) =
            Self::fold_join_node_keys(ctx, join.hash_keys(), join.probe_keys())
        else {
            return Ok(None);
        };
        let mut new_join = join.clone();
        new_join.set_hash_keys(hash_keys);
        new_join.set_probe_keys(probe_keys);
        let mut result = Self::replace_node(PlanNodeEnum::RightJoin(new_join))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply_semi_join(
        &self,
        ctx: &RewriteContext,
        join: &SemiJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let Some((hash_keys, probe_keys)) =
            Self::fold_join_node_keys(ctx, join.hash_keys(), join.probe_keys())
        else {
            return Ok(None);
        };
        let mut new_join = join.clone();
        new_join.set_hash_keys(hash_keys);
        new_join.set_probe_keys(probe_keys);
        let mut result = Self::replace_node(PlanNodeEnum::SemiJoin(new_join))?;
        Self::mark_folded(&mut result);
        Ok(result)
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            PlanNodeEnum::Filter(filter) => self.apply_filter(ctx, filter),
            PlanNodeEnum::Project(project) => self.apply_project(ctx, project),
            PlanNodeEnum::Assign(assign) => self.apply_assign(ctx, assign),
            PlanNodeEnum::Sort(sort) => self.apply_sort(sort),
            PlanNodeEnum::Window(window) => self.apply_window(window),
            PlanNodeEnum::Aggregate(aggregate) => self.apply_aggregate(aggregate),
            PlanNodeEnum::InnerJoin(join) => self.apply_inner_join(ctx, join),
            PlanNodeEnum::LeftJoin(join) => self.apply_left_join(ctx, join),
            PlanNodeEnum::FullOuterJoin(join) => self.apply_full_outer_join(ctx, join),
            PlanNodeEnum::RightJoin(join) => self.apply_right_join(ctx, join),
            PlanNodeEnum::SemiJoin(join) => self.apply_semi_join(ctx, join),
            _ => Ok(None),
        }
    }
}

impl RewriteRule for FoldConstantsRule {
    fn name(&self) -> &'static str {
        Self::name(self)
    }

    fn pattern(&self) -> Pattern {
        Self::pattern(self)
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        Self::apply(self, ctx, node)
    }
}

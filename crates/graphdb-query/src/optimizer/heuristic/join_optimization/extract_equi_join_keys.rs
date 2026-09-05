//! Extract equi-join keys from a Filter above a keyless InnerJoin.
//!
//! This rule identifies the Filter -> InnerJoin pattern where the join
//! carries no hash/probe keys (a cross product with the predicate evaluated
//! per pair above it) and moves cross-side equalities into the join keys.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   Filter(a.value = b.value)
//!           |
//!   InnerJoin (no keys)
//!   /          \
//! Left        Right
//! ```
//!
//! After:
//! ```text
//!   InnerJoin (hash_keys=[b.value], probe_keys=[a.value])
//!   /          \
//! Left        Right
//! ```
//!
//! # Applicable Conditions
//!
//! - The InnerJoin has empty hash/probe keys (existing keys are never
//!   disturbed).
//! - A top-level `AND` conjunct is an `Equal` whose two sides each reference
//!   columns from exactly one join input (left/right or right/left).
//!   Anything else (single-side predicates, literals, non-equalities,
//!   ambiguous column references) stays in a residual Filter above the join.
//!
//! # Side convention
//!
//! The executor builds the hash table from the right child with `hash_keys`
//! and probes it with the left child using `probe_keys`
//! (`BuildSide::Right` default), so the right-side expression becomes a
//! hash key and the left-side expression a probe key.

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::PlanNodeEnum;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::visitor::ExpressionVisitor;
use graphdb_core::types::expr::visitor_collectors::VariableCollector;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::Expression;
use std::sync::Arc;

/// Extract equi-join keys from a Filter above a keyless InnerJoin.
#[derive(Debug)]
pub struct ExtractEquiJoinKeysRule;

impl ExtractEquiJoinKeysRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Flatten a top-level `AND` chain into its conjuncts.
    fn flatten_conjuncts(expr: &Expression, out: &mut Vec<Expression>) {
        match expr {
            Expression::Binary {
                op: BinaryOperator::And,
                left,
                right,
            } => {
                Self::flatten_conjuncts(left, out);
                Self::flatten_conjuncts(right, out);
            }
            _ => out.push(expr.clone()),
        }
    }

    /// Collect the variable names referenced by an expression.
    fn referenced_variables(expr: &Expression) -> Vec<String> {
        let mut collector = VariableCollector::new();
        collector.visit(expr);
        collector.variables
    }

    /// Attribute an expression to exactly one join side.
    ///
    /// Returns `Some(true)` when every referenced variable belongs to the
    /// left input (and none to the right), `Some(false)` for the mirrored
    /// right case, and `None` when the expression is ambiguous (empty,
    /// single-side-unresolvable, or shared column names).
    fn is_left_side(vars: &[String], left_cols: &[String], right_cols: &[String]) -> Option<bool> {
        if vars.is_empty() {
            return None;
        }
        let in_left = vars.iter().all(|v| left_cols.contains(v));
        let in_right = vars.iter().all(|v| right_cols.contains(v));
        match (in_left, in_right) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        }
    }

    /// Register a key expression in the given context.
    fn register_key(
        expr: Expression,
        ctx: &Arc<ExpressionAnalysisContext>,
    ) -> ContextualExpression {
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx.clone())
    }

    /// Combine residual conjuncts back into a single `AND` expression,
    /// preserving the original left-to-right order.
    fn combine_residual(conjuncts: Vec<Expression>) -> Option<Expression> {
        let mut iter = conjuncts.into_iter();
        let first = iter.next()?;
        Some(iter.fold(first, |acc, next| Expression::Binary {
            left: Box::new(acc),
            op: BinaryOperator::And,
            right: Box::new(next),
        }))
    }
}

impl Default for ExtractEquiJoinKeysRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for ExtractEquiJoinKeysRule {
    fn name(&self) -> &'static str {
        "ExtractEquiJoinKeysRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("InnerJoin")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let filter_node = match node {
            PlanNodeEnum::Filter(n) => n,
            _ => return Ok(None),
        };
        let join = match filter_node.input() {
            PlanNodeEnum::InnerJoin(n) => n,
            _ => return Ok(None),
        };
        // Never disturb joins that already carry keys.
        if !join.hash_keys().is_empty() || !join.probe_keys().is_empty() {
            return Ok(None);
        }

        let condition = filter_node.condition();
        let filter_expr = match condition.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };
        let expr_ctx = condition.context().clone();

        let left_cols = join.left_input().col_names().to_vec();
        let right_cols = join.right_input().col_names().to_vec();

        let mut conjuncts = Vec::new();
        Self::flatten_conjuncts(&filter_expr, &mut conjuncts);

        let mut hash_keys = Vec::new();
        let mut probe_keys = Vec::new();
        let mut residual = Vec::new();

        for conjunct in conjuncts {
            // A usable key is an equality whose sides belong to opposite
            // join inputs. The left-input expression probes, the
            // right-input expression builds (see the module docs).
            let sides = match &conjunct {
                Expression::Binary {
                    op: BinaryOperator::Equal,
                    left,
                    right,
                } => {
                    let left_is_left = Self::is_left_side(
                        &Self::referenced_variables(left),
                        &left_cols,
                        &right_cols,
                    );
                    let right_is_left = Self::is_left_side(
                        &Self::referenced_variables(right),
                        &left_cols,
                        &right_cols,
                    );
                    match (left_is_left, right_is_left) {
                        (Some(true), Some(false)) => Some((left, right)),
                        (Some(false), Some(true)) => Some((right, left)),
                        _ => None,
                    }
                }
                _ => None,
            };
            match sides {
                Some((probe_side, hash_side)) => {
                    probe_keys.push(Self::register_key((**probe_side).clone(), &expr_ctx));
                    hash_keys.push(Self::register_key((**hash_side).clone(), &expr_ctx));
                }
                None => residual.push(conjunct),
            }
        }

        if hash_keys.is_empty() {
            return Ok(None);
        }

        let mut new_join = join.clone();
        new_join.set_hash_keys(hash_keys);
        new_join.set_probe_keys(probe_keys);
        let new_join_enum = PlanNodeEnum::InnerJoin(new_join);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        if residual.is_empty() {
            result.add_new_node(new_join_enum);
        } else {
            let residual_expr = match Self::combine_residual(residual) {
                Some(expr) => expr,
                None => {
                    result.add_new_node(new_join_enum);
                    return Ok(Some(result));
                }
            };
            let residual_ctx_expr = Self::register_key(residual_expr, &expr_ctx);
            let new_filter = FilterNode::new(new_join_enum, residual_ctx_expr).map_err(|e| {
                crate::optimizer::heuristic::result::RewriteError::rewrite_failed(format!(
                    "Failed to create residual FilterNode: {:?}",
                    e
                ))
            })?;
            result.add_new_node(PlanNodeEnum::Filter(new_filter));
        }
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;

    fn scan_with_cols(space: &str, cols: &[&str]) -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, space);
        scan.set_col_names(cols.iter().map(|s| s.to_string()).collect());
        PlanNodeEnum::ScanVertices(scan)
    }

    fn contextual(expr: Expression, ctx: &Arc<ExpressionAnalysisContext>) -> ContextualExpression {
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx.clone())
    }

    fn equality_filter(
        input: PlanNodeEnum,
        left: Expression,
        right: Expression,
        ctx: &Arc<ExpressionAnalysisContext>,
    ) -> PlanNodeEnum {
        let cond = Expression::Binary {
            left: Box::new(left),
            op: BinaryOperator::Equal,
            right: Box::new(right),
        };
        let filter = FilterNode::new(input, contextual(cond, ctx)).expect("filter");
        PlanNodeEnum::Filter(filter)
    }

    fn prop(var: &str, property: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::variable(var)),
            property: property.to_string(),
        }
    }

    #[test]
    fn test_rule_name() {
        let rule = ExtractEquiJoinKeysRule::new();
        assert_eq!(rule.name(), "ExtractEquiJoinKeysRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = ExtractEquiJoinKeysRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_skips_join_with_existing_keys() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = scan_with_cols("s", &["a"]);
        let right = scan_with_cols("s", &["b"]);
        let key = contextual(Expression::variable("a"), &ctx);
        let join =
            InnerJoinNode::new(left, right, vec![key.clone()], vec![key]).expect("join with keys");
        let node = equality_filter(
            PlanNodeEnum::InnerJoin(join),
            prop("a", "value"),
            prop("b", "value"),
            &ctx,
        );
        let rule = ExtractEquiJoinKeysRule::new();
        let mut rewrite_ctx = RewriteContext::new();
        let out = rule.apply(&mut rewrite_ctx, &node).expect("apply");
        assert!(out.is_none(), "joins with keys must be left alone");
    }

    #[test]
    fn test_extracts_cross_side_equality() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = scan_with_cols("s", &["a"]);
        let right = scan_with_cols("s", &["b"]);
        let join = InnerJoinNode::new(left, right, vec![], vec![]).expect("keyless join");
        let node = equality_filter(
            PlanNodeEnum::InnerJoin(join),
            prop("a", "value"),
            prop("b", "value"),
            &ctx,
        );
        let rule = ExtractEquiJoinKeysRule::new();
        let mut rewrite_ctx = RewriteContext::new();
        let out = rule
            .apply(&mut rewrite_ctx, &node)
            .expect("apply")
            .expect("must fire");
        assert!(out.erase_curr);
        assert_eq!(out.new_nodes.len(), 1);
        match &out.new_nodes[0] {
            PlanNodeEnum::InnerJoin(j) => {
                assert_eq!(j.hash_keys().len(), 1);
                assert_eq!(j.probe_keys().len(), 1);
                // Build side (right) key references b, probe side (left) key
                // references a.
                let hash_vars = ExtractEquiJoinKeysRule::referenced_variables(
                    j.hash_keys()[0].expression().expect("hash").inner(),
                );
                let probe_vars = ExtractEquiJoinKeysRule::referenced_variables(
                    j.probe_keys()[0].expression().expect("probe").inner(),
                );
                assert_eq!(hash_vars, vec!["b".to_string()]);
                assert_eq!(probe_vars, vec!["a".to_string()]);
            }
            other => panic!("expected keyed InnerJoin, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn test_extracts_mirrored_equality() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = scan_with_cols("s", &["a"]);
        let right = scan_with_cols("s", &["b"]);
        let join = InnerJoinNode::new(left, right, vec![], vec![]).expect("keyless join");
        // Equality written right-first: b.value = a.value.
        let node = equality_filter(
            PlanNodeEnum::InnerJoin(join),
            prop("b", "value"),
            prop("a", "value"),
            &ctx,
        );
        let rule = ExtractEquiJoinKeysRule::new();
        let mut rewrite_ctx = RewriteContext::new();
        let out = rule
            .apply(&mut rewrite_ctx, &node)
            .expect("apply")
            .expect("must fire");
        match &out.new_nodes[0] {
            PlanNodeEnum::InnerJoin(j) => {
                let hash_vars = ExtractEquiJoinKeysRule::referenced_variables(
                    j.hash_keys()[0].expression().expect("hash").inner(),
                );
                let probe_vars = ExtractEquiJoinKeysRule::referenced_variables(
                    j.probe_keys()[0].expression().expect("probe").inner(),
                );
                assert_eq!(hash_vars, vec!["b".to_string()]);
                assert_eq!(probe_vars, vec!["a".to_string()]);
            }
            other => panic!("expected keyed InnerJoin, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn test_keeps_residual_filter() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = scan_with_cols("s", &["a"]);
        let right = scan_with_cols("s", &["b"]);
        let join = InnerJoinNode::new(left, right, vec![], vec![]).expect("keyless join");
        let equality = Expression::Binary {
            left: Box::new(prop("a", "value")),
            op: BinaryOperator::Equal,
            right: Box::new(prop("b", "value")),
        };
        let range = Expression::Binary {
            left: Box::new(prop("a", "value")),
            op: BinaryOperator::LessThan,
            right: Box::new(Expression::int(200)),
        };
        let cond = Expression::Binary {
            left: Box::new(equality),
            op: BinaryOperator::And,
            right: Box::new(range),
        };
        let filter =
            FilterNode::new(PlanNodeEnum::InnerJoin(join), contextual(cond, &ctx)).expect("filter");
        let node = PlanNodeEnum::Filter(filter);
        let rule = ExtractEquiJoinKeysRule::new();
        let mut rewrite_ctx = RewriteContext::new();
        let out = rule
            .apply(&mut rewrite_ctx, &node)
            .expect("apply")
            .expect("must fire");
        assert_eq!(out.new_nodes.len(), 1);
        match &out.new_nodes[0] {
            PlanNodeEnum::Filter(f) => match f.input() {
                PlanNodeEnum::InnerJoin(j) => {
                    assert_eq!(j.hash_keys().len(), 1);
                    assert_eq!(j.probe_keys().len(), 1);
                }
                other => panic!(
                    "residual filter must sit above keyed join, got {:?}",
                    other.type_name()
                ),
            },
            other => panic!("expected residual Filter, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn test_ignores_single_side_predicate() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = scan_with_cols("s", &["a"]);
        let right = scan_with_cols("s", &["b"]);
        let join = InnerJoinNode::new(left, right, vec![], vec![]).expect("keyless join");
        let range = Expression::Binary {
            left: Box::new(prop("a", "value")),
            op: BinaryOperator::LessThan,
            right: Box::new(Expression::int(200)),
        };
        let filter = FilterNode::new(PlanNodeEnum::InnerJoin(join), contextual(range, &ctx))
            .expect("filter");
        let node = PlanNodeEnum::Filter(filter);
        let rule = ExtractEquiJoinKeysRule::new();
        let mut rewrite_ctx = RewriteContext::new();
        let out = rule.apply(&mut rewrite_ctx, &node).expect("apply");
        assert!(out.is_none(), "single-side predicates are not join keys");
    }
}

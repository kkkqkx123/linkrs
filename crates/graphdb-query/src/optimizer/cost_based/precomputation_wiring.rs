//! Expression precomputation decision wiring (CBO phase, note-only).
//!
//! Collects the scalar expressions attached to the plan tree (filter
//! predicates, projection columns, sort keys, join keys, window clauses),
//! counts how many times each distinct expression is referenced, and lets
//! the [`ExpressionPrecomputationOptimizer`] decide whether precomputing
//! it pays off. The decisions are emitted as `precompute:` notes in
//! EXPLAIN's `CBO decisions:` list so they are observable.
//!
//! This phase only makes the decision visible; it never rewrites the
//! plan. Injecting precomputed values into operator inputs is left to
//! future work, so execution semantics are unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::core::types::expr::{Expression, ExpressionMeta};
use crate::core::types::ContextualExpression;
use crate::optimizer::cost_based::expression_precomputation::{
    ExpressionPrecomputationOptimizer, PrecomputationDecision,
};
use crate::planning::plan::PlanNodeEnum;

/// Distinct expressions referenced by the plan tree, keyed by a stable
/// fingerprint (the Debug rendering of the bound expression).
///
/// Expressions of the same shape attached to different nodes are treated
/// as one expression; this is the reuse the precomputation decision is
/// about.
struct ExpressionRegistry {
    ctx: Arc<ExpressionAnalysisContext>,
    counts: HashMap<String, usize>,
    contextual: HashMap<String, ContextualExpression>,
}

impl ExpressionRegistry {
    fn new() -> Self {
        Self {
            ctx: Arc::new(ExpressionAnalysisContext::new()),
            counts: HashMap::new(),
            contextual: HashMap::new(),
        }
    }

    /// Record an already bound (contextual) expression.
    fn record_contextual(&mut self, expr: &ContextualExpression) {
        let Some(inner) = expr.get_expression() else {
            return;
        };
        self.record(inner, || expr.clone());
    }

    /// Record a raw expression (sort items, window clauses) by binding it
    /// into the registry's context on first sight.
    fn record_raw(&mut self, expr: &Expression) {
        let id = self
            .ctx
            .register_expression(ExpressionMeta::new(expr.clone()));
        let bound = ContextualExpression::new(id, Arc::clone(&self.ctx));
        self.record(expr.clone(), || bound);
    }

    fn record(&mut self, inner: Expression, build_bound: impl FnOnce() -> ContextualExpression) {
        let fingerprint = format!("{:?}", inner);
        *self.counts.entry(fingerprint.clone()).or_insert(0) += 1;
        self.contextual
            .entry(fingerprint)
            .or_insert_with(build_bound);
    }
}

/// Collect `precompute:` notes for every expression whose cost-benefit
/// analysis favors precomputation. Expressions referenced only once are
/// skipped (a precomputation decision requires reuse).
pub fn collect_precompute_notes(
    root: &PlanNodeEnum,
    optimizer: &ExpressionPrecomputationOptimizer,
) -> Vec<String> {
    let mut registry = ExpressionRegistry::new();
    walk(root, &mut registry);

    let mut notes = Vec::new();
    for (fingerprint, count) in &registry.counts {
        if *count <= 1 {
            continue;
        }
        let Some(expr) = registry.contextual.get(fingerprint) else {
            continue;
        };
        if let PrecomputationDecision::Precompute {
            reason,
            benefit,
            cost,
        } = optimizer.should_precompute(expr, *count)
        {
            notes.push(format!(
                "precompute: {} x{} (reason={:?}, benefit={:.3}, cost={:.3})",
                expr.to_expression_string(),
                count,
                reason,
                benefit,
                cost
            ));
        }
    }

    // Sort for deterministic EXPLAIN output (HashMap iteration is unstable).
    notes.sort();
    notes
}

/// Walk the plan tree and record every scalar expression carried by the
/// node kinds the optimizer understands.
fn walk(node: &PlanNodeEnum, registry: &mut ExpressionRegistry) {
    use PlanNodeEnum::*;

    match node {
        Filter(filter) => registry.record_contextual(filter.condition()),
        Project(project) => {
            for column in project.columns() {
                registry.record_contextual(&column.expression);
            }
        }
        Sort(sort) => {
            for item in sort.sort_items() {
                registry.record_raw(&item.expression);
            }
        }
        TopN(sort) => {
            for item in sort.sort_items() {
                registry.record_raw(&item.expression);
            }
        }
        Window(window) => {
            for function in window.window_functions() {
                for arg in &function.args {
                    registry.record_raw(arg);
                }
                for expr in &function.partition_by {
                    registry.record_raw(expr);
                }
                for expr in &function.order_by {
                    registry.record_raw(expr);
                }
            }
        }
        InnerJoin(join) => {
            record_join_keys(join.hash_keys(), join.probe_keys(), registry);
        }
        LeftJoin(join) => {
            record_join_keys(join.hash_keys(), join.probe_keys(), registry);
        }
        FullOuterJoin(join) => {
            record_join_keys(join.hash_keys(), join.probe_keys(), registry);
        }
        RightJoin(join) => {
            record_join_keys(join.hash_keys(), join.probe_keys(), registry);
        }
        SemiJoin(join) => {
            record_join_keys(join.hash_keys(), join.probe_keys(), registry);
        }
        _ => {}
    }

    for child in node.children() {
        walk(child, registry);
    }
}

/// Record a join node's hash and probe key expressions.
fn record_join_keys(
    hash_keys: &[ContextualExpression],
    probe_keys: &[ContextualExpression],
    registry: &mut ExpressionRegistry,
) {
    for key in hash_keys {
        registry.record_contextual(key);
    }
    for key in probe_keys {
        registry.record_contextual(key);
    }
}

use std::collections::HashMap;

use crate::optimizer::cost_based::join_order::{JoinCondition, JoinOrderOptimizer, JoinOrderResult, TableInfo};
use crate::optimizer::JoinAlgorithm;
use linkrs_core::types::expr::analysis_utils::collect_variables_from_contextual;
use linkrs_core::types::expr::contextual::ContextualExpression;

use super::super::types::normalize_join_algorithm_decision;

pub(crate) fn resolve_fallback(
    keys: &[ContextualExpression],
    table_index: &HashMap<&str, usize>,
    tables: &[TableInfo],
) -> String {
    for expr in keys {
        let vars = collect_variables_from_contextual(expr);
        for v in &vars {
            if let Some(&idx) = table_index.get(v.as_str()) {
                return tables[idx].id.clone();
            }
            for t in tables {
                if v.starts_with(&t.id) || t.id.starts_with(v) {
                    return t.id.clone();
                }
            }
        }
    }
    tables.first().map(|t| t.id.clone()).unwrap_or_default()
}

pub(crate) fn collect_variables_from_slice(keys: &[ContextualExpression]) -> Vec<String> {
    let mut vars = Vec::new();
    for expr in keys {
        vars.extend(collect_variables_from_contextual(expr));
    }
    vars
}

pub(crate) fn normalize_join_algorithm(
    algorithm: Option<&JoinAlgorithm>,
    has_hash_keys: bool,
    left_rows: u64,
    right_rows: u64,
) -> Option<JoinAlgorithm> {
    normalize_join_algorithm_decision(algorithm, has_hash_keys, left_rows, right_rows)
}

/// Review gate shared by the physical and logical join-chain walkers.
///
/// Returns the current order's cost when the proposal replaces the chain,
/// `None` when the current shape is kept: unpriceable or non-finite costs,
/// identical orders, and improvements within the configured review
/// threshold (ties keep current so plans never flap between equal-cost
/// alternatives).
///
/// Chains with no cost signal at all (zero row estimates) keep the legacy
/// rebuild so algorithm decisions are still recorded for the arena builder.
pub(crate) fn review_accepts_order(
    optimizer: &JoinOrderOptimizer,
    tables: &[TableInfo],
    conditions: &[JoinCondition],
    current_order: &[String],
    result: &JoinOrderResult,
    min_improvement: f64,
) -> Option<f64> {
    let current_cost = optimizer.cost_of_order(tables, conditions, current_order)?;
    if !current_cost.is_finite() || !result.total_cost.is_finite() {
        return None;
    }
    if current_cost <= 0.0 {
        return (result.order != current_order).then_some(current_cost);
    }
    if result.order == current_order {
        return None;
    }
    let gate = if min_improvement.is_finite() {
        min_improvement.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if result.total_cost < current_cost * (1.0 - gate) {
        Some(current_cost)
    } else {
        None
    }
}

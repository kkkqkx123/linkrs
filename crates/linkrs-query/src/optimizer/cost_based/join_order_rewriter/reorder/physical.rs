use std::collections::HashMap;

use crate::optimizer::cost::CostCalculator;
use crate::optimizer::cost_based::join_order::{
    JoinCondition, JoinOrderOptimizer, JoinOrderResult, TableInfo,
};
use crate::optimizer::cost_based::ndv::{refine_join_selectivity, DEFAULT_JOIN_SELECTIVITY};
use crate::optimizer::stats::StatsView;
use crate::optimizer::JoinAlgorithm;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::PlanNodeEnum;
use linkrs_core::types::expr::contextual::ContextualExpression;

use super::super::flatten::{
    assign_leaf_info, classify_join, flatten_join_chain, leaf_id,
};
use super::super::types::{
    accumulated_join_rows, build_optimizer_input_from_parts, build_pred_map,
    format_join_order_note, normalize_join_algorithm_decision, should_swap_join_keys,
    FlattenedJoinChain, JoinNodeType, OptResult, PredMap,
};
use super::shared::{collect_variables_from_slice, resolve_fallback, review_accepts_order};

pub fn build_optimizer_input(chain: &FlattenedJoinChain) -> (Vec<TableInfo>, Vec<JoinCondition>) {
    let ids: Vec<String> = chain.leaves.iter().map(|leaf| leaf.id.clone()).collect();
    let rows: Vec<u64> = chain
        .leaves
        .iter()
        .map(|leaf| leaf.estimated_rows)
        .collect();
    let indexed: Vec<bool> = chain.leaves.iter().map(|leaf| leaf.has_index).collect();
    let table_index: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let tables_hint: Vec<TableInfo> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            TableInfo::new(id.clone(), rows[i])
                .with_index(indexed[i])
                .with_bit_id(i as u32)
        })
        .collect();
    let mut predicates = chain.predicates.clone();
    for p in &mut predicates {
        let left = p.left_table.as_str();
        let right = p.right_table.as_str();
        if left == right || left.is_empty() || right.is_empty() {
            let lid = resolve_fallback(&p.left_key, &table_index, &tables_hint);
            let rid = resolve_fallback(&p.right_key, &table_index, &tables_hint);
            p.left_table = lid;
            p.right_table = rid;
        }
    }
    let resolved_ids = ids.clone();
    let _ = resolved_ids;
    build_optimizer_input_from_parts(&ids, &rows, &indexed, &predicates)
}

/// Rebuild the reordered join tree, stamping the per-join `JoinAlgorithm`
/// decision directly on the rebuilt `InnerJoin` node.
///
/// The decision is the first cost-based physical choice that takes effect
/// downstream: the arena builder reads `InnerJoinNode::algorithm` when
/// converting the join node. Decisions are only stamped when executable:
///
/// - `HashJoin` requires valid equi keys (`has_hash_keys`);
/// - `NestedLoopJoin` requires trusted row estimates (both operands > 0) so
///   that a missing-statistics plan (0-row estimates) does not silently turn
///   every keyed join into an O(N*M) scan;
/// - `IndexJoin` has no executor yet and is left to the default heuristic.
fn reconstruct_join_tree_with_decisions(
    original_root: &PlanNodeEnum,
    chain: &FlattenedJoinChain,
    result: &JoinOrderResult,
) -> PlanNodeEnum {
    let original_type = classify_join(original_root);
    if original_type != JoinNodeType::Inner && original_type != JoinNodeType::Cross {
        return original_root.clone();
    }

    let leaf_map: HashMap<&str, &PlanNodeEnum> = chain
        .leaves
        .iter()
        .map(|l| (l.id.as_str(), &l.physical_node))
        .collect();

    let leaf_rows: HashMap<&str, u64> = chain
        .leaves
        .iter()
        .map(|l| (l.id.as_str(), l.estimated_rows))
        .collect();

    let pred_map: PredMap = build_pred_map(&chain.predicates);

    let mut current: Option<PlanNodeEnum> = None;
    let mut accumulated_rows: u64 = 0;
    let mut step: usize = 0;

    for table_id in &result.order {
        let right_node = match leaf_map.get(table_id.as_str()) {
            Some(node) => (*node).clone(),
            None => {
                log::warn!("JoinOrderOptimizer returned unknown table '{}'", table_id);
                continue;
            }
        };
        let right_rows = leaf_rows.get(table_id.as_str()).copied().unwrap_or(0);

        current = match current.take() {
            Some(left) => {
                let lid = leaf_id(&left);
                let rid = leaf_id(&right_node);
                let pair_key = if lid <= rid {
                    (lid.clone(), rid.clone())
                } else {
                    (rid.clone(), lid.clone())
                };
                let (hash_keys, probe_keys) =
                    resolve_keys_for_pair(&pair_key, &pred_map, &left, &right_node);
                let has_hash_keys = !hash_keys.is_empty();
                let algorithm = normalize_join_algorithm_decision(
                    result.algorithms.get(step),
                    has_hash_keys,
                    accumulated_rows,
                    right_rows,
                );
                let joined = build_inner_join(left, right_node, hash_keys, probe_keys, algorithm);
                step += 1;
                // Output estimate mirrors the join-order cost model's
                // default join selectivity.
                let selectivity = chain
                    .predicates
                    .iter()
                    .find(|p| {
                        let a = p.left_table.as_str();
                        let b = p.right_table.as_str();
                        (a == lid && b == rid) || (a == rid && b == lid)
                    })
                    .map(|p| p.selectivity)
                    .unwrap_or(DEFAULT_JOIN_SELECTIVITY);
                accumulated_rows = accumulated_join_rows(accumulated_rows, right_rows, selectivity);
                Some(joined)
            }
            None => {
                accumulated_rows = right_rows;
                Some(right_node)
            }
        };
    }

    current.unwrap_or_else(|| original_root.clone())
}

fn resolve_keys_for_pair(
    pair_key: &(String, String),
    pred_map: &PredMap,
    left_physical: &PlanNodeEnum,
    right_physical: &PlanNodeEnum,
) -> (Vec<ContextualExpression>, Vec<ContextualExpression>) {
    if let Some(keys_list) = pred_map.get(pair_key) {
        if let Some((hk, pk)) = keys_list.first() {
            let left_id = leaf_id(left_physical);
            let right_id = leaf_id(right_physical);
            let left_vars = collect_variables_from_slice(hk);
            let right_vars = collect_variables_from_slice(pk);
            if should_swap_join_keys(&left_id, &right_id, &left_vars, &right_vars) {
                return (pk.clone(), hk.clone());
            }
            return (hk.clone(), pk.clone());
        }
    }
    (vec![], vec![])
}

fn build_inner_join(
    left: PlanNodeEnum,
    right: PlanNodeEnum,
    hash_keys: Vec<ContextualExpression>,
    probe_keys: Vec<ContextualExpression>,
    algorithm: Option<JoinAlgorithm>,
) -> PlanNodeEnum {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    match InnerJoinNode::new(left, right, hash_keys, probe_keys) {
        Ok(mut node) => {
            node.set_algorithm(algorithm);
            PlanNodeEnum::InnerJoin(node)
        }
        Err(e) => {
            panic!("InnerJoin construction failed: {}", e);
        }
    }
}

fn try_optimize_join_tree(
    root: &PlanNodeEnum,
    stats: &StatsView,
    cost_calculator: &CostCalculator,
) -> OptResult {
    let Some(mut chain) = flatten_join_chain(root) else {
        return OptResult::Unchanged;
    };

    if chain.leaves.len() < 2 {
        return OptResult::Unchanged;
    }

    assign_leaf_info(&mut chain, stats);

    for pred in &mut chain.predicates {
        pred.selectivity =
            refine_join_selectivity(stats, &pred.left_key, &pred.right_key, pred.selectivity);
    }

    let (tables, conditions) = build_optimizer_input(&chain);

    let optimizer = JoinOrderOptimizer::new(std::sync::Arc::new(cost_calculator.clone()));
    let result = optimizer.optimize_join_order(&tables, &conditions);

    let current_order: Vec<String> = chain.leaves.iter().map(|leaf| leaf.id.clone()).collect();
    let min_improvement = cost_calculator
        .config()
        .strategy_thresholds
        .join_reorder_min_improvement;
    let Some(current_cost) = review_accepts_order(
        &optimizer,
        &tables,
        &conditions,
        &current_order,
        &result,
        min_improvement,
    ) else {
        return OptResult::Unchanged;
    };

    log::debug!(
        "Join order optimization: {} tables, cost {} -> {}, method={:?}, order={:?}",
        chain.leaves.len(),
        current_cost,
        result.total_cost,
        result.optimization_method,
        result.order,
    );

    let note = format_join_order_note(
        chain.leaves.len(),
        &format!("{:?}", result.optimization_method),
        &result.order,
        current_cost,
        result.total_cost,
    );
    OptResult::Changed(
        Box::new(reconstruct_join_tree_with_decisions(root, &chain, &result)),
        note,
    )
}

/// Recursively rewrite reorderable join chains, recording the cost-based
/// `JoinAlgorithm` decisions (keyed by the rebuilt join node ids) into
/// `decisions` for the arena builder.
pub fn walk_and_optimize_joins(
    root: &PlanNodeEnum,
    stats: &StatsView,
    cost_calculator: &CostCalculator,
    notes: &mut Vec<String>,
) -> PlanNodeEnum {
    if let OptResult::Changed(optimized, note) =
        try_optimize_join_tree(root, stats, cost_calculator)
    {
        notes.push(note);
        return *optimized;
    }

    match root {
        PlanNodeEnum::Project(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Project(cloned)
        }
        PlanNodeEnum::Filter(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Filter(cloned)
        }
        PlanNodeEnum::Sort(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Sort(cloned)
        }
        PlanNodeEnum::Limit(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Limit(cloned)
        }
        PlanNodeEnum::TopN(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::TopN(cloned)
        }
        PlanNodeEnum::Sample(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Sample(cloned)
        }
        PlanNodeEnum::Dedup(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Dedup(cloned)
        }
        PlanNodeEnum::Aggregate(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Aggregate(cloned)
        }
        PlanNodeEnum::Window(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Window(cloned)
        }
        PlanNodeEnum::Traverse(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Traverse(cloned)
        }
        PlanNodeEnum::LeftJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator, notes);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::LeftJoin(cloned)
        }
        PlanNodeEnum::RightJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator, notes);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::RightJoin(cloned)
        }
        PlanNodeEnum::FullOuterJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator, notes);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::FullOuterJoin(cloned)
        }
        PlanNodeEnum::SemiJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator, notes);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::SemiJoin(cloned)
        }
        _ => root.clone(),
    }
}

// =====================================================================
// Logical-plan reorder/reconstruction functions
// =====================================================================

use std::collections::HashMap;

use crate::optimizer::cost::CostCalculator;
use crate::optimizer::cost_based::join_order::{
    JoinCondition, JoinOrderOptimizer, JoinOrderResult, TableInfo,
};
use crate::optimizer::cost_based::ndv::{refine_join_selectivity, DEFAULT_JOIN_SELECTIVITY};
use crate::optimizer::stats::StatsView;
use crate::optimizer::JoinAlgorithm;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;
use graphdb_core::types::expr::analysis_utils::collect_variables_from_contextual;
use graphdb_core::types::expr::contextual::ContextualExpression;

use super::flatten::{
    assign_leaf_info, assign_leaf_info_logical, classify_join, classify_join_logical,
    flatten_join_chain, flatten_join_chain_logical, leaf_id, leaf_id_logical, logical_column_types,
};
use super::types::{
    FlattenedJoinChain, FlattenedJoinChainLogical, JoinNodeType, OptResult, OptResultLogical,
    PredMap,
};

pub fn build_optimizer_input(chain: &FlattenedJoinChain) -> (Vec<TableInfo>, Vec<JoinCondition>) {
    let tables: Vec<TableInfo> = chain
        .leaves
        .iter()
        .enumerate()
        .map(|(i, leaf)| {
            TableInfo::new(leaf.id.clone(), leaf.estimated_rows)
                .with_index(leaf.has_index)
                .with_bit_id(i as u32)
        })
        .collect();

    let table_index: HashMap<&str, usize> = tables
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.as_str(), i))
        .collect();

    let conditions: Vec<JoinCondition> = chain
        .predicates
        .iter()
        .map(|p| {
            let left = p.left_table.as_str();
            let right = p.right_table.as_str();
            let (left_id, right_id) = if left == right || left.is_empty() || right.is_empty() {
                let lid = resolve_fallback(&p.left_key, &table_index, &tables);
                let rid = resolve_fallback(&p.right_key, &table_index, &tables);
                (lid, rid)
            } else {
                (left.to_string(), right.to_string())
            };
            JoinCondition::new(left_id, right_id).with_selectivity(p.selectivity)
        })
        .collect();

    (tables, conditions)
}

fn resolve_fallback(
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

/// Rebuild the reordered join tree, recording the per-join `JoinAlgorithm`
/// decision (from `result.algorithms`) keyed by the newly created
/// `InnerJoin` node id.
///
/// The decision channel is the first cost-based physical choice that takes
/// effect downstream: the arena builder consults
/// `ExecutionContext::join_algorithms` when converting the join node.
/// Decisions are only recorded when they are executable and safe:
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
    decisions: &mut Option<&mut HashMap<i64, JoinAlgorithm>>,
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

    let mut pred_map: PredMap = HashMap::new();
    for p in &chain.predicates {
        let (a, b) = if p.left_table <= p.right_table {
            (p.left_table.clone(), p.right_table.clone())
        } else {
            (p.right_table.clone(), p.left_table.clone())
        };
        pred_map
            .entry((a, b))
            .or_default()
            .push((p.left_key.clone(), p.right_key.clone()));
    }

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
                let joined = build_inner_join(left, right_node, hash_keys, probe_keys);
                if let Some(decisions) = decisions.as_deref_mut() {
                    let algorithm = result.algorithms.get(step);
                    record_join_algorithm(
                        decisions,
                        &joined,
                        algorithm,
                        has_hash_keys,
                        accumulated_rows,
                        right_rows,
                    );
                }
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
                accumulated_rows =
                    ((accumulated_rows as f64 * right_rows as f64 * selectivity) as u64).max(1);
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

/// Normalize a cost-based join algorithm decision and record it for the
/// arena builder.  See [`reconstruct_join_tree_with_decisions`] for the
/// safety gates.
fn record_join_algorithm(
    decisions: &mut HashMap<i64, JoinAlgorithm>,
    node: &PlanNodeEnum,
    algorithm: Option<&JoinAlgorithm>,
    has_hash_keys: bool,
    left_rows: u64,
    right_rows: u64,
) {
    let Some(algorithm) = algorithm else {
        return;
    };
    match algorithm {
        JoinAlgorithm::NestedLoopJoin { .. } => {
            if left_rows > 0 && right_rows > 0 {
                decisions.insert(node.id(), algorithm.clone());
            }
        }
        JoinAlgorithm::HashJoin { .. } => {
            if has_hash_keys {
                decisions.insert(node.id(), algorithm.clone());
            }
        }
        JoinAlgorithm::IndexJoin { .. } => {
            // No index-join executor: the default heuristic (hash join)
            // applies.
        }
    }
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

            let swap = left_vars
                .iter()
                .any(|v| right_id.contains(v) || v.contains(&right_id))
                || right_vars
                    .iter()
                    .any(|v| left_id.contains(v) || v.contains(&left_id));
            if swap {
                return (pk.clone(), hk.clone());
            }
            return (hk.clone(), pk.clone());
        }
    }
    (vec![], vec![])
}

fn collect_variables_from_slice(keys: &[ContextualExpression]) -> Vec<String> {
    let mut vars = Vec::new();
    for expr in keys {
        vars.extend(collect_variables_from_contextual(expr));
    }
    vars
}

fn build_inner_join(
    left: PlanNodeEnum,
    right: PlanNodeEnum,
    hash_keys: Vec<ContextualExpression>,
    probe_keys: Vec<ContextualExpression>,
) -> PlanNodeEnum {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    match InnerJoinNode::new(left, right, hash_keys, probe_keys) {
        Ok(node) => PlanNodeEnum::InnerJoin(node),
        Err(e) => {
            panic!("InnerJoin construction failed: {}", e);
        }
    }
}

fn try_optimize_join_tree(
    root: &PlanNodeEnum,
    stats: &StatsView,
    cost_calculator: &CostCalculator,
    decisions: &mut Option<&mut HashMap<i64, JoinAlgorithm>>,
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

    let note = format!(
        "join_order: {} tables, method={:?}, order=[{}], cost {:.1}->{:.1} (reviewer)",
        chain.leaves.len(),
        result.optimization_method,
        result.order.join(", "),
        current_cost,
        result.total_cost,
    );
    OptResult::Changed(
        Box::new(reconstruct_join_tree_with_decisions(
            root, &chain, &result, decisions,
        )),
        note,
    )
}

/// Recursively rewrite reorderable join chains, recording the cost-based
/// `JoinAlgorithm` decisions (keyed by the rebuilt join node ids) into
/// `decisions` for the arena builder.
pub fn walk_and_optimize_joins_with_decisions(
    root: &PlanNodeEnum,
    stats: &StatsView,
    cost_calculator: &CostCalculator,
    notes: &mut Vec<String>,
    decisions: &mut Option<&mut HashMap<i64, JoinAlgorithm>>,
) -> PlanNodeEnum {
    if let OptResult::Changed(optimized, note) =
        try_optimize_join_tree(root, stats, cost_calculator, decisions)
    {
        notes.push(note);
        return *optimized;
    }

    match root {
        PlanNodeEnum::Project(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Project(cloned)
        }
        PlanNodeEnum::Filter(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Filter(cloned)
        }
        PlanNodeEnum::Sort(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Sort(cloned)
        }
        PlanNodeEnum::Limit(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Limit(cloned)
        }
        PlanNodeEnum::TopN(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::TopN(cloned)
        }
        PlanNodeEnum::Sample(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Sample(cloned)
        }
        PlanNodeEnum::Dedup(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Dedup(cloned)
        }
        PlanNodeEnum::Aggregate(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Aggregate(cloned)
        }
        PlanNodeEnum::Window(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Window(cloned)
        }
        PlanNodeEnum::Traverse(n) => {
            let new_input = walk_and_optimize_joins_with_decisions(
                n.input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Traverse(cloned)
        }
        PlanNodeEnum::LeftJoin(n) => {
            let new_left = walk_and_optimize_joins_with_decisions(
                n.left_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let new_right = walk_and_optimize_joins_with_decisions(
                n.right_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::LeftJoin(cloned)
        }
        PlanNodeEnum::RightJoin(n) => {
            let new_left = walk_and_optimize_joins_with_decisions(
                n.left_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let new_right = walk_and_optimize_joins_with_decisions(
                n.right_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::RightJoin(cloned)
        }
        PlanNodeEnum::FullOuterJoin(n) => {
            let new_left = walk_and_optimize_joins_with_decisions(
                n.left_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let new_right = walk_and_optimize_joins_with_decisions(
                n.right_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::FullOuterJoin(cloned)
        }
        PlanNodeEnum::SemiJoin(n) => {
            let new_left = walk_and_optimize_joins_with_decisions(
                n.left_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
            let new_right = walk_and_optimize_joins_with_decisions(
                n.right_input(),
                stats,
                cost_calculator,
                notes,
                decisions,
            );
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

pub fn build_optimizer_input_logical(
    chain: &FlattenedJoinChainLogical,
) -> (Vec<TableInfo>, Vec<JoinCondition>) {
    let tables: Vec<TableInfo> = chain
        .leaves
        .iter()
        .enumerate()
        .map(|(i, leaf)| {
            // The logical tree carries no IndexScan yet (index selection is
            // a later phase), so leaves are never index-backed here.
            TableInfo::new(leaf.id.clone(), leaf.estimated_rows)
                .with_index(false)
                .with_bit_id(i as u32)
        })
        .collect();

    let table_index: HashMap<&str, usize> = tables
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.as_str(), i))
        .collect();

    let conditions: Vec<JoinCondition> = chain
        .predicates
        .iter()
        .map(|p| {
            let left = p.left_table.as_str();
            let right = p.right_table.as_str();
            let (left_id, right_id) = if left == right || left.is_empty() || right.is_empty() {
                let lid = resolve_fallback(&p.left_key, &table_index, &tables);
                let rid = resolve_fallback(&p.right_key, &table_index, &tables);
                (lid, rid)
            } else {
                (left.to_string(), right.to_string())
            };
            JoinCondition::new(left_id, right_id).with_selectivity(p.selectivity)
        })
        .collect();

    (tables, conditions)
}

/// Rebuild a reordered logical join tree from the optimizer result.
pub fn reconstruct_join_tree_logical(
    original_root: &LogicalNodeEnum,
    chain: &FlattenedJoinChainLogical,
    result: &JoinOrderResult,
) -> LogicalNodeEnum {
    let original_type = classify_join_logical(original_root);
    if original_type != JoinNodeType::Inner && original_type != JoinNodeType::Cross {
        return original_root.clone();
    }

    let leaf_map: HashMap<&str, &LogicalNodeEnum> = chain
        .leaves
        .iter()
        .map(|l| (l.id.as_str(), &l.logical_node))
        .collect();

    let leaf_rows: HashMap<&str, u64> = chain
        .leaves
        .iter()
        .map(|l| (l.id.as_str(), l.estimated_rows))
        .collect();

    let mut pred_map: PredMap = HashMap::new();
    for p in &chain.predicates {
        let (a, b) = if p.left_table <= p.right_table {
            (p.left_table.clone(), p.right_table.clone())
        } else {
            (p.right_table.clone(), p.left_table.clone())
        };
        pred_map
            .entry((a, b))
            .or_default()
            .push((p.left_key.clone(), p.right_key.clone()));
    }

    let mut current: Option<LogicalNodeEnum> = None;
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
                let lid = leaf_id_logical(&left);
                let rid = leaf_id_logical(&right_node);
                let pair_key = if lid <= rid {
                    (lid.clone(), rid.clone())
                } else {
                    (rid.clone(), lid.clone())
                };
                let (hash_keys, probe_keys) =
                    resolve_keys_for_pair_logical(&pair_key, &pred_map, &left, &right_node);
                let has_hash_keys = !hash_keys.is_empty();
                let recommended_algorithm = normalize_join_algorithm(
                    result.algorithms.get(step),
                    has_hash_keys,
                    accumulated_rows,
                    right_rows,
                );
                step += 1;
                let joined = build_logical_inner_join(
                    left,
                    right_node,
                    hash_keys,
                    probe_keys,
                    recommended_algorithm,
                );
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
                accumulated_rows =
                    ((accumulated_rows as f64 * right_rows as f64 * selectivity) as u64).max(1);
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

/// Normalize a cost-based join algorithm decision for the logical tree.
///
/// Mirrors the safety gates of the physical `record_join_algorithm`:
/// `HashJoin` requires valid equi keys, `NestedLoopJoin` requires trusted
/// row estimates, and `IndexJoin` has no executor yet so no algorithm is
/// recommended. Returns the algorithm to stamp on
/// `LogicalInnerJoinNode.recommended_algorithm`, or `None` to keep the
/// default heuristic.
fn normalize_join_algorithm(
    algorithm: Option<&JoinAlgorithm>,
    has_hash_keys: bool,
    left_rows: u64,
    right_rows: u64,
) -> Option<JoinAlgorithm> {
    let algorithm = algorithm?;
    match algorithm {
        JoinAlgorithm::NestedLoopJoin { .. } => {
            if left_rows > 0 && right_rows > 0 {
                Some(algorithm.clone())
            } else {
                None
            }
        }
        JoinAlgorithm::HashJoin { .. } => {
            if has_hash_keys {
                Some(algorithm.clone())
            } else {
                None
            }
        }
        JoinAlgorithm::IndexJoin { .. } => None,
    }
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
fn review_accepts_order(
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

fn resolve_keys_for_pair_logical(
    pair_key: &(String, String),
    pred_map: &PredMap,
    left_logical: &LogicalNodeEnum,
    right_logical: &LogicalNodeEnum,
) -> (Vec<ContextualExpression>, Vec<ContextualExpression>) {
    if let Some(keys_list) = pred_map.get(pair_key) {
        if let Some((hk, pk)) = keys_list.first() {
            let left_id = leaf_id_logical(left_logical);
            let right_id = leaf_id_logical(right_logical);
            let left_vars = collect_variables_from_slice(hk);
            let right_vars = collect_variables_from_slice(pk);

            let swap = left_vars
                .iter()
                .any(|v| right_id.contains(v) || v.contains(&right_id))
                || right_vars
                    .iter()
                    .any(|v| left_id.contains(v) || v.contains(&left_id));
            if swap {
                return (pk.clone(), hk.clone());
            }
            return (hk.clone(), pk.clone());
        }
    }
    (vec![], vec![])
}

/// Build a logical inner join, mirroring the physical `InnerJoinNode::new`
/// column-name merge semantics.
fn build_logical_inner_join(
    left: LogicalNodeEnum,
    right: LogicalNodeEnum,
    hash_keys: Vec<ContextualExpression>,
    probe_keys: Vec<ContextualExpression>,
    recommended_algorithm: Option<JoinAlgorithm>,
) -> LogicalNodeEnum {
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;

    let mut col_names = left.col_names().to_vec();
    let right_col_names = right.col_names();
    for col in right_col_names {
        if !col_names.contains(col) {
            col_names.push(col.clone());
        } else {
            let mut idx = 1;
            let mut new_col = format!("{}_{}", col, idx);
            while col_names.contains(&new_col) {
                idx += 1;
                new_col = format!("{}_{}", col, idx);
            }
            col_names.push(new_col);
        }
    }

    let mut column_types = logical_column_types(&left);
    column_types.extend(logical_column_types(&right));

    LogicalNodeEnum::InnerJoin(LogicalInnerJoinNode {
        id: next_node_id(),
        left: Box::new(left),
        right: Box::new(right),
        hash_keys,
        probe_keys,
        recommended_algorithm,
        output_var: None,
        col_names,
        column_types,
    })
}

fn try_optimize_join_tree_logical(
    root: &LogicalNodeEnum,
    stats: &StatsView,
    cost_calculator: &CostCalculator,
) -> OptResultLogical {
    let Some(mut chain) = flatten_join_chain_logical(root) else {
        return OptResultLogical::Unchanged;
    };

    if chain.leaves.len() < 2 {
        return OptResultLogical::Unchanged;
    }

    assign_leaf_info_logical(&mut chain, stats);

    for pred in &mut chain.predicates {
        pred.selectivity =
            refine_join_selectivity(stats, &pred.left_key, &pred.right_key, pred.selectivity);
    }

    let (tables, conditions) = build_optimizer_input_logical(&chain);

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
        return OptResultLogical::Unchanged;
    };

    let note = format!(
        "join_order: {} tables, method={:?}, order=[{}], cost {:.1}->{:.1} (reviewer)",
        chain.leaves.len(),
        result.optimization_method,
        result.order.join(", "),
        current_cost,
        result.total_cost,
    );
    OptResultLogical::Changed(
        Box::new(reconstruct_join_tree_logical(root, &chain, &result)),
        note,
    )
}

/// Walk a logical plan tree and record the join order decision for every
/// reorderable join chain as a CBO note, returning the reordered logical
/// tree. The reorder decision is taken on the pure logical operators; the
/// physical walker applies the corresponding rewrite to the executable root.
pub fn walk_and_optimize_joins_logical(
    root: &LogicalNodeEnum,
    stats: &StatsView,
    cost_calculator: &CostCalculator,
    notes: &mut Vec<String>,
) -> LogicalNodeEnum {
    if let OptResultLogical::Changed(optimized, note) =
        try_optimize_join_tree_logical(root, stats, cost_calculator)
    {
        notes.push(note);
        return *optimized;
    }

    match root {
        LogicalNodeEnum::Project(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Project(cloned)
        }
        LogicalNodeEnum::Filter(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Filter(cloned)
        }
        LogicalNodeEnum::Sort(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Sort(cloned)
        }
        LogicalNodeEnum::Limit(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Limit(cloned)
        }
        LogicalNodeEnum::Skip(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Skip(cloned)
        }
        LogicalNodeEnum::TopN(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::TopN(cloned)
        }
        LogicalNodeEnum::Sample(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Sample(cloned)
        }
        LogicalNodeEnum::Dedup(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Dedup(cloned)
        }
        LogicalNodeEnum::Aggregate(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Aggregate(cloned)
        }
        LogicalNodeEnum::Window(n) => {
            let new_input =
                walk_and_optimize_joins_logical(n.input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            LogicalNodeEnum::Window(cloned)
        }
        LogicalNodeEnum::LeftJoin(n) => {
            let new_left =
                walk_and_optimize_joins_logical(n.left_input(), stats, cost_calculator, notes);
            let new_right =
                walk_and_optimize_joins_logical(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            LogicalNodeEnum::LeftJoin(cloned)
        }
        LogicalNodeEnum::RightJoin(n) => {
            let new_left =
                walk_and_optimize_joins_logical(n.left_input(), stats, cost_calculator, notes);
            let new_right =
                walk_and_optimize_joins_logical(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            LogicalNodeEnum::RightJoin(cloned)
        }
        LogicalNodeEnum::FullOuterJoin(n) => {
            let new_left =
                walk_and_optimize_joins_logical(n.left_input(), stats, cost_calculator, notes);
            let new_right =
                walk_and_optimize_joins_logical(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            LogicalNodeEnum::FullOuterJoin(cloned)
        }
        LogicalNodeEnum::SemiJoin(n) => {
            let new_left =
                walk_and_optimize_joins_logical(n.left_input(), stats, cost_calculator, notes);
            let new_right =
                walk_and_optimize_joins_logical(n.right_input(), stats, cost_calculator, notes);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            LogicalNodeEnum::SemiJoin(cloned)
        }
        _ => root.clone(),
    }
}

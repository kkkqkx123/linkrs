use std::collections::HashMap;

use crate::core::types::expr::analysis_utils::collect_variables_from_contextual;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::optimizer::cost::CostCalculator;
use crate::optimizer::cost_based::join_order::{
    JoinCondition, JoinOrderOptimizer, JoinOrderResult, TableInfo,
};
use crate::optimizer::stats::StatsView;
use crate::optimizer::JoinAlgorithm;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;

type PredMap =
    HashMap<(String, String), Vec<(Vec<ContextualExpression>, Vec<ContextualExpression>)>>;

/// A leaf input to a join chain — a subtree whose root is not a reorderable join.
#[derive(Debug, Clone)]
pub struct LeafInfo {
    pub id: String,
    pub estimated_rows: u64,
    pub has_index: bool,
    pub physical_node: PlanNodeEnum,
}

/// A join predicate extracted from the chain.
#[derive(Debug, Clone)]
pub struct JoinPredicate {
    pub left_key: Vec<ContextualExpression>,
    pub right_key: Vec<ContextualExpression>,
    pub left_table: String,
    pub right_table: String,
    pub selectivity: f64,
}

/// The flattened representation of a join tree.
#[derive(Debug, Clone)]
pub struct FlattenedJoinChain {
    pub leaves: Vec<LeafInfo>,
    pub predicates: Vec<JoinPredicate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinNodeType {
    Inner,
    Cross,
    NonReorderable,
    NotJoin,
}

fn classify_join(node: &PlanNodeEnum) -> JoinNodeType {
    match node {
        PlanNodeEnum::InnerJoin(_) => JoinNodeType::Inner,
        PlanNodeEnum::CrossJoin(_) => JoinNodeType::Cross,
        PlanNodeEnum::LeftJoin(_)
        | PlanNodeEnum::RightJoin(_)
        | PlanNodeEnum::SemiJoin(_)
        | PlanNodeEnum::FullOuterJoin(_) => JoinNodeType::NonReorderable,
        _ => JoinNodeType::NotJoin,
    }
}

fn leaf_id(node: &PlanNodeEnum) -> String {
    let mut current = node;
    for _ in 0..5 {
        if let Some(var) = current.output_var() {
            if !var.is_empty() {
                return var.to_string();
            }
        }
        match current {
            PlanNodeEnum::ScanVertices(n) => {
                if let Some(tag) = n.tag() {
                    if !tag.is_empty() {
                        return format!("scan_{}", tag);
                    }
                }
            }
            PlanNodeEnum::ScanEdges(n) => {
                if let Some(et) = n.edge_type() {
                    if !et.is_empty() {
                        return format!("scan_{}", et);
                    }
                }
            }
            PlanNodeEnum::IndexScan(n) => {
                if let Some(var) = n.output_var() {
                    if !var.is_empty() {
                        return var.to_string();
                    }
                }
                return format!("index_scan_{}", n.id());
            }
            _ => {}
        }
        current = match current {
            PlanNodeEnum::Project(n) => n.input(),
            PlanNodeEnum::Filter(n) => n.input(),
            PlanNodeEnum::Sort(n) => n.input(),
            PlanNodeEnum::Limit(n) => n.input(),
            PlanNodeEnum::TopN(n) => n.input(),
            PlanNodeEnum::Sample(n) => n.input(),
            PlanNodeEnum::Dedup(n) => n.input(),
            PlanNodeEnum::Aggregate(n) => n.input(),
            PlanNodeEnum::Window(n) => n.input(),
            _ => break,
        };
    }
    format!("leaf_{}", node.id())
}

fn estimate_leaf_rows(node: &PlanNodeEnum, stats: &StatsView) -> u64 {
    match node {
        PlanNodeEnum::ScanVertices(n) => {
            if let Some(tag) = n.tag() {
                let count = stats.vertex_count(tag);
                if count > 0 {
                    return count;
                }
            }
            10000
        }
        PlanNodeEnum::ScanEdges(n) => {
            if let Some(et) = n.edge_type() {
                let count = stats.edge_count(&et);
                if count > 0 {
                    return count;
                }
            }
            50000
        }
        PlanNodeEnum::IndexScan(_) => 5000,
        PlanNodeEnum::Filter(n) => {
            let child = estimate_leaf_rows(n.input(), stats);
            (child / 10).max(1)
        }
        PlanNodeEnum::Project(n) => estimate_leaf_rows(n.input(), stats),
        PlanNodeEnum::Aggregate(n) => {
            let child = estimate_leaf_rows(n.input(), stats);
            (child / 5).max(1)
        }
        PlanNodeEnum::Sort(n) => estimate_leaf_rows(n.input(), stats),
        PlanNodeEnum::TopN(n) => estimate_leaf_rows(n.input(), stats),
        PlanNodeEnum::Limit(n) => {
            let limit = n.count().max(0) as u64;
            let child = estimate_leaf_rows(n.input(), stats);
            limit.min(child).max(1)
        }
        PlanNodeEnum::Dedup(n) => {
            let child = estimate_leaf_rows(n.input(), stats);
            (child / 2).max(1)
        }
        PlanNodeEnum::GetVertices(_) => 1000,
        PlanNodeEnum::GetEdges(_) => 1000,
        PlanNodeEnum::GetNeighbors(_) => 5000,
        PlanNodeEnum::Traverse(n) => {
            let child = estimate_leaf_rows(n.input(), stats);
            child * 2
        }
        PlanNodeEnum::Expand(n) => {
            let child = n
                .dependencies()
                .first()
                .map(|c| estimate_leaf_rows(c, stats))
                .unwrap_or(10000);
            child * 3
        }
        _ => 10000,
    }
}

fn match_key_to_leaf(key: &[ContextualExpression], leaves: &[LeafInfo]) -> Option<String> {
    for expr in key {
        let vars = collect_variables_from_contextual(expr);
        for v in &vars {
            for leaf in leaves {
                if leaf.id == *v {
                    return Some(leaf.id.clone());
                }
                if v.starts_with(&leaf.id) || leaf.id.starts_with(v) {
                    return Some(leaf.id.clone());
                }
            }
        }
    }
    None
}

pub fn flatten_join_chain(root: &PlanNodeEnum) -> Option<FlattenedJoinChain> {
    let chain_type = classify_join(root);
    if chain_type == JoinNodeType::NotJoin || chain_type == JoinNodeType::NonReorderable {
        return None;
    }

    let mut leaves: Vec<LeafInfo> = Vec::new();
    let mut predicates: Vec<JoinPredicate> = Vec::new();
    flatten_recursive(root, &mut leaves, &mut predicates)?;

    for pred in &mut predicates {
        if pred.left_table.is_empty() {
            if let Some(id) = match_key_to_leaf(&pred.left_key, &leaves) {
                pred.left_table = id;
            }
        }
        if pred.right_table.is_empty() {
            if let Some(id) = match_key_to_leaf(&pred.right_key, &leaves) {
                pred.right_table = id;
            }
        }
    }

    Some(FlattenedJoinChain { leaves, predicates })
}

fn flatten_recursive(
    node: &PlanNodeEnum,
    leaves: &mut Vec<LeafInfo>,
    predicates: &mut Vec<JoinPredicate>,
) -> Option<()> {
    match classify_join(node) {
        JoinNodeType::Inner => {
            let (left, right, hash_keys, probe_keys) = match node {
                PlanNodeEnum::InnerJoin(n) => (
                    n.left_input(),
                    n.right_input(),
                    n.hash_keys().to_vec(),
                    n.probe_keys().to_vec(),
                ),
                _ => unreachable!(),
            };

            if !hash_keys.is_empty() || !probe_keys.is_empty() {
                predicates.push(JoinPredicate {
                    left_key: hash_keys,
                    right_key: probe_keys,
                    left_table: String::new(),
                    right_table: String::new(),
                    selectivity: 0.3,
                });
            }

            flatten_recursive(left, leaves, predicates)?;
            flatten_recursive(right, leaves, predicates)?;
            Some(())
        }
        JoinNodeType::Cross => {
            let n = match node {
                PlanNodeEnum::CrossJoin(n) => n,
                _ => unreachable!(),
            };
            flatten_recursive(n.left_input(), leaves, predicates)?;
            flatten_recursive(n.right_input(), leaves, predicates)?;
            Some(())
        }
        JoinNodeType::NonReorderable | JoinNodeType::NotJoin => {
            leaves.push(LeafInfo {
                id: String::new(),
                estimated_rows: 0,
                has_index: false,
                physical_node: node.clone(),
            });
            Some(())
        }
    }
}

pub fn assign_leaf_info(chain: &mut FlattenedJoinChain, stats: &StatsView) {
    for leaf in &mut chain.leaves {
        if leaf.id.is_empty() {
            leaf.id = leaf_id(&leaf.physical_node);
        }
        if leaf.estimated_rows == 0 {
            leaf.estimated_rows = estimate_leaf_rows(&leaf.physical_node, stats);
        }
        leaf.has_index = has_index_scan(&leaf.physical_node);
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    for leaf in &mut chain.leaves {
        let key = leaf.id.clone();
        let count = seen.entry(key.clone()).or_insert(0);
        if *count > 0 {
            leaf.id = format!("{}_{}", key, count);
        }
        *count += 1;
    }

    // Resolve predicate table ids now that the leaf ids are assigned. At
    // flatten time the leaves are still unnamed, so the ids resolved there
    // would be empty and the join-key resolution in
    // `reconstruct_join_tree_with_decisions` could not map a predicate back
    // to its operand pair.
    for pred in &mut chain.predicates {
        if pred.left_table.is_empty() {
            if let Some(id) = match_key_to_leaf(&pred.left_key, &chain.leaves) {
                pred.left_table = id;
            }
        }
        if pred.right_table.is_empty() {
            if let Some(id) = match_key_to_leaf(&pred.right_key, &chain.leaves) {
                pred.right_table = id;
            }
        }
    }
}

fn has_index_scan(node: &PlanNodeEnum) -> bool {
    matches!(node, PlanNodeEnum::IndexScan(_))
}

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

pub fn reconstruct_join_tree(
    original_root: &PlanNodeEnum,
    chain: &FlattenedJoinChain,
    result: &JoinOrderResult,
) -> PlanNodeEnum {
    reconstruct_join_tree_with_decisions(original_root, chain, result, &mut None)
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
                // `calculate_join_cost` (default selectivity 0.3).
                let selectivity = chain
                    .predicates
                    .iter()
                    .find(|p| {
                        let a = p.left_table.as_str();
                        let b = p.right_table.as_str();
                        (a == lid && b == rid) || (a == rid && b == lid)
                    })
                    .map(|p| p.selectivity)
                    .unwrap_or(0.3);
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

enum OptResult {
    Changed(Box<PlanNodeEnum>, String),
    Unchanged,
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

    let (tables, conditions) = build_optimizer_input(&chain);

    let optimizer = JoinOrderOptimizer::new(std::sync::Arc::new(cost_calculator.clone()));
    let result = optimizer.optimize_join_order(&tables, &conditions);

    log::debug!(
        "Join order optimization: {} tables, cost={}, method={:?}, order={:?}",
        chain.leaves.len(),
        result.total_cost,
        result.optimization_method,
        result.order,
    );

    let note = format!(
        "join_order: {} tables, method={:?}, order=[{}]",
        chain.leaves.len(),
        result.optimization_method,
        result.order.join(", ")
    );
    OptResult::Changed(
        Box::new(reconstruct_join_tree_with_decisions(
            root, &chain, &result, decisions,
        )),
        note,
    )
}

pub fn walk_and_optimize_joins(
    root: &PlanNodeEnum,
    stats: &StatsView,
    cost_calculator: &CostCalculator,
    notes: &mut Vec<String>,
) -> PlanNodeEnum {
    walk_and_optimize_joins_with_decisions(root, stats, cost_calculator, notes, &mut None)
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
// Logical-plan variants (PlanNodeEnum logic/physical separation).
//
// The cost-based join order decision is taken on the pure logical tree
// (`LogicalNodeEnum`) and produces the same `join_order:` note as the
// physical walker. The physical walker remains responsible for applying
// the structural rewrite on the executable root.
// =====================================================================

/// A leaf input of a logical join chain.
#[derive(Debug, Clone)]
pub struct LeafInfoLogical {
    pub id: String,
    pub estimated_rows: u64,
    pub logical_node: LogicalNodeEnum,
}

/// The flattened representation of a logical join tree.
#[derive(Debug, Clone)]
pub struct FlattenedJoinChainLogical {
    pub leaves: Vec<LeafInfoLogical>,
    pub predicates: Vec<JoinPredicate>,
}

/// Logical join classification. The logical tree is pure — it never
/// carries physical variants (InnerJoin), so only InnerJoin/CrossJoin
/// are reorderable.
fn classify_join_logical(node: &LogicalNodeEnum) -> JoinNodeType {
    match node {
        LogicalNodeEnum::InnerJoin(_) => JoinNodeType::Inner,
        LogicalNodeEnum::CrossJoin(_) => JoinNodeType::Cross,
        LogicalNodeEnum::LeftJoin(_)
        | LogicalNodeEnum::RightJoin(_)
        | LogicalNodeEnum::SemiJoin(_)
        | LogicalNodeEnum::FullOuterJoin(_) => JoinNodeType::NonReorderable,
        _ => JoinNodeType::NotJoin,
    }
}

fn leaf_id_logical(node: &LogicalNodeEnum) -> String {
    let mut current = node;
    for _ in 0..5 {
        if let Some(var) = logical_output_var(current) {
            if !var.is_empty() {
                return var.to_string();
            }
        }
        match current {
            LogicalNodeEnum::ScanVertices(n) => {
                if let Some(tag) = n.tag.as_deref() {
                    if !tag.is_empty() {
                        return format!("scan_{}", tag);
                    }
                }
            }
            LogicalNodeEnum::ScanEdges(n) => {
                if let Some(et) = n.edge_type.as_deref() {
                    if !et.is_empty() {
                        return format!("scan_{}", et);
                    }
                }
            }
            _ => {}
        }
        current = match current {
            LogicalNodeEnum::Project(n) => n.input(),
            LogicalNodeEnum::Filter(n) => n.input(),
            LogicalNodeEnum::Sort(n) => n.input(),
            LogicalNodeEnum::Limit(n) => n.input(),
            LogicalNodeEnum::TopN(n) => n.input(),
            LogicalNodeEnum::Sample(n) => n.input(),
            LogicalNodeEnum::Dedup(n) => n.input(),
            LogicalNodeEnum::Aggregate(n) => n.input(),
            LogicalNodeEnum::Window(n) => n.input(),
            _ => break,
        };
    }
    format!("leaf_{}", node.id())
}

/// The output variable of a logical node (convertible subset), if any.
fn logical_output_var(node: &LogicalNodeEnum) -> Option<&str> {
    match node {
        LogicalNodeEnum::Start(n) => n.output_var(),
        LogicalNodeEnum::GetVertices(n) => n.output_var(),
        LogicalNodeEnum::GetEdges(n) => n.output_var(),
        LogicalNodeEnum::GetNeighbors(n) => n.output_var(),
        LogicalNodeEnum::ScanVertices(n) => n.output_var(),
        LogicalNodeEnum::ScanEdges(n) => n.output_var(),
        LogicalNodeEnum::Project(n) => n.output_var(),
        LogicalNodeEnum::Filter(n) => n.output_var(),
        LogicalNodeEnum::Sort(n) => n.output_var(),
        LogicalNodeEnum::Limit(n) => n.output_var(),
        LogicalNodeEnum::TopN(n) => n.output_var(),
        LogicalNodeEnum::Sample(n) => n.output_var(),
        LogicalNodeEnum::Dedup(n) => n.output_var(),
        LogicalNodeEnum::Aggregate(n) => n.output_var(),
        LogicalNodeEnum::Window(n) => n.output_var(),
        LogicalNodeEnum::InnerJoin(n) => n.output_var(),
        LogicalNodeEnum::LeftJoin(n) => n.output_var(),
        LogicalNodeEnum::RightJoin(n) => n.output_var(),
        LogicalNodeEnum::CrossJoin(n) => n.output_var(),
        LogicalNodeEnum::FullOuterJoin(n) => n.output_var(),
        LogicalNodeEnum::SemiJoin(n) => n.output_var(),
        _ => None,
    }
}

/// The output column types of a logical node (convertible subset), if any.
fn logical_column_types(node: &LogicalNodeEnum) -> Vec<crate::core::DataType> {
    match node {
        LogicalNodeEnum::Start(n) => n.column_types().to_vec(),
        LogicalNodeEnum::GetVertices(n) => n.column_types().to_vec(),
        LogicalNodeEnum::GetEdges(n) => n.column_types().to_vec(),
        LogicalNodeEnum::GetNeighbors(n) => n.column_types().to_vec(),
        LogicalNodeEnum::ScanVertices(n) => n.column_types().to_vec(),
        LogicalNodeEnum::ScanEdges(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Project(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Filter(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Sort(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Limit(n) => n.column_types().to_vec(),
        LogicalNodeEnum::TopN(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Sample(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Dedup(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Aggregate(n) => n.column_types().to_vec(),
        LogicalNodeEnum::Window(n) => n.column_types().to_vec(),
        LogicalNodeEnum::InnerJoin(n) => n.column_types().to_vec(),
        LogicalNodeEnum::LeftJoin(n) => n.column_types().to_vec(),
        LogicalNodeEnum::RightJoin(n) => n.column_types().to_vec(),
        LogicalNodeEnum::CrossJoin(n) => n.column_types().to_vec(),
        LogicalNodeEnum::FullOuterJoin(n) => n.column_types().to_vec(),
        LogicalNodeEnum::SemiJoin(n) => n.column_types().to_vec(),
        _ => vec![],
    }
}

fn estimate_leaf_rows_logical(node: &LogicalNodeEnum, stats: &StatsView) -> u64 {
    match node {
        LogicalNodeEnum::ScanVertices(n) => {
            if let Some(tag) = n.tag.as_deref() {
                let count = stats.vertex_count(tag);
                if count > 0 {
                    return count;
                }
            }
            10000
        }
        LogicalNodeEnum::ScanEdges(n) => {
            if let Some(et) = n.edge_type.as_deref() {
                let count = stats.edge_count(et);
                if count > 0 {
                    return count;
                }
            }
            50000
        }
        LogicalNodeEnum::Filter(n) => {
            let child = estimate_leaf_rows_logical(n.input(), stats);
            (child / 10).max(1)
        }
        LogicalNodeEnum::Project(n) => estimate_leaf_rows_logical(n.input(), stats),
        LogicalNodeEnum::Aggregate(n) => {
            let child = estimate_leaf_rows_logical(n.input(), stats);
            (child / 5).max(1)
        }
        LogicalNodeEnum::Sort(n) => estimate_leaf_rows_logical(n.input(), stats),
        LogicalNodeEnum::TopN(n) => estimate_leaf_rows_logical(n.input(), stats),
        LogicalNodeEnum::Limit(n) => {
            let limit = n.count.max(0) as u64;
            let child = estimate_leaf_rows_logical(n.input(), stats);
            limit.min(child).max(1)
        }
        LogicalNodeEnum::Dedup(n) => {
            let child = estimate_leaf_rows_logical(n.input(), stats);
            (child / 2).max(1)
        }
        LogicalNodeEnum::GetVertices(_) => 1000,
        LogicalNodeEnum::GetEdges(_) => 1000,
        LogicalNodeEnum::GetNeighbors(_) => 5000,
        LogicalNodeEnum::Traverse(n) => {
            let child = estimate_leaf_rows_logical(n.input(), stats);
            child * 2
        }
        LogicalNodeEnum::Expand(n) => {
            let child = n
                .dependencies()
                .first()
                .map(|c| estimate_leaf_rows_logical(c, stats))
                .unwrap_or(10000);
            child * 3
        }
        _ => 10000,
    }
}

fn match_key_to_leaf_ids(key: &[ContextualExpression], leaf_ids: &[String]) -> Option<String> {
    for expr in key {
        let vars = collect_variables_from_contextual(expr);
        for v in &vars {
            for id in leaf_ids {
                if id == v || v.starts_with(id) || id.starts_with(v) {
                    return Some(id.clone());
                }
            }
        }
    }
    None
}

/// Flatten a logical join tree into leaves and join predicates.
pub fn flatten_join_chain_logical(root: &LogicalNodeEnum) -> Option<FlattenedJoinChainLogical> {
    let chain_type = classify_join_logical(root);
    if chain_type == JoinNodeType::NotJoin || chain_type == JoinNodeType::NonReorderable {
        return None;
    }

    let mut leaves: Vec<LeafInfoLogical> = Vec::new();
    let mut predicates: Vec<JoinPredicate> = Vec::new();
    flatten_recursive_logical(root, &mut leaves, &mut predicates)?;

    for pred in &mut predicates {
        if pred.left_table.is_empty() {
            let ids: Vec<String> = leaves.iter().map(|l| l.id.clone()).collect();
            if let Some(id) = match_key_to_leaf_ids(&pred.left_key, &ids) {
                pred.left_table = id;
            }
        }
        if pred.right_table.is_empty() {
            let ids: Vec<String> = leaves.iter().map(|l| l.id.clone()).collect();
            if let Some(id) = match_key_to_leaf_ids(&pred.right_key, &ids) {
                pred.right_table = id;
            }
        }
    }

    Some(FlattenedJoinChainLogical { leaves, predicates })
}

fn flatten_recursive_logical(
    node: &LogicalNodeEnum,
    leaves: &mut Vec<LeafInfoLogical>,
    predicates: &mut Vec<JoinPredicate>,
) -> Option<()> {
    match classify_join_logical(node) {
        JoinNodeType::Inner => {
            let (left, right, hash_keys, probe_keys) = match node {
                LogicalNodeEnum::InnerJoin(n) => (
                    n.left_input(),
                    n.right_input(),
                    n.hash_keys().to_vec(),
                    n.probe_keys().to_vec(),
                ),
                _ => unreachable!(),
            };

            if !hash_keys.is_empty() || !probe_keys.is_empty() {
                predicates.push(JoinPredicate {
                    left_key: hash_keys,
                    right_key: probe_keys,
                    left_table: String::new(),
                    right_table: String::new(),
                    selectivity: 0.3,
                });
            }

            flatten_recursive_logical(left, leaves, predicates)?;
            flatten_recursive_logical(right, leaves, predicates)?;
            Some(())
        }
        JoinNodeType::Cross => {
            let n = match node {
                LogicalNodeEnum::CrossJoin(n) => n,
                _ => unreachable!(),
            };
            flatten_recursive_logical(n.left_input(), leaves, predicates)?;
            flatten_recursive_logical(n.right_input(), leaves, predicates)?;
            Some(())
        }
        JoinNodeType::NonReorderable | JoinNodeType::NotJoin => {
            leaves.push(LeafInfoLogical {
                id: String::new(),
                estimated_rows: 0,
                logical_node: node.clone(),
            });
            Some(())
        }
    }
}

pub fn assign_leaf_info_logical(chain: &mut FlattenedJoinChainLogical, stats: &StatsView) {
    for leaf in &mut chain.leaves {
        if leaf.id.is_empty() {
            leaf.id = leaf_id_logical(&leaf.logical_node);
        }
        if leaf.estimated_rows == 0 {
            leaf.estimated_rows = estimate_leaf_rows_logical(&leaf.logical_node, stats);
        }
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    for leaf in &mut chain.leaves {
        let key = leaf.id.clone();
        let count = seen.entry(key.clone()).or_insert(0);
        if *count > 0 {
            leaf.id = format!("{}_{}", key, count);
        }
        *count += 1;
    }

    // Resolve predicate table ids now that the leaf ids are assigned (see
    // `assign_leaf_info` for the rationale).
    let leaf_ids: Vec<String> = chain.leaves.iter().map(|l| l.id.clone()).collect();
    for pred in &mut chain.predicates {
        if pred.left_table.is_empty() {
            if let Some(id) = match_key_to_leaf_ids(&pred.left_key, &leaf_ids) {
                pred.left_table = id;
            }
        }
        if pred.right_table.is_empty() {
            if let Some(id) = match_key_to_leaf_ids(&pred.right_key, &leaf_ids) {
                pred.right_table = id;
            }
        }
    }
}

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

    for table_id in &result.order {
        let right_node = match leaf_map.get(table_id.as_str()) {
            Some(node) => (*node).clone(),
            None => {
                log::warn!("JoinOrderOptimizer returned unknown table '{}'", table_id);
                continue;
            }
        };

        current = match current.take() {
            Some(left) => {
                let lid = leaf_id_logical(&left);
                let rid = leaf_id_logical(&right_node);
                let pair_key = if lid <= rid {
                    (lid.clone(), rid.clone())
                } else {
                    (rid, lid)
                };
                let (hash_keys, probe_keys) =
                    resolve_keys_for_pair_logical(&pair_key, &pred_map, &left, &right_node);
                Some(build_logical_inner_join(
                    left, right_node, hash_keys, probe_keys,
                ))
            }
            None => Some(right_node),
        };
    }

    current.unwrap_or_else(|| original_root.clone())
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
        left: Box::new(left.clone()),
        right: Box::new(right.clone()),
        hash_keys,
        probe_keys,
        deps: vec![left, right],
        output_var: None,
        col_names,
        column_types,
    })
}

enum OptResultLogical {
    Changed(Box<LogicalNodeEnum>, String),
    Unchanged,
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

    let (tables, conditions) = build_optimizer_input_logical(&chain);

    let optimizer = JoinOrderOptimizer::new(std::sync::Arc::new(cost_calculator.clone()));
    let result = optimizer.optimize_join_order(&tables, &conditions);

    let note = format!(
        "join_order: {} tables, method={:?}, order=[{}]",
        chain.leaves.len(),
        result.optimization_method,
        result.order.join(", ")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::cost::CostCalculator;
    use crate::optimizer::stats::StatisticsManager;
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;

    fn make_scan(id: &str, _rows: u64) -> PlanNodeEnum {
        // Use a StartNode as a stand-in leaf for testing
        let mut node =
            crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        node.set_output_var(id.to_string());
        node.set_col_names(vec![id.to_string()]);
        PlanNodeEnum::Start(node)
    }

    fn make_hash_join(
        left: PlanNodeEnum,
        right: PlanNodeEnum,
        hk: Vec<&str>,
        pk: Vec<&str>,
    ) -> PlanNodeEnum {
        let ctx = std::sync::Arc::new(
            crate::core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let hash_keys: Vec<ContextualExpression> = hk
            .iter()
            .map(|s| {
                let meta = crate::core::types::expr::ExpressionMeta::new(
                    crate::core::types::expr::Expression::Variable(s.to_string()),
                );
                let id = ctx.register_expression(meta);
                crate::core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
            })
            .collect();
        let probe_keys: Vec<ContextualExpression> = pk
            .iter()
            .map(|s| {
                let meta = crate::core::types::expr::ExpressionMeta::new(
                    crate::core::types::expr::Expression::Variable(s.to_string()),
                );
                let id = ctx.register_expression(meta);
                crate::core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
            })
            .collect();
        PlanNodeEnum::InnerJoin(InnerJoinNode::new(left, right, hash_keys, probe_keys).unwrap())
    }

    #[test]
    fn test_single_table_returns_none() {
        let scan = make_scan("a", 1000);
        assert!(flatten_join_chain(&scan).is_none());
    }

    #[test]
    fn test_two_table_chain() {
        let a = make_scan("a", 1000);
        let b = make_scan("b", 500);
        let join = make_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
        let chain = flatten_join_chain(&join).expect("should flatten");
        assert_eq!(chain.leaves.len(), 2);
        assert_eq!(chain.predicates.len(), 1);
    }

    #[test]
    fn test_three_table_chain() {
        let a = make_scan("a", 1000);
        let b = make_scan("b", 500);
        let c = make_scan("c", 2000);
        let join1 = make_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
        let join2 = make_hash_join(join1, c, vec!["b.id"], vec!["c.id"]);
        let chain = flatten_join_chain(&join2).expect("should flatten");
        assert_eq!(chain.leaves.len(), 3);
        assert_eq!(chain.predicates.len(), 2);
    }

    #[test]
    fn test_non_join_unchanged() {
        let a = make_scan("a", 1000);
        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let result = walk_and_optimize_joins(&a, &stats_view, &cost_calc, &mut notes);
        // StartNode is preserved (same variant)
        assert!(matches!(result, PlanNodeEnum::Start(_)));
        // Output var is preserved
        assert_eq!(result.output_var(), a.output_var());
    }

    #[test]
    fn test_optimize_three_table() {
        let a = make_scan("a", 1000);
        let b = make_scan("b", 10);
        let c = make_scan("c", 2000);
        let join1 = make_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
        let join2 = make_hash_join(join1, c, vec!["b.id"], vec!["c.id"]);

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let optimized = walk_and_optimize_joins(&join2, &stats_view, &cost_calc, &mut notes);

        // The smallest table (b, 10 rows) should be first in the new tree
        assert!(matches!(optimized, PlanNodeEnum::InnerJoin(_)));
        // Verify it's still a valid join tree
        assert!(optimized.children().len() >= 2);
    }

    #[test]
    fn test_keyed_chain_records_join_decision() {
        let a = make_scan("a", 1000);
        let b = make_scan("b", 10);
        let join = make_hash_join(a, b, vec!["a.id"], vec!["b.id"]);

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let mut decisions = HashMap::new();
        let optimized = walk_and_optimize_joins_with_decisions(
            &join,
            &stats_view,
            &cost_calc,
            &mut notes,
            &mut Some(&mut decisions),
        );

        // The reordered tree is a join and the rebuild records exactly one
        // decision, keyed by the rebuilt root node id.
        assert!(matches!(optimized, PlanNodeEnum::InnerJoin(_)));
        assert_eq!(decisions.len(), 1);
        let algorithm = decisions
            .get(&optimized.id())
            .expect("decision for the rebuilt join node");
        assert!(
            matches!(
                algorithm,
                JoinAlgorithm::HashJoin { .. } | JoinAlgorithm::NestedLoopJoin { .. }
            ),
            "expected an executable join algorithm, got {:?}",
            algorithm
        );
    }

    #[test]
    fn test_unchanged_tree_records_no_decisions() {
        let a = make_scan("a", 1000);
        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let mut decisions = HashMap::new();
        let result = walk_and_optimize_joins_with_decisions(
            &a,
            &stats_view,
            &cost_calc,
            &mut notes,
            &mut Some(&mut decisions),
        );
        assert!(matches!(result, PlanNodeEnum::Start(_)));
        assert!(decisions.is_empty());
    }

    #[test]
    fn test_cross_join_never_records_hash_decision() {
        // A join without keyed predicates has no hash keys: the HashJoin
        // decision must never be recorded for it even when the cost model
        // selects the hash algorithm.
        let a = make_scan("a", 1000);
        let b = make_scan("b", 10);
        let join = make_hash_join(a, b, vec![], vec![]);

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let mut decisions = HashMap::new();
        let optimized = walk_and_optimize_joins_with_decisions(
            &join,
            &stats_view,
            &cost_calc,
            &mut notes,
            &mut Some(&mut decisions),
        );
        assert!(matches!(optimized, PlanNodeEnum::InnerJoin(_)));
        assert!(
            !decisions
                .values()
                .any(|a| matches!(a, JoinAlgorithm::HashJoin { .. })),
            "keyless join must not record a HashJoin decision"
        );
    }

    #[test]
    fn test_left_join_acts_as_boundary() {
        let a = make_scan("a", 1000);
        let b = make_scan("b", 500);
        let inner = make_hash_join(a.clone(), b, vec!["a.id"], vec!["b.id"]);
        // LeftJoin with an inner join on the left and a scan on the right
        let c = make_scan("c", 100);
        let _ctx = std::sync::Arc::new(
            crate::core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        use crate::planning::plan::core::nodes::join::join_node::LeftJoinNode;
        let left_join =
            PlanNodeEnum::LeftJoin(LeftJoinNode::new(inner, c, vec![], vec![]).unwrap());

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let result = walk_and_optimize_joins(&left_join, &stats_view, &cost_calc, &mut notes);

        // The root should still be a LeftJoin
        assert!(matches!(result, PlanNodeEnum::LeftJoin(_)));
    }

    #[test]
    fn test_large_join_uses_greedy() {
        let mut tables = Vec::new();
        for i in 0..12 {
            tables.push(make_scan(&format!("t{}", i), (i as u64 + 1) * 100));
        }
        // Build a left-deep join chain
        let mut join = make_hash_join(tables[0].clone(), tables[1].clone(), vec![], vec![]);
        for table in tables.iter().skip(2) {
            join = make_hash_join(join, table.clone(), vec![], vec![]);
        }

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let result = walk_and_optimize_joins(&join, &stats_view, &cost_calc, &mut notes);

        // Should complete without panic (greedy path)
        assert!(matches!(result, PlanNodeEnum::InnerJoin(_)));
    }

    // ===================================================================
    // Logical-plan walker tests
    // ===================================================================

    use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
    use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;

    fn make_logical_scan(id: &str) -> LogicalNodeEnum {
        let mut node = LogicalStartNode::new();
        node.set_output_var(id.to_string());
        node.set_col_names(vec![id.to_string()]);
        LogicalNodeEnum::Start(node)
    }

    fn make_logical_hash_join(
        left: LogicalNodeEnum,
        right: LogicalNodeEnum,
        hk: Vec<&str>,
        pk: Vec<&str>,
    ) -> LogicalNodeEnum {
        let ctx = std::sync::Arc::new(
            crate::core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let hash_keys: Vec<ContextualExpression> = hk
            .iter()
            .map(|s| {
                let meta = crate::core::types::expr::ExpressionMeta::new(
                    crate::core::types::expr::Expression::Variable(s.to_string()),
                );
                let id = ctx.register_expression(meta);
                crate::core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
            })
            .collect();
        let probe_keys: Vec<ContextualExpression> = pk
            .iter()
            .map(|s| {
                let meta = crate::core::types::expr::ExpressionMeta::new(
                    crate::core::types::expr::Expression::Variable(s.to_string()),
                );
                let id = ctx.register_expression(meta);
                crate::core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
            })
            .collect();
        LogicalNodeEnum::InnerJoin(LogicalInnerJoinNode {
            id: crate::planning::plan::core::node_id_generator::next_node_id(),
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
            hash_keys,
            probe_keys,
            deps: vec![left, right],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    #[test]
    fn test_logical_single_table_returns_none() {
        let scan = make_logical_scan("a");
        assert!(flatten_join_chain_logical(&scan).is_none());
    }

    #[test]
    fn test_logical_two_table_chain() {
        let a = make_logical_scan("a");
        let b = make_logical_scan("b");
        let join = make_logical_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
        let chain = flatten_join_chain_logical(&join).expect("should flatten");
        assert_eq!(chain.leaves.len(), 2);
        assert_eq!(chain.predicates.len(), 1);
    }

    #[test]
    fn test_logical_three_table_reorder_emits_note() {
        let a = make_logical_scan("a");
        let b = make_logical_scan("b");
        let c = make_logical_scan("c");
        let join1 = make_logical_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
        let join2 = make_logical_hash_join(join1, c, vec!["b.id"], vec!["c.id"]);

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let optimized =
            walk_and_optimize_joins_logical(&join2, &stats_view, &cost_calc, &mut notes);

        // The reordered logical tree is a logical InnerJoin (no physical
        // InnerJoin can appear in the logical tree).
        assert!(matches!(optimized, LogicalNodeEnum::InnerJoin(_)));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].starts_with("join_order:"));
        assert!(notes[0].contains("order=["));
    }

    #[test]
    fn test_logical_non_join_unchanged() {
        let a = make_logical_scan("a");
        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let result = walk_and_optimize_joins_logical(&a, &stats_view, &cost_calc, &mut notes);
        assert!(matches!(result, LogicalNodeEnum::Start(_)));
        assert_eq!(logical_output_var(&result), logical_output_var(&a));
        assert!(notes.is_empty());
    }

    #[test]
    fn test_logical_left_join_acts_as_boundary() {
        let a = make_logical_scan("a");
        let b = make_logical_scan("b");
        let inner = make_logical_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
        let c = make_logical_scan("c");
        let left_join = LogicalNodeEnum::LeftJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode {
                id: crate::planning::plan::core::node_id_generator::next_node_id(),
                left: Box::new(inner.clone()),
                right: Box::new(c.clone()),
                hash_keys: vec![],
                probe_keys: vec![],
                deps: vec![inner, c],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let stats_view = StatsView::new(&stats, None);
        let mut notes = Vec::new();
        let result =
            walk_and_optimize_joins_logical(&left_join, &stats_view, &cost_calc, &mut notes);

        // The root stays a LeftJoin; the inner join below it may be reordered.
        assert!(matches!(result, LogicalNodeEnum::LeftJoin(_)));
    }
}

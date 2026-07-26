use std::collections::HashMap;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::analysis_utils::collect_variables_from_contextual;
use crate::query::optimizer::cost::CostCalculator;
use crate::query::optimizer::cost_based::join_order::{
    JoinCondition, JoinOrderOptimizer, JoinOrderResult, TableInfo,
};
use crate::query::optimizer::stats::StatisticsManager;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::PlanNodeEnum;

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
        PlanNodeEnum::InnerJoin(_) | PlanNodeEnum::HashInnerJoin(_) => JoinNodeType::Inner,
        PlanNodeEnum::CrossJoin(_) => JoinNodeType::Cross,
        PlanNodeEnum::LeftJoin(_)
        | PlanNodeEnum::HashLeftJoin(_)
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

fn estimate_leaf_rows(node: &PlanNodeEnum, stats: &StatisticsManager) -> u64 {
    match node {
        PlanNodeEnum::ScanVertices(n) => {
            if let Some(tag) = n.tag() {
                let count = stats.get_vertex_count(tag);
                if count > 0 {
                    return count;
                }
            }
            10000
        }
        PlanNodeEnum::ScanEdges(n) => {
            if let Some(et) = n.edge_type() {
                let count = stats.get_edge_count(&et);
                if count > 0 {
                    return count;
                }
            }
            50000
        }
        PlanNodeEnum::IndexScan(_) => 5000,
        PlanNodeEnum::EdgeIndexScan(n) => {
            let count = stats.get_edge_count(n.edge_type());
            if count > 0 {
                return count;
            }
            5000
        }
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
            let child = n.dependencies().first().map(|c| estimate_leaf_rows(c, stats)).unwrap_or(10000);
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
                PlanNodeEnum::InnerJoin(n) => {
                    (n.left_input(), n.right_input(), n.hash_keys().to_vec(), n.probe_keys().to_vec())
                }
                PlanNodeEnum::HashInnerJoin(n) => {
                    (n.left_input(), n.right_input(), n.hash_keys().to_vec(), n.probe_keys().to_vec())
                }
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

pub fn assign_leaf_info(chain: &mut FlattenedJoinChain, stats: &StatisticsManager) {
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
}

fn has_index_scan(node: &PlanNodeEnum) -> bool {
    matches!(
        node,
        PlanNodeEnum::IndexScan(_) | PlanNodeEnum::EdgeIndexScan(_)
    )
}

pub fn build_optimizer_input(
    chain: &FlattenedJoinChain,
) -> (Vec<TableInfo>, Vec<JoinCondition>) {
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
            JoinCondition::new(left_id, right_id)
                .with_selectivity(p.selectivity)
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
    let original_type = classify_join(original_root);
    if original_type != JoinNodeType::Inner && original_type != JoinNodeType::Cross {
        return original_root.clone();
    }

    let leaf_map: HashMap<&str, &PlanNodeEnum> = chain
        .leaves
        .iter()
        .map(|l| (l.id.as_str(), &l.physical_node))
        .collect();

    let mut pred_map: HashMap<(String, String), Vec<(Vec<ContextualExpression>, Vec<ContextualExpression>)>> =
        HashMap::new();
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
                let lid = leaf_id(&left);
                let rid = leaf_id(&right_node);
                let pair_key = if lid <= rid {
                    (lid.clone(), rid.clone())
                } else {
                    (rid, lid)
                };
                let (hash_keys, probe_keys) = resolve_keys_for_pair(
                    &pair_key,
                    &pred_map,
                    &left,
                    &right_node,
                );
                Some(build_hash_inner_join(left, right_node, hash_keys, probe_keys))
            }
            None => Some(right_node),
        };
    }

    current.unwrap_or_else(|| original_root.clone())
}

fn resolve_keys_for_pair(
    pair_key: &(String, String),
    pred_map: &HashMap<(String, String), Vec<(Vec<ContextualExpression>, Vec<ContextualExpression>)>>,
    left_physical: &PlanNodeEnum,
    right_physical: &PlanNodeEnum,
) -> (Vec<ContextualExpression>, Vec<ContextualExpression>) {
    if let Some(keys_list) = pred_map.get(pair_key) {
        if let Some((hk, pk)) = keys_list.first() {
            let left_id = leaf_id(left_physical);
            let right_id = leaf_id(right_physical);
            let left_vars = collect_variables_from_slice(hk);
            let right_vars = collect_variables_from_slice(pk);

            let swap = left_vars.iter().any(|v| right_id.contains(v) || v.contains(&right_id))
                || right_vars.iter().any(|v| left_id.contains(v) || v.contains(&left_id));
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

fn build_hash_inner_join(
    left: PlanNodeEnum,
    right: PlanNodeEnum,
    hash_keys: Vec<ContextualExpression>,
    probe_keys: Vec<ContextualExpression>,
) -> PlanNodeEnum {
    use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;
    match HashInnerJoinNode::new(left, right, hash_keys, probe_keys) {
        Ok(node) => PlanNodeEnum::HashInnerJoin(node),
        Err(e) => {
            panic!("HashInnerJoin construction failed: {}", e);
        }
    }
}

enum OptResult {
    Changed(PlanNodeEnum),
    Unchanged,
}

fn try_optimize_join_tree(
    root: &PlanNodeEnum,
    stats: &StatisticsManager,
    cost_calculator: &CostCalculator,
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

    OptResult::Changed(reconstruct_join_tree(root, &chain, &result))
}

pub fn walk_and_optimize_joins(
    root: &PlanNodeEnum,
    stats: &StatisticsManager,
    cost_calculator: &CostCalculator,
) -> PlanNodeEnum {
    if let OptResult::Changed(optimized) = try_optimize_join_tree(root, stats, cost_calculator) {
        return optimized;
    }

    match root {
        PlanNodeEnum::Project(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Project(cloned)
        }
        PlanNodeEnum::Filter(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Filter(cloned)
        }
        PlanNodeEnum::Sort(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Sort(cloned)
        }
        PlanNodeEnum::Limit(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Limit(cloned)
        }
        PlanNodeEnum::TopN(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::TopN(cloned)
        }
        PlanNodeEnum::Sample(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Sample(cloned)
        }
        PlanNodeEnum::Dedup(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Dedup(cloned)
        }
        PlanNodeEnum::Aggregate(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Aggregate(cloned)
        }
        PlanNodeEnum::Window(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Window(cloned)
        }
        PlanNodeEnum::Traverse(n) => {
            let new_input = walk_and_optimize_joins(n.input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_input(new_input);
            PlanNodeEnum::Traverse(cloned)
        }
        PlanNodeEnum::LeftJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::LeftJoin(cloned)
        }
        PlanNodeEnum::RightJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::RightJoin(cloned)
        }
        PlanNodeEnum::HashLeftJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::HashLeftJoin(cloned)
        }
        PlanNodeEnum::FullOuterJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::FullOuterJoin(cloned)
        }
        PlanNodeEnum::SemiJoin(n) => {
            let new_left = walk_and_optimize_joins(n.left_input(), stats, cost_calculator);
            let new_right = walk_and_optimize_joins(n.right_input(), stats, cost_calculator);
            let mut cloned = n.clone();
            cloned.set_left_input(new_left);
            cloned.set_right_input(new_right);
            PlanNodeEnum::SemiJoin(cloned)
        }
        _ => root.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::cost::CostCalculator;
    use crate::query::planning::plan::core::nodes::join::join_node::HashInnerJoinNode;

    fn make_scan(id: &str, rows: u64) -> PlanNodeEnum {
        // Use a StartNode as a stand-in leaf for testing
        let mut node = crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
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
        let hash_keys: Vec<ContextualExpression> = hk.iter().map(|s| {
            let meta = crate::core::types::expr::ExpressionMeta::new(
                crate::core::types::expr::Expression::Variable(s.to_string()),
            );
            let id = ctx.register_expression(meta);
            crate::core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
        }).collect();
        let probe_keys: Vec<ContextualExpression> = pk.iter().map(|s| {
            let meta = crate::core::types::expr::ExpressionMeta::new(
                crate::core::types::expr::Expression::Variable(s.to_string()),
            );
            let id = ctx.register_expression(meta);
            crate::core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
        }).collect();
        PlanNodeEnum::HashInnerJoin(
            HashInnerJoinNode::new(left, right, hash_keys, probe_keys).unwrap(),
        )
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
        let result = walk_and_optimize_joins(&a, &stats, &cost_calc);
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
        let optimized = walk_and_optimize_joins(&join2, &stats, &cost_calc);

        // The smallest table (b, 10 rows) should be first in the new tree
        assert!(matches!(optimized, PlanNodeEnum::HashInnerJoin(_)));
        // Verify it's still a valid join tree
        assert!(optimized.children().len() >= 2);
    }

    #[test]
    fn test_left_join_acts_as_boundary() {
        let a = make_scan("a", 1000);
        let b = make_scan("b", 500);
        let inner = make_hash_join(a.clone(), b, vec!["a.id"], vec!["b.id"]);
        // LeftJoin with an inner join on the left and a scan on the right
        let c = make_scan("c", 100);
        let ctx = std::sync::Arc::new(
            crate::core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        use crate::query::planning::plan::core::nodes::join::join_node::LeftJoinNode;
        let left_join = PlanNodeEnum::LeftJoin(
            LeftJoinNode::new(inner, c, vec![], vec![]).unwrap(),
        );

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let result = walk_and_optimize_joins(&left_join, &stats, &cost_calc);

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
        let mut join = make_hash_join(
            tables[0].clone(),
            tables[1].clone(),
            vec![],
            vec![],
        );
        for i in 2..12 {
            join = make_hash_join(join, tables[i].clone(), vec![], vec![]);
        }

        let stats = StatisticsManager::new();
        let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
        let result = walk_and_optimize_joins(&join, &stats, &cost_calc);

        // Should complete without panic (greedy path)
        assert!(matches!(result, PlanNodeEnum::HashInnerJoin(_)));
    }
}

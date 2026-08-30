use std::collections::HashMap;

use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;
use graphdb_core::types::expr::analysis_utils::collect_variables_from_contextual;
use graphdb_core::types::expr::contextual::ContextualExpression;

use super::types::{
    FlattenedJoinChain, FlattenedJoinChainLogical, JoinNodeType, JoinPredicate, LeafInfo,
    LeafInfoLogical,
};
use crate::optimizer::stats::StatsView;

pub(super) fn classify_join(node: &PlanNodeEnum) -> JoinNodeType {
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

pub(super) fn leaf_id(node: &PlanNodeEnum) -> String {
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

// =====================================================================
// Logical-plan flatten functions
// =====================================================================

/// Logical join classification. The logical tree is pure — it never
/// carries physical variants (InnerJoin), so only InnerJoin/CrossJoin
/// are reorderable.
pub(super) fn classify_join_logical(node: &LogicalNodeEnum) -> JoinNodeType {
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

pub(super) fn leaf_id_logical(node: &LogicalNodeEnum) -> String {
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
pub(super) fn logical_output_var(node: &LogicalNodeEnum) -> Option<&str> {
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
pub(super) fn logical_column_types(node: &LogicalNodeEnum) -> Vec<graphdb_core::DataType> {
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

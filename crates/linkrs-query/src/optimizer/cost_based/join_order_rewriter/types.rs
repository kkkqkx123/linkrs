use std::collections::HashMap;

use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;
use linkrs_core::types::expr::contextual::ContextualExpression;

pub(crate) type PredMap =
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
pub(crate) enum JoinNodeType {
    Inner,
    Cross,
    NonReorderable,
    NotJoin,
}

pub(crate) enum OptResult {
    Changed(Box<PlanNodeEnum>, String),
    Unchanged,
}

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

pub(crate) enum OptResultLogical {
    Changed(Box<LogicalNodeEnum>, String),
    Unchanged,
}

/// Shared join-order decision core operating on plain values.
///
/// Both the physical and logical adapters extract leaf ids, row counts and
/// index flags from their own node types and call through here, so costing,
/// gating and note text cannot drift between the two trees.
pub(crate) fn build_optimizer_input_from_parts(
    ids: &[String],
    rows: &[u64],
    indexed: &[bool],
    predicates: &[JoinPredicate],
) -> (
    Vec<crate::optimizer::cost_based::join_order::TableInfo>,
    Vec<crate::optimizer::cost_based::join_order::JoinCondition>,
) {
    use crate::optimizer::cost_based::join_order::{JoinCondition, TableInfo};

    let tables: Vec<TableInfo> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            TableInfo::new(id.clone(), rows.get(i).copied().unwrap_or(0))
                .with_index(indexed.get(i).copied().unwrap_or(false))
                .with_bit_id(i as u32)
        })
        .collect();
    let conditions: Vec<JoinCondition> = predicates
        .iter()
        .map(|p| {
            JoinCondition::new(p.left_table.clone(), p.right_table.clone())
                .with_selectivity(p.selectivity)
        })
        .collect();
    (tables, conditions)
}

/// Build the pair-keyed predicate map shared by both reconstruct paths.
pub(crate) fn build_pred_map(predicates: &[JoinPredicate]) -> PredMap {
    let mut pred_map: PredMap = std::collections::HashMap::new();
    for p in predicates {
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
    pred_map
}

/// Deduplicate leaf ids in place (`base`, `base_1`, ...).
pub(crate) fn deduplicate_leaf_ids(ids: &mut [String]) {
    use std::collections::HashMap;
    let mut seen: HashMap<String, usize> = HashMap::new();
    for id in ids.iter_mut() {
        let key = id.clone();
        let count = seen.entry(key.clone()).or_insert(0);
        if *count > 0 {
            *id = format!("{}_{}", key, count);
        }
        *count += 1;
    }
}

/// Single safety gate for join algorithm decisions.
///
/// `HashJoin` needs valid equi keys, `NestedLoopJoin` needs trusted row
/// estimates, `IndexJoin` has no executor yet.
pub(crate) fn normalize_join_algorithm_decision(
    algorithm: Option<&crate::optimizer::JoinAlgorithm>,
    has_hash_keys: bool,
    left_rows: u64,
    right_rows: u64,
) -> Option<crate::optimizer::JoinAlgorithm> {
    let algorithm = algorithm?;
    match algorithm {
        crate::optimizer::JoinAlgorithm::NestedLoopJoin { .. } => {
            if left_rows > 0 && right_rows > 0 {
                Some(algorithm.clone())
            } else {
                None
            }
        }
        crate::optimizer::JoinAlgorithm::HashJoin { .. } => {
            if has_hash_keys {
                Some(algorithm.clone())
            } else {
                None
            }
        }
        crate::optimizer::JoinAlgorithm::IndexJoin { .. } => None,
    }
}

/// Whether hash/probe keys must swap to match left/right operand identity.
pub(crate) fn should_swap_join_keys(
    left_id: &str,
    right_id: &str,
    hash_vars: &[String],
    probe_vars: &[String],
) -> bool {
    hash_vars
        .iter()
        .any(|v| right_id.contains(v) || v.contains(right_id))
        || probe_vars
            .iter()
            .any(|v| left_id.contains(v) || v.contains(left_id))
}

/// Output estimate mirroring the join-order cost model default selectivity.
pub(crate) fn accumulated_join_rows(accumulated: u64, right_rows: u64, selectivity: f64) -> u64 {
    ((accumulated as f64 * right_rows as f64 * selectivity) as u64).max(1)
}

/// Shared CBO note text for join-order rewrites.
pub(crate) fn format_join_order_note(
    leaf_count: usize,
    method: &str,
    order: &[String],
    current_cost: f64,
    total_cost: f64,
) -> String {
    format!(
        "join_order: {} tables, method={}, order=[{}], cost {:.1}->{:.1} (reviewer)",
        leaf_count,
        method,
        order.join(", "),
        current_cost,
        total_cost,
    )
}

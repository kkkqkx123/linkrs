use std::collections::HashMap;

use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::PlanNodeEnum;
use graphdb_core::types::expr::contextual::ContextualExpression;

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

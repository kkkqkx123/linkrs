use crate::binder::query_graph::QueryGraph;
use crate::parser::ast::{LimitClause, SkipClause, Steps};

use linkrs_core::types::EdgeDirection;

use super::clause::{
    BoundOrderByItem, BoundProjectionItem, BoundReturnClause, BoundWhereClause, BoundYieldClause,
};
use super::expr::{BoundAggregateCall, BoundExpression};
use super::stmt::BoundStatement;

#[derive(Debug, Clone)]
pub struct BoundMatchStatement {
    pub query_graph: QueryGraph,
    pub join_hint: Option<BoundJoinHint>,
    pub where_clause: Option<BoundWhereClause>,
    pub return_clause: Option<BoundReturnClause>,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub optional: bool,
    pub delete_clause: Option<BoundMatchDeleteClause>,
}

/// Bound `USING JOIN` hint: pattern variables pinned to a join shape.
/// Resolution against the planning query graph happens in the planner;
/// the binder only checks that every named variable is in scope.
#[derive(Debug, Clone)]
pub enum BoundJoinHint {
    Binary { left: String, right: String },
    Multiway { probe: String, builds: Vec<String> },
}

impl BoundJoinHint {
    /// Variables referenced by the hint, in order.
    pub fn variables(&self) -> Vec<&str> {
        match self {
            BoundJoinHint::Binary { left, right } => vec![left.as_str(), right.as_str()],
            BoundJoinHint::Multiway { probe, builds } => {
                let mut vars = Vec::with_capacity(builds.len() + 1);
                vars.push(probe.as_str());
                vars.extend(builds.iter().map(String::as_str));
                vars
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct BoundGoStatement {
    pub steps: Steps,
    pub from: Vec<BoundExpression>,
    pub over: Option<Vec<String>>,
    pub direction: EdgeDirection,
    pub where_clause: Option<BoundWhereClause>,
    pub yield_clause: Option<BoundYieldClause>,
}

#[derive(Debug, Clone)]
pub struct BoundLookupStatement {
    pub target: BoundLookupTarget,
    pub where_clause: Option<BoundWhereClause>,
    pub yield_clause: Option<BoundYieldClause>,
}

#[derive(Debug, Clone)]
pub enum BoundLookupTarget {
    Tag(String),
    Edge(String),
}

#[derive(Debug, Clone)]
pub struct BoundFetchVerticesStatement {
    pub tag_name: Option<String>,
    pub ids: Vec<BoundExpression>,
    pub properties: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct BoundFetchEdgesStatement {
    pub src: BoundExpression,
    pub dst: BoundExpression,
    pub edge_type: String,
    pub rank: Option<BoundExpression>,
    pub properties: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct BoundFindPathStatement {
    pub from: Vec<BoundExpression>,
    pub to: BoundExpression,
    pub over: Option<(Vec<String>, EdgeDirection)>,
    pub where_clause: Option<BoundWhereClause>,
    pub shortest: bool,
    pub max_steps: Option<usize>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub yield_clause: Option<BoundYieldClause>,
    /// WEIGHT clause: property name used as the edge weight (validated at
    /// bind time to only appear on FIND SHORTEST PATH).
    pub weight_expression: Option<String>,
    /// `WITH LOOP`: self-loop edges may be taken as terminal extensions.
    pub with_loop: bool,
    /// `WITH TRAIL`: path edges may not repeat; vertices may.
    pub with_trail: bool,
    /// `WITH WALK`: path vertices may repeat.
    pub with_walk: bool,
}

#[derive(Debug, Clone)]
pub struct BoundSubgraphStatement {
    pub steps: Steps,
    pub from: Vec<BoundExpression>,
    pub over: Option<(Vec<String>, EdgeDirection)>,
    pub where_clause: Option<BoundWhereClause>,
    pub yield_clause: Option<BoundYieldClause>,
}

#[derive(Debug, Clone)]
pub struct BoundReturnStatement {
    pub items: Vec<BoundProjectionItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub skip: Option<SkipClause>,
    pub limit: Option<LimitClause>,
}

#[derive(Debug, Clone)]
pub struct BoundWithStatement {
    pub items: Vec<BoundProjectionItem>,
    pub condition: Option<BoundExpression>,
    pub ctes: Vec<BoundCteDef>,
}

/// A bound common table expression.
///
/// Under `WITH RECURSIVE`, `step` is the `UNION ALL` right branch bound with
/// the CTE name visible as a scannable working table; otherwise `step` is
/// `None` and the CTE is a plain inline view.
#[derive(Debug, Clone)]
pub struct BoundCteDef {
    pub name: String,
    pub anchor: Box<BoundStatement>,
    pub step: Option<Box<BoundStatement>>,
}

#[derive(Debug, Clone)]
pub struct BoundUnwindStatement {
    pub expression: BoundExpression,
    pub alias: String,
}

#[derive(Debug, Clone)]
pub struct BoundPipeStatement {
    pub statements: Vec<BoundStatement>,
}

#[derive(Debug, Clone)]
pub struct BoundGroupByStatement {
    pub keys: Vec<BoundExpression>,
    pub aggregates: Vec<BoundAggregateCall>,
}

#[derive(Debug, Clone)]
pub struct BoundSetOperationStatement {
    pub left: Box<BoundStatement>,
    pub right: Box<BoundStatement>,
    pub operation: SetOperationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperationKind {
    Union,
    UnionAll,
    Intersect,
    Minus,
}

#[derive(Debug, Clone)]
pub struct BoundFilter {
    pub condition: BoundExpression,
}

#[derive(Debug, Clone)]
pub struct BoundYield {
    pub items: Vec<BoundProjectionItem>,
    pub where_clause: Option<BoundExpression>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub skip: Option<SkipClause>,
    pub limit: Option<LimitClause>,
}

#[derive(Debug, Clone)]
pub struct BoundCollect {
    pub items: Vec<BoundProjectionItem>,
}

#[derive(Debug, Clone)]
pub struct BoundAssignVariable {
    pub name: String,
    pub expression: BoundExpression,
}

#[derive(Debug, Clone)]
pub struct BoundMatchDeleteClause {
    pub target: BoundMatchDeleteTarget,
    pub with_edge: bool,
}

#[derive(Debug, Clone)]
pub enum BoundMatchDeleteTarget {
    Vertices(Vec<BoundExpression>),
    Edges(Vec<BoundExpression>),
    EdgeRefs(Vec<(BoundExpression, BoundExpression, Option<BoundExpression>)>),
}

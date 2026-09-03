use crate::parser::ast::pattern::Pattern;
use crate::parser::ast::types::{LimitClause, SampleClause, SkipClause};
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::{EdgeDirection, OrderDirection, Span};

use super::Stmt;

#[derive(Debug, Clone)]
pub struct QueryStmt {
    pub span: Span,
    pub statements: Vec<Stmt>,
}

impl QueryStmt {
    pub fn new(statements: Vec<Stmt>, span: Span) -> Self {
        Self { span, statements }
    }
}

impl PartialEq for QueryStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.statements.len() == other.statements.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchStmt {
    pub span: Span,
    pub patterns: Vec<Pattern>,
    pub join_hint: Option<super::super::hint::JoinHintAst>,
    pub where_clause: Option<ContextualExpression>,
    pub return_clause: Option<ReturnClause>,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub optional: bool,
    pub delete_clause: Option<MatchDeleteClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchDeleteClause {
    pub span: Span,
    pub target: MatchDeleteTarget,
    pub with_edge: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchDeleteTarget {
    Vertices(Vec<ContextualExpression>),
    Edges(Vec<ContextualExpression>),
    EdgeRefs(
        Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
        )>,
    ),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnClause {
    pub span: Span,
    pub items: Vec<ReturnItem>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub sample: Option<SampleClause>,
    pub having_clause: Option<ContextualExpression>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReturnItem {
    Expression {
        expression: ContextualExpression,
        alias: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderByClause {
    pub span: Span,
    pub items: Vec<OrderByItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderByItem {
    pub expression: ContextualExpression,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoStmt {
    pub span: Span,
    pub steps: Steps,
    pub from: FromClause,
    pub over: Option<OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub yield_clause: Option<YieldClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Steps {
    Fixed(usize),
    Range { min: usize, max: usize },
    Variable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepClause {
    pub span: Span,
    pub steps: Steps,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhereClause {
    pub span: Span,
    pub condition: ContextualExpression,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FromClause {
    pub span: Span,
    pub vertices: Vec<ContextualExpression>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OverClause {
    pub span: Span,
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YieldClause {
    pub span: Span,
    pub items: Vec<YieldItem>,
    pub where_clause: Option<ContextualExpression>,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub sample: Option<SampleClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YieldItem {
    pub expression: ContextualExpression,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchStmt {
    pub span: Span,
    pub target: FetchTarget,
    pub yield_clause: Option<YieldClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchTarget {
    Vertices {
        tag_name: Option<String>,
        ids: Vec<ContextualExpression>,
        properties: Option<Vec<String>>,
    },
    Edges {
        src: ContextualExpression,
        dst: ContextualExpression,
        edge_type: String,
        rank: Option<ContextualExpression>,
        properties: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LookupStmt {
    pub span: Span,
    pub target: LookupTarget,
    pub where_clause: Option<ContextualExpression>,
    pub yield_clause: Option<YieldClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LookupTarget {
    Tag(String),
    Edge(String),
    Unspecified(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubgraphStmt {
    pub span: Span,
    pub steps: Steps,
    pub from: FromClause,
    pub over: Option<OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub yield_clause: Option<YieldClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FindPathStmt {
    pub span: Span,
    pub from: FromClause,
    pub to: ContextualExpression,
    pub over: Option<OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub shortest: bool,
    pub max_steps: Option<usize>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub yield_clause: Option<YieldClause>,
    pub weight_expression: Option<String>,
    pub heuristic_expression: Option<String>,
    pub with_loop: bool,
    pub with_cycle: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnStmt {
    pub span: Span,
    pub items: Vec<ReturnItem>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub skip: Option<SkipClause>,
    pub limit: Option<LimitClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YieldStmt {
    pub span: Span,
    pub items: Vec<YieldItem>,
    pub where_clause: Option<ContextualExpression>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub skip: Option<SkipClause>,
    pub limit: Option<LimitClause>,
}

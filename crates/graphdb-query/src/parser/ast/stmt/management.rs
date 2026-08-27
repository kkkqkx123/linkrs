use crate::core::types::expr::analysis_utils::collect_variables_from_contextual;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::Span;

use super::PatternUtils;

use super::{CreateTarget, DeleteTarget, FetchTarget, UpdateTarget};
use super::{OrderByClause, ReturnClause, ReturnItem, Stmt, YieldClause};
use crate::parser::ast::types::{LimitClause, SkipClause};

#[derive(Debug, Clone, PartialEq)]
pub struct UseStmt {
    pub span: Span,
    pub space: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowStmt {
    pub span: Span,
    pub target: ShowTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShowTarget {
    Spaces,
    Tags,
    Edges,
    Tag(String),
    Edge(String),
    Indexes,
    Index(String),
    Users,
    Roles,
    Stats,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ExplainFormat {
    #[default]
    Table,
    Dot,
}

#[derive(Debug, Clone)]
pub struct ExplainStmt {
    pub span: Span,
    pub statement: Box<Stmt>,
    pub format: ExplainFormat,
    /// Whether the statement is executed and actual operator statistics
    /// (rows / time) are overlaid on the plan output.
    pub analyze: bool,
}

impl PartialEq for ExplainStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.format == other.format && self.analyze == other.analyze
    }
}

#[derive(Debug, Clone)]
pub struct ProfileStmt {
    pub span: Span,
    pub statement: Box<Stmt>,
    pub format: ExplainFormat,
}

/// ANALYZE statement: collect statistics for a space.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyzeStmt {
    pub span: Span,
    /// None = current/default space.
    pub space: Option<String>,
}

impl PartialEq for ProfileStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.format == other.format
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroupingType {
    Standard,
    Rollup(Vec<ContextualExpression>),
    Cube(Vec<ContextualExpression>),
    GroupingSets(Vec<Vec<ContextualExpression>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupByStmt {
    pub span: Span,
    pub group_items: Vec<ContextualExpression>,
    pub grouping_type: GroupingType,
    pub yield_clause: YieldClause,
    pub having_clause: Option<ContextualExpression>,
}

#[derive(Debug, Clone)]
pub struct PipeStmt {
    pub span: Span,
    pub left: Box<Stmt>,
    pub right: Box<Stmt>,
}

impl PartialEq for PipeStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnwindStmt {
    pub span: Span,
    pub expression: ContextualExpression,
    pub variable: String,
    pub return_clause: Option<ReturnClause>,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WithStmt {
    pub span: Span,
    pub items: Vec<ReturnItem>,
    pub where_clause: Option<ContextualExpression>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub skip: Option<SkipClause>,
    pub limit: Option<LimitClause>,
    pub recursive: bool,
}

/// A standalone WHERE stage used as a pipe suffix (e.g. `GO ... | WHERE age > 25`).
#[derive(Debug, Clone, PartialEq)]
pub struct FilterStmt {
    pub span: Span,
    pub expression: ContextualExpression,
}

/// A standalone COLLECT stage used as a pipe suffix (e.g. `| COLLECT LIST(name) AS names`).
#[derive(Debug, Clone, PartialEq)]
pub struct CollectStmt {
    pub span: Span,
    pub items: Vec<super::YieldItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowSessionsStmt {
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowQueriesStmt {
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KillQueryStmt {
    pub span: Span,
    pub session_id: i64,
    pub plan_id: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowConfigsStmt {
    pub span: Span,
    pub module: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateConfigsStmt {
    pub span: Span,
    pub module: Option<String>,
    pub config_name: String,
    pub config_value: ContextualExpression,
}

#[derive(Debug, Clone)]
pub struct AssignmentStmt {
    pub span: Span,
    pub variable: String,
    pub statement: Box<Stmt>,
}

impl PartialEq for AssignmentStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.variable == other.variable
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetOperationType {
    Union,
    UnionAll,
    Intersect,
    Minus,
}

#[derive(Debug, Clone)]
pub struct SetOperationStmt {
    pub span: Span,
    pub op_type: SetOperationType,
    pub left: Box<Stmt>,
    pub right: Box<Stmt>,
}

impl PartialEq for SetOperationStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.op_type == other.op_type
    }
}

pub struct StmtUtils;

impl StmtUtils {
    pub fn find_variables(stmt: &Stmt) -> Vec<String> {
        let mut variables = Vec::new();
        Self::find_variables_recursive(stmt, &mut variables);
        variables
    }

    fn find_variables_recursive(stmt: &Stmt, variables: &mut Vec<String>) {
        match stmt {
            Stmt::Match(s) => {
                for pattern in &s.patterns {
                    variables.extend(PatternUtils::find_variables(pattern));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Create(s) => match &s.target {
                CreateTarget::Node {
                    properties: Some(props),
                    ..
                } => {
                    variables.extend(collect_variables_from_contextual(props));
                }
                CreateTarget::Edge {
                    src,
                    dst,
                    properties: Some(props),
                    ..
                } => {
                    variables.extend(collect_variables_from_contextual(src));
                    variables.extend(collect_variables_from_contextual(dst));
                    variables.extend(collect_variables_from_contextual(props));
                }
                _ => {}
            },
            Stmt::Delete(s) => {
                match &s.target {
                    DeleteTarget::Vertices(vertices) => {
                        for vertex in vertices {
                            variables.extend(collect_variables_from_contextual(vertex));
                        }
                    }
                    DeleteTarget::Edges { edges, .. } => {
                        for (src, dst, rank) in edges {
                            variables.extend(collect_variables_from_contextual(src));
                            variables.extend(collect_variables_from_contextual(dst));
                            if let Some(ref rank) = rank {
                                variables.extend(collect_variables_from_contextual(rank));
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Update(s) => {
                match &s.target {
                    UpdateTarget::Vertex(vertex) => {
                        variables.extend(collect_variables_from_contextual(vertex));
                    }
                    UpdateTarget::Edge { src, dst, rank, .. } => {
                        variables.extend(collect_variables_from_contextual(src));
                        variables.extend(collect_variables_from_contextual(dst));
                        if let Some(ref rank) = rank {
                            variables.extend(collect_variables_from_contextual(rank));
                        }
                    }
                    _ => {}
                }
                for assignment in &s.set_clause.assignments {
                    variables.extend(collect_variables_from_contextual(&assignment.value));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Go(s) => {
                for vertex in &s.from.vertices {
                    variables.extend(collect_variables_from_contextual(vertex));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Fetch(s) => match &s.target {
                FetchTarget::Vertices { ids, .. } => {
                    for id in ids {
                        variables.extend(collect_variables_from_contextual(id));
                    }
                }
                FetchTarget::Edges { src, dst, rank, .. } => {
                    variables.extend(collect_variables_from_contextual(src));
                    variables.extend(collect_variables_from_contextual(dst));
                    if let Some(ref rank) = rank {
                        variables.extend(collect_variables_from_contextual(rank));
                    }
                }
            },
            Stmt::Lookup(s) => {
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Subgraph(s) => {
                for vertex in &s.from.vertices {
                    variables.extend(collect_variables_from_contextual(vertex));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::FindPath(s) => {
                for vertex in &s.from.vertices {
                    variables.extend(collect_variables_from_contextual(vertex));
                }
                variables.extend(collect_variables_from_contextual(&s.to));
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            _ => {}
        }
    }
}

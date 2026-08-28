use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::Span;
use crate::parser::ast::pattern::Pattern;

use super::YieldClause;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteStmt {
    pub span: Span,
    pub target: DeleteTarget,
    pub where_clause: Option<ContextualExpression>,
    pub with_edge: bool,
}

impl DeleteStmt {
    pub fn new(target: DeleteTarget, span: Span) -> Self {
        Self {
            span,
            target,
            where_clause: None,
            with_edge: false,
        }
    }

    pub fn with_edge(mut self, with_edge: bool) -> Self {
        self.with_edge = with_edge;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeleteTarget {
    Vertices(Vec<ContextualExpression>),
    Edges {
        edge_type: Option<String>,
        edges: Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
        )>,
    },
    Tags {
        tag_names: Vec<String>,
        vertex_ids: Vec<ContextualExpression>,
        is_all_tags: bool,
    },
    Index(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStmt {
    pub span: Span,
    pub target: UpdateTarget,
    pub set_clause: SetClause,
    pub where_clause: Option<ContextualExpression>,
    pub is_upsert: bool,
    pub yield_clause: Option<YieldClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateTarget {
    Vertex(ContextualExpression),
    Edge {
        src: ContextualExpression,
        dst: ContextualExpression,
        edge_type: Option<String>,
        rank: Option<ContextualExpression>,
    },
    Tag(String),
    TagOnVertex {
        vid: Box<ContextualExpression>,
        tag_name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetClause {
    pub span: Span,
    pub assignments: Vec<Assignment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub property: String,
    pub value: ContextualExpression,
    pub target: Option<ContextualExpression>,
    pub object: Option<ContextualExpression>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub span: Span,
    pub target: InsertTarget,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertTarget {
    Vertices {
        tags: Vec<TagInsertSpec>,
        values: Vec<VertexRow>,
    },
    Edge {
        edge_name: String,
        prop_names: Vec<String>,
        edges: Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
            Vec<ContextualExpression>,
        )>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagInsertSpec {
    pub tag_name: String,
    pub prop_names: Vec<String>,
    pub is_default_props: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VertexRow {
    pub vid: ContextualExpression,
    pub tag_values: Vec<Vec<ContextualExpression>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeStmt {
    pub span: Span,
    pub pattern: Pattern,
    pub on_create: Option<SetClause>,
    pub on_match: Option<SetClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetStmt {
    pub span: Span,
    pub assignments: Vec<Assignment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoveStmt {
    pub span: Span,
    pub items: Vec<ContextualExpression>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CopyStmt {
    pub span: Span,
    pub target: CopyTarget,
    /// Import (`FROM`) or export (`TO`) direction.
    pub direction: CopyDirection,
    pub file_path: String,
    pub header: bool,
    pub delimiter: char,
    pub batch_size: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyDirection {
    /// `COPY ... FROM 'file'`: bulk import.
    From,
    /// `COPY ... TO 'file'`: bulk export.
    To,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CopyTarget {
    Vertex(String),
    Edge(String),
}

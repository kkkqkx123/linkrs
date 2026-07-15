use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::{EdgeDirection, PropertyDef, Span};
use crate::query::parser::ast::pattern::Pattern;
use crate::query::parser::ast::types::DataType;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateStmt {
    pub span: Span,
    pub target: CreateTarget,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CreateTarget {
    Node {
        variable: Option<String>,
        labels: Vec<String>,
        properties: Option<ContextualExpression>,
    },
    Edge {
        variable: Option<String>,
        edge_type: String,
        src: ContextualExpression,
        dst: ContextualExpression,
        properties: Option<ContextualExpression>,
        direction: EdgeDirection,
    },
    Path { patterns: Vec<Pattern> },
    Tag {
        name: String,
        properties: Vec<PropertyDef>,
        ttl_duration: Option<i64>,
        ttl_col: Option<String>,
    },
    EdgeType {
        name: String,
        properties: Vec<PropertyDef>,
        ttl_duration: Option<i64>,
        ttl_col: Option<String>,
        src_tag: Option<String>,
        dst_tag: Option<String>,
    },
    Space {
        name: String,
        vid_type: String,
        comment: Option<String>,
    },
    Index {
        index_type: IndexType,
        name: String,
        on: String,
        properties: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexType {
    Tag,
    Edge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropStmt {
    pub span: Span,
    pub target: DropTarget,
    pub if_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DropTarget {
    Space(String),
    Tags(Vec<String>),
    Edges(Vec<String>),
    TagIndex {
        space_name: String,
        index_name: String,
    },
    EdgeIndex {
        space_name: String,
        index_name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DescStmt {
    pub span: Span,
    pub target: DescTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DescTarget {
    Space(String),
    Tag {
        space_name: String,
        tag_name: String,
    },
    Edge {
        space_name: String,
        edge_name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlterStmt {
    pub span: Span,
    pub target: AlterTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyChange {
    pub old_name: String,
    pub new_name: String,
    pub data_type: DataType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlterTarget {
    Tag {
        tag_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
        changes: Vec<PropertyChange>,
    },
    Edge {
        edge_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
        changes: Vec<PropertyChange>,
    },
    Space {
        space_name: String,
        comment: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClearSpaceStmt {
    pub span: Span,
    pub space_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowCreateStmt {
    pub span: Span,
    pub target: ShowCreateTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShowCreateTarget {
    Space(String),
    Tag(String),
    Edge(String),
    Index(String),
}

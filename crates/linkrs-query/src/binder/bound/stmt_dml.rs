use crate::parser::ast::{CopyDirection, CopyTarget, TagInsertSpec};

use linkrs_core::types::EdgeDirection;

use super::expr::BoundExpression;

#[derive(Debug, Clone)]
pub struct BoundInsert {
    pub target: BoundInsertTarget,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub enum BoundInsertTarget {
    Vertices {
        tag: TagInsertSpec,
        values: Vec<BoundVertexRow>,
    },
    Edge {
        edge_name: String,
        prop_names: Vec<String>,
        edges: Vec<(
            BoundExpression,
            BoundExpression,
            Option<BoundExpression>,
            Vec<BoundExpression>,
        )>,
    },
}

#[derive(Debug, Clone)]
pub struct BoundVertexRow {
    pub vid: BoundExpression,
    pub values: Vec<BoundExpression>,
}

#[derive(Debug, Clone)]
pub struct BoundUpdate {
    pub target: BoundUpdateTarget,
    pub assignments: Vec<BoundAssignment>,
    pub where_clause: Option<BoundExpression>,
    pub is_upsert: bool,
}

#[derive(Debug, Clone)]
pub enum BoundUpdateTarget {
    Vertex(BoundExpression),
    Edge(Box<BoundEdgeUpdateTarget>),
    Tag(String),
    TagOnVertex {
        vid: BoundExpression,
        tag_name: String,
    },
}

/// Edge update target payload. Boxed inside [`BoundUpdateTarget::Edge`] so the
/// several large `BoundExpression` fields do not inflate every enum variant.
#[derive(Debug, Clone)]
pub struct BoundEdgeUpdateTarget {
    pub src: BoundExpression,
    pub dst: BoundExpression,
    pub edge_type: Option<String>,
    pub rank: Option<BoundExpression>,
}

#[derive(Debug, Clone)]
pub struct BoundAssignment {
    pub property: String,
    pub value: BoundExpression,
    pub target: Option<BoundExpression>,
    pub object: Option<BoundExpression>,
    pub is_map_overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct BoundDelete {
    pub target: BoundDeleteTarget,
    pub with_edge: bool,
    pub detach: bool,
}

#[derive(Debug, Clone)]
pub enum BoundDeleteTarget {
    Vertices {
        tag: String,
        ids: Vec<BoundExpression>,
    },
    Edges {
        edge_type: Option<String>,
        edges: Vec<(BoundExpression, BoundExpression, Option<BoundExpression>)>,
    },
    Index(String),
}

// ── Bound pattern types (replace raw AST patterns) ───────────────────────────

#[derive(Debug, Clone)]
pub struct BoundPatternVertex {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: Option<Vec<(String, BoundExpression)>>,
}

#[derive(Debug, Clone)]
pub struct BoundPatternEdge {
    pub variable: Option<String>,
    pub edge_types: Vec<String>,
    pub properties: Option<Vec<(String, BoundExpression)>>,
    pub direction: EdgeDirection,
}

#[derive(Debug, Clone)]
pub enum BoundMergePattern {
    Node(BoundPatternVertex),
    Edge {
        src: BoundPatternVertex,
        edge: BoundPatternEdge,
        dst: BoundPatternVertex,
    },
}

#[derive(Debug, Clone)]
pub struct BoundMerge {
    pub pattern: BoundMergePattern,
    pub on_create: Vec<BoundAssignment>,
    pub on_match: Vec<BoundAssignment>,
}

#[derive(Debug, Clone)]
pub struct BoundSet {
    pub assignments: Vec<BoundAssignment>,
}

#[derive(Debug, Clone)]
pub struct BoundRemove {
    pub items: Vec<BoundExpression>,
}

#[derive(Debug, Clone)]
pub struct BoundCopy {
    pub target: CopyTarget,
    pub direction: CopyDirection,
    pub file_paths: Vec<String>,
    pub by_column: bool,
    pub header: bool,
    pub delimiter: char,
    pub batch_size: Option<usize>,
}

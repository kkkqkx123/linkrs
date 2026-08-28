//! Bound IR types: fully resolved intermediate representation.
//!
//! These types are produced by the [`Binder`](super::binder::Binder) and
//! consumed by the [`Planner`](crate::planning::planner::Planner).
//! Every name reference has been resolved against the catalog so that
//! downstream phases never need to re-parse or re-resolve the AST.

use crate::parser::ast::{LimitClause, SampleClause, SkipClause, Steps};
use graphdb_core::types::operators::{BinaryOperator, UnaryOperator};
use graphdb_core::types::semantic::{ColumnDef, ValueType};
use graphdb_core::types::{EdgeDirection, OrderDirection, Span};
use graphdb_core::DataType;
use graphdb_core::Value;

use super::query_graph::QueryGraph;

// ── BoundExpression ──────────────────────────────────────────────────────────

/// A fully resolved expression with known types and catalog references.
///
/// Unlike the AST [`Expression`] which carries unresolved names,
/// every variant here has been type-checked and catalog-resolved.
#[derive(Debug, Clone)]
pub enum BoundExpression {
    /// Literal value with resolved type
    Literal(Value, DataType),

    /// Column reference resolved to a variable + property
    ColumnRef(BoundColumnRef),

    /// Variable reference (no property access)
    Variable(String, DataType),

    /// Property access on an expression (e.g. `n.name`)
    Property {
        object: Box<BoundExpression>,
        property: String,
        value_type: DataType,
    },

    /// STRUCT field access (e.g. `addr.city`)
    StructField {
        base: Box<BoundExpression>,
        field: String,
        return_type: DataType,
    },

    /// Binary operation with resolved result type
    BinaryOp {
        left: Box<BoundExpression>,
        op: BinaryOperator,
        right: Box<BoundExpression>,
        return_type: DataType,
    },

    /// Unary operation
    UnaryOp {
        op: UnaryOperator,
        operand: Box<BoundExpression>,
        return_type: DataType,
    },

    /// Function call with resolved return type
    Function(BoundFunctionCall),

    /// Aggregate function call
    Aggregate(BoundAggregateCall),

    /// Parameter reference (`@param`)
    ParameterRef(String, DataType),

    /// Session variable reference (`$name`), distinct from query parameters.
    /// Type is dynamic (resolved at execution time from session state).
    SessionVariable(String, DataType),

    /// Subquery expression
    Subquery(Box<BoundStatement>),

    /// Cast expression
    Cast {
        expr: Box<BoundExpression>,
        target_type: DataType,
    },

    /// List literal
    List(Vec<BoundExpression>, DataType),

    /// Map literal
    Map(Vec<(String, BoundExpression)>, DataType),

    /// Case/when expression
    Case {
        expr: Option<Box<BoundExpression>>,
        when_then: Vec<(BoundExpression, BoundExpression)>,
        else_expr: Option<Box<BoundExpression>>,
        return_type: DataType,
    },

    /// Exists predicate
    Exists { query: Box<BoundStatement> },

    /// Pattern expression (path pattern in expression context)
    Pattern(QueryGraph),

    /// Label expression
    Label(String),

    /// Tag property access
    TagProperty {
        tag_name: String,
        property: String,
        value_type: DataType,
    },

    /// Edge property access
    EdgeProperty {
        edge_name: String,
        property: String,
        value_type: DataType,
    },

    /// Predicate expression (FILTER, ALL, ANY, etc.)
    Predicate {
        func: String,
        args: Vec<BoundExpression>,
        return_type: DataType,
    },

    /// Subscript access (collection[index])
    Subscript {
        collection: Box<BoundExpression>,
        index: Box<BoundExpression>,
        return_type: DataType,
    },

    /// Window function
    WindowFunction {
        name: String,
        args: Vec<BoundExpression>,
        over_partition_by: Vec<BoundExpression>,
        over_order_by: Vec<BoundExpression>,
        over_order_desc: Vec<bool>,
        return_type: DataType,
    },

    /// IN subquery
    In {
        expr: Box<BoundExpression>,
        subquery: Box<BoundStatement>,
        negated: bool,
    },

    /// Path expression
    Path(Vec<BoundExpression>, DataType),

    /// List comprehension
    ListComprehension {
        variable: String,
        source: Box<BoundExpression>,
        filter: Option<Box<BoundExpression>>,
        map: Option<Box<BoundExpression>>,
        return_type: DataType,
    },

    /// Reduce expression
    Reduce {
        accumulator: String,
        initial: Box<BoundExpression>,
        variable: String,
        source: Box<BoundExpression>,
        mapping: Box<BoundExpression>,
        return_type: DataType,
    },

    /// Path build
    PathBuild(Vec<BoundExpression>, DataType),

    /// Vector literal
    Vector(Vec<f32>),
}

/// A reference to a column, resolved to the defining variable and property.
#[derive(Debug, Clone)]
pub struct BoundColumnRef {
    /// Variable name (e.g., "n", "e")
    pub variable: String,
    /// Property name (e.g., "name", "age")
    pub property: String,
    /// Resolved tag name from catalog (if applicable)
    pub resolved_tag: Option<String>,
    /// Resolved type
    pub value_type: ValueType,
}

/// A function call with resolved arguments.
#[derive(Debug, Clone)]
pub struct BoundFunctionCall {
    pub name: String,
    pub args: Vec<BoundExpression>,
    pub return_type: ValueType,
}

/// Aggregate function call.
#[derive(Debug, Clone)]
pub struct BoundAggregateCall {
    pub function_name: String,
    pub arguments: Vec<BoundExpression>,
    pub distinct: bool,
    pub alias: Option<String>,
    pub return_type: ValueType,
}

impl BoundExpression {
    pub fn value_type(&self) -> ValueType {
        match self {
            Self::Literal(_, dt) => ValueType::from_data_type(dt),
            Self::ColumnRef(r) => r.value_type.clone(),
            Self::Variable(_, _) => ValueType::String,
            Self::Property { value_type, .. } => ValueType::from_data_type(value_type),
            Self::StructField { return_type, .. } => ValueType::from_data_type(return_type),
            Self::BinaryOp { return_type, .. } => ValueType::from_data_type(return_type),
            Self::UnaryOp { return_type, .. } => ValueType::from_data_type(return_type),
            Self::Function(f) => f.return_type.clone(),
            Self::Aggregate(a) => a.return_type.clone(),
            Self::ParameterRef(_, dt) => ValueType::from_data_type(dt),
            Self::SessionVariable(_, _) => ValueType::Unknown,
            Self::Subquery(_) => ValueType::Unknown,
            Self::Cast { target_type, .. } => ValueType::from_data_type(target_type),
            Self::List(_, dt) => ValueType::from_data_type(dt),
            Self::Map(_, dt) => ValueType::from_data_type(dt),
            Self::Case { return_type, .. } => ValueType::from_data_type(return_type),
            Self::Exists { .. } => ValueType::Bool,
            Self::Pattern(_) => ValueType::Path,
            Self::Label(_) => ValueType::String,
            Self::TagProperty { value_type, .. } => ValueType::from_data_type(value_type),
            Self::EdgeProperty { value_type, .. } => ValueType::from_data_type(value_type),
            Self::Predicate { return_type, .. } => ValueType::from_data_type(return_type),
            Self::Subscript { return_type, .. } => ValueType::from_data_type(return_type),
            Self::WindowFunction { return_type, .. } => ValueType::from_data_type(return_type),
            Self::In { .. } => ValueType::Bool,
            Self::Path(_, dt) => ValueType::from_data_type(dt),
            Self::ListComprehension { return_type, .. } => ValueType::from_data_type(return_type),
            Self::Reduce { return_type, .. } => ValueType::from_data_type(return_type),
            Self::PathBuild(_, dt) => ValueType::from_data_type(dt),
            Self::Vector(_) => ValueType::List,
        }
    }

    /// Return the concrete DataType of this expression.
    pub fn return_type(&self) -> DataType {
        match self {
            Self::Literal(_, dt) => dt.clone(),
            Self::ColumnRef(r) => r.value_type.to_data_type(),
            Self::Variable(_, dt) => dt.clone(),
            Self::Property { value_type, .. } => value_type.clone(),
            Self::StructField { return_type, .. } => return_type.clone(),
            Self::BinaryOp { return_type, .. } => return_type.clone(),
            Self::UnaryOp { return_type, .. } => return_type.clone(),
            Self::Function(f) => f.return_type.to_data_type(),
            Self::Aggregate(a) => a.return_type.to_data_type(),
            Self::ParameterRef(_, dt) => dt.clone(),
            Self::SessionVariable(_, _) => DataType::Unknown,
            Self::Subquery(_) => DataType::Unknown,
            Self::Cast { target_type, .. } => target_type.clone(),
            Self::List(_, dt) => dt.clone(),
            Self::Map(_, dt) => dt.clone(),
            Self::Case { return_type, .. } => return_type.clone(),
            Self::Exists { .. } => DataType::Bool,
            Self::Pattern(_) => DataType::Path,
            Self::Label(_) => DataType::String,
            Self::TagProperty { value_type, .. } => value_type.clone(),
            Self::EdgeProperty { value_type, .. } => value_type.clone(),
            Self::Predicate { return_type, .. } => return_type.clone(),
            Self::Subscript { return_type, .. } => return_type.clone(),
            Self::WindowFunction { return_type, .. } => return_type.clone(),
            Self::In { .. } => DataType::Bool,
            Self::Path(_, dt) => dt.clone(),
            Self::ListComprehension { return_type, .. } => return_type.clone(),
            Self::Reduce { return_type, .. } => return_type.clone(),
            Self::PathBuild(_, dt) => dt.clone(),
            Self::Vector(v) => DataType::VectorDense(v.len()),
        }
    }
}

// ── Clause-level bound types ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BoundReturnItem {
    pub expression: BoundExpression,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BoundOrderByItem {
    pub expression: BoundExpression,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone)]
pub struct BoundReturnClause {
    pub items: Vec<BoundReturnItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub sample: Option<SampleClause>,
}

#[derive(Debug, Clone)]
pub struct BoundYieldItem {
    pub expression: BoundExpression,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BoundYieldClause {
    pub items: Vec<BoundYieldItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
}

#[derive(Debug, Clone)]
pub struct BoundWhereClause {
    pub condition: BoundExpression,
}

#[derive(Debug, Clone)]
pub struct BoundWithClause {
    pub items: Vec<BoundReturnItem>,
    pub condition: Option<BoundExpression>,
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

// ── Statement-level bound types ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BoundMatchStatement {
    pub span: Span,
    pub query_graph: QueryGraph,
    pub where_clause: Option<BoundWhereClause>,
    pub return_clause: Option<BoundReturnClause>,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub optional: bool,
    pub delete_clause: Option<BoundMatchDeleteClause>,
}

#[derive(Debug, Clone)]
pub struct BoundGoStatement {
    pub span: Span,
    pub steps: Steps,
    pub from: Vec<BoundExpression>,
    pub over: Option<Vec<String>>,
    pub direction: EdgeDirection,
    pub where_clause: Option<BoundWhereClause>,
    pub yield_clause: Option<BoundYieldClause>,
}

#[derive(Debug, Clone)]
pub struct BoundLookupStatement {
    pub span: Span,
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
    pub span: Span,
    pub tag_name: Option<String>,
    pub ids: Vec<BoundExpression>,
    pub properties: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct BoundFetchEdgesStatement {
    pub span: Span,
    pub src: BoundExpression,
    pub dst: BoundExpression,
    pub edge_type: String,
    pub rank: Option<BoundExpression>,
    pub properties: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct BoundFindPathStatement {
    pub span: Span,
    pub from: Vec<BoundExpression>,
    pub to: BoundExpression,
    pub over: Option<(Vec<String>, EdgeDirection)>,
    pub where_clause: Option<BoundWhereClause>,
    pub shortest: bool,
    pub max_steps: Option<usize>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub yield_clause: Option<BoundYieldClause>,
}

#[derive(Debug, Clone)]
pub struct BoundSubgraphStatement {
    pub span: Span,
    pub steps: Steps,
    pub from: Vec<BoundExpression>,
    pub over: Option<(Vec<String>, EdgeDirection)>,
    pub where_clause: Option<BoundWhereClause>,
    pub yield_clause: Option<BoundYieldClause>,
}

#[derive(Debug, Clone)]
pub struct BoundReturnStatement {
    pub span: Span,
    pub items: Vec<BoundReturnItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub skip: Option<SkipClause>,
    pub limit: Option<LimitClause>,
}

#[derive(Debug, Clone)]
pub struct BoundWithStatement {
    pub span: Span,
    pub items: Vec<BoundReturnItem>,
    pub condition: Option<BoundExpression>,
}

#[derive(Debug, Clone)]
pub struct BoundUnwindStatement {
    pub span: Span,
    pub expression: BoundExpression,
    pub alias: String,
}

#[derive(Debug, Clone)]
pub struct BoundPipeStatement {
    pub span: Span,
    pub statements: Vec<BoundStatement>,
}

#[derive(Debug, Clone)]
pub struct BoundGroupByStatement {
    pub span: Span,
    pub keys: Vec<BoundExpression>,
    pub aggregates: Vec<BoundAggregateCall>,
}

#[derive(Debug, Clone)]
pub struct BoundSetOperationStatement {
    pub span: Span,
    pub left: Box<BoundStatement>,
    pub right: Box<BoundStatement>,
    pub operation: SetOperationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperationKind {
    Union,
    Intersect,
    Minus,
}

// ── BoundStatement ───────────────────────────────────────────────────────────

/// Fully bound query statement with all names resolved.
#[derive(Debug, Clone)]
pub enum BoundStatement {
    // ── DQL ─────────────────────────────────────────────────────────────────
    Match(BoundMatchStatement),
    Go(BoundGoStatement),
    Lookup(BoundLookupStatement),
    FetchVertices(BoundFetchVerticesStatement),
    FetchEdges(BoundFetchEdgesStatement),
    FindPath(BoundFindPathStatement),
    Subgraph(BoundSubgraphStatement),

    // ── Clause operators ────────────────────────────────────────────────────
    Pipe(BoundPipeStatement),
    Return(BoundReturnStatement),
    With(BoundWithStatement),
    Unwind(BoundUnwindStatement),
    GroupBy(BoundGroupByStatement),
    SetOperation(BoundSetOperationStatement),

    // ── Placeholder for management/DDL/DML ─────────────────────────────────
    Other(Box<crate::parser::ast::Stmt>),
}

impl BoundStatement {
    pub fn kind(&self) -> &str {
        match self {
            Self::Match(_) => "Match",
            Self::Go(_) => "Go",
            Self::Lookup(_) => "Lookup",
            Self::FetchVertices(_) => "FetchVertices",
            Self::FetchEdges(_) => "FetchEdges",
            Self::FindPath(_) => "FindPath",
            Self::Subgraph(_) => "Subgraph",
            Self::Pipe(_) => "Pipe",
            Self::Return(_) => "Return",
            Self::With(_) => "With",
            Self::Unwind(_) => "Unwind",
            Self::GroupBy(_) => "GroupBy",
            Self::SetOperation(_) => "SetOperation",
            Self::Other(stmt) => stmt.kind(),
        }
    }

    pub fn as_match(&self) -> Option<&BoundMatchStatement> {
        if let Self::Match(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn as_go(&self) -> Option<&BoundGoStatement> {
        if let Self::Go(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

// ── ColumnDef helper ─────────────────────────────────────────────────────────

/// Column definition with name and type.
#[derive(Debug, Clone)]
pub struct BoundColumnDef {
    pub name: String,
    pub type_: ValueType,
}

impl From<ColumnDef> for BoundColumnDef {
    fn from(c: ColumnDef) -> Self {
        Self {
            name: c.name,
            type_: c.type_,
        }
    }
}

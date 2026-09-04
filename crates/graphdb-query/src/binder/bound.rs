//! Bound IR types: fully resolved intermediate representation.
//!
//! These types are produced by the [`Binder`](super::binder::Binder) and
//! consumed by the [`Planner`](crate::planning::planner::Planner).
//! Every name reference has been resolved against the catalog so that
//! downstream phases never need to re-parse or re-resolve the AST.

use crate::parser::ast::{LimitClause, SampleClause, SkipClause, Steps};
use graphdb_core::types::operators::{BinaryOperator, UnaryOperator};
use graphdb_core::types::semantic::{ColumnDef, ValueType};
use graphdb_core::types::{EdgeDirection, OrderDirection};
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

    /// Get the ValueType of this expression for downstream consumers.
    pub fn value_type(&self) -> ValueType {
        ValueType::from_data_type(&self.return_type())
    }
}

// ── Clause-level bound types ─────────────────────────────────────────────────

/// A unified projection item used by RETURN, YIELD, WITH, and COLLECT clauses.
///
/// Previously `BoundReturnItem` and `BoundYieldItem` existed as identical
/// separate structs; they have been merged into this single type.
#[derive(Debug, Clone)]
pub struct BoundProjectionItem {
    pub expression: BoundExpression,
    pub alias: Option<String>,
}

/// Backward-compatible alias for [`BoundProjectionItem`].
pub type BoundReturnItem = BoundProjectionItem;
/// Backward-compatible alias for [`BoundProjectionItem`].
pub type BoundYieldItem = BoundProjectionItem;

#[derive(Debug, Clone)]
pub struct BoundOrderByItem {
    pub expression: BoundExpression,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone)]
pub struct BoundReturnClause {
    pub items: Vec<BoundProjectionItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub sample: Option<SampleClause>,
}

#[derive(Debug, Clone)]
pub struct BoundYieldClause {
    pub items: Vec<BoundProjectionItem>,
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
    pub items: Vec<BoundProjectionItem>,
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
    Intersect,
    Minus,
}

// ── DML bound types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BoundInsert {
    pub target: BoundInsertTarget,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub enum BoundInsertTarget {
    Vertices {
        tags: Vec<crate::parser::ast::TagInsertSpec>,
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
    pub tag_values: Vec<Vec<BoundExpression>>,
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
}

#[derive(Debug, Clone)]
pub struct BoundDelete {
    pub target: BoundDeleteTarget,
    pub where_clause: Option<BoundExpression>,
    pub with_edge: bool,
}

#[derive(Debug, Clone)]
pub enum BoundDeleteTarget {
    Vertices(Vec<BoundExpression>),
    Edges {
        edge_type: Option<String>,
        edges: Vec<(BoundExpression, BoundExpression, Option<BoundExpression>)>,
    },
    Tags {
        tag_names: Vec<String>,
        vertex_ids: Vec<BoundExpression>,
        is_all_tags: bool,
    },
    Index(String),
}

// ── Bound pattern types (replace raw AST patterns) ───────────────────────────

#[derive(Debug, Clone)]
pub enum BoundCreateTarget {
    Node {
        variable: Option<String>,
        labels: Vec<String>,
        properties: Option<Vec<(String, BoundExpression)>>,
    },
    Edge(Box<BoundEdgeCreateTarget>),
    Path {
        patterns: Vec<BoundPatternElement>,
    },
}

/// Edge create target payload. Boxed inside [`BoundCreateTarget::Edge`] to keep
/// the enum's inline size bounded by its smaller variants.
#[derive(Debug, Clone)]
pub struct BoundEdgeCreateTarget {
    pub variable: Option<String>,
    pub edge_type: String,
    pub src: BoundExpression,
    pub dst: BoundExpression,
    pub properties: Option<Vec<(String, BoundExpression)>>,
    pub direction: EdgeDirection,
}

#[derive(Debug, Clone)]
pub enum BoundPatternElement {
    Node(BoundPatternVertex),
    Edge(BoundPatternEdge),
}

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
pub struct BoundCreate {
    pub target: BoundCreateTarget,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundDrop {
    pub target: crate::parser::ast::DropTarget,
    pub if_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundAlter {
    pub target: crate::parser::ast::AlterTarget,
}

#[derive(Debug, Clone)]
pub struct BoundUse {
    pub space: String,
}

#[derive(Debug, Clone)]
pub struct BoundShow {
    pub target: crate::parser::ast::ShowTarget,
}

#[derive(Debug, Clone)]
pub struct BoundShowCreate {
    pub target: crate::parser::ast::ShowCreateTarget,
}

#[derive(Debug, Clone)]
pub struct BoundDesc {
    pub target: crate::parser::ast::DescTarget,
}

#[derive(Debug, Clone)]
pub struct BoundClearSpace {
    pub space_name: String,
}

#[derive(Debug, Clone)]
pub struct BoundCreateUser {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundDropUser {
    pub username: String,
    pub if_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundAlterUser {
    pub username: String,
    pub password: Option<String>,
    pub new_role: Option<String>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct BoundCreateFulltextIndex {
    pub index_name: String,
    pub schema_name: String,
    pub fields: Vec<crate::parser::ast::fulltext::IndexFieldDef>,
    pub engine_type: graphdb_core::types::FulltextEngineType,
    pub options: crate::parser::ast::fulltext::IndexOptions,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundCreateVectorIndex {
    pub index_name: String,
    pub schema_name: String,
    pub field_name: String,
    pub config: crate::parser::ast::vector::VectorIndexConfig,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundExplain {
    pub statement: Box<BoundStatement>,
    pub format: crate::parser::ast::ExplainFormat,
    pub analyze: bool,
}

#[derive(Debug, Clone)]
pub struct BoundProfile {
    pub statement: Box<BoundStatement>,
    pub format: crate::parser::ast::ExplainFormat,
}

#[derive(Debug, Clone)]
pub struct BoundBeginTransaction {
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct BoundCommit;

#[derive(Debug, Clone)]
pub struct BoundRollback {
    pub savepoint_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BoundCopy {
    pub target: crate::parser::ast::CopyTarget,
    pub direction: crate::parser::ast::CopyDirection,
    pub file_path: String,
    pub header: bool,
    pub delimiter: char,
    pub batch_size: Option<usize>,
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
    Filter(BoundFilter),
    Yield(BoundYield),
    Collect(BoundCollect),
    AssignVariable(BoundAssignVariable),

    // ── DML ─────────────────────────────────────────────────────────────────
    Insert(BoundInsert),
    Update(BoundUpdate),
    Delete(BoundDelete),
    Merge(BoundMerge),
    Set(BoundSet),
    Remove(BoundRemove),
    Copy(BoundCopy),

    // ── DDL ─────────────────────────────────────────────────────────────────
    Create(BoundCreate),
    Drop(BoundDrop),
    Alter(BoundAlter),
    Show(BoundShow),
    ShowCreate(BoundShowCreate),
    Desc(BoundDesc),
    ClearSpace(BoundClearSpace),
    CreateUser(BoundCreateUser),
    DropUser(BoundDropUser),
    AlterUser(BoundAlterUser),
    CreateFulltextIndex(BoundCreateFulltextIndex),
    CreateVectorIndex(BoundCreateVectorIndex),

    // ── EXPLAIN / PROFILE ──────────────────────────────────────────────────
    Explain(BoundExplain),
    Profile(BoundProfile),

    // ── USE ────────────────────────────────────────────────────────────────
    Use(BoundUse),

    // ── Transaction ─────────────────────────────────────────────────────────
    BeginTransaction(BoundBeginTransaction),
    Commit(BoundCommit),
    Rollback(BoundRollback),

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
            Self::Filter(_) => "Filter",
            Self::Yield(_) => "Yield",
            Self::Collect(_) => "Collect",
            Self::AssignVariable(_) => "AssignVariable",
            Self::Insert(_) => "Insert",
            Self::Update(_) => "Update",
            Self::Delete(_) => "Delete",
            Self::Merge(_) => "Merge",
            Self::Set(_) => "Set",
            Self::Remove(_) => "Remove",
            Self::Copy(_) => "Copy",
            Self::Create(_) => "Create",
            Self::Drop(_) => "Drop",
            Self::Alter(_) => "Alter",
            Self::Show(_) => "Show",
            Self::ShowCreate(_) => "ShowCreate",
            Self::Desc(_) => "Desc",
            Self::ClearSpace(_) => "ClearSpace",
            Self::CreateUser(_) => "CreateUser",
            Self::DropUser(_) => "DropUser",
            Self::AlterUser(_) => "AlterUser",
            Self::CreateFulltextIndex(_) => "CreateFulltextIndex",
            Self::CreateVectorIndex(_) => "CreateVectorIndex",
            Self::Explain(_) => "Explain",
            Self::Profile(_) => "Profile",
            Self::Use(_) => "Use",
            Self::BeginTransaction(s) => match s.read_only {
                Some(true) => "BeginTransactionReadOnly",
                Some(false) => "BeginTransactionReadWrite",
                None => "BeginTransaction",
            },
            Self::Commit(_) => "Commit",
            Self::Rollback(s) => {
                if s.savepoint_name.is_some() {
                    "RollbackToSavepoint"
                } else {
                    "Rollback"
                }
            }
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

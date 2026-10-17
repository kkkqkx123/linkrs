use linkrs_core::types::expr::SubqueryBody;
use linkrs_core::types::operators::{BinaryOperator, UnaryOperator};
use linkrs_core::types::semantic::ValueType;
use linkrs_core::DataType;
use linkrs_core::Value;

use super::stmt::BoundStatement;
use crate::binder::query_graph::QueryGraph;

/// A fully resolved expression with known types and catalog references.
///
/// Unlike the AST [`Expression`] which carries unresolved names,
/// every variant here has been type-checked and catalog-resolved.
#[derive(Debug, Clone)]
pub enum BoundExpression {
    /// Literal value with resolved type
    Literal(Value, DataType),

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
    Subquery {
        query: Box<BoundStatement>,
        original_body: Option<Box<SubqueryBody>>,
    },

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
    Exists {
        query: Box<BoundStatement>,
        original_body: Option<Box<SubqueryBody>>,
    },

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
        original_body: Option<Box<SubqueryBody>>,
    },

    /// COUNT subquery: evaluates to the number of rows returned by the subquery
    CountSubquery {
        query: Box<BoundStatement>,
        original_body: Option<Box<SubqueryBody>>,
    },

    /// Lambda expression: anonymous function used as callback for higher-order list functions
    Lambda {
        params: Vec<String>,
        body: Box<BoundExpression>,
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
            Self::Variable(_, dt) => dt.clone(),
            Self::Property { value_type, .. } => value_type.clone(),
            Self::StructField { return_type, .. } => return_type.clone(),
            Self::BinaryOp { return_type, .. } => return_type.clone(),
            Self::UnaryOp { return_type, .. } => return_type.clone(),
            Self::Function(f) => f.return_type.to_data_type(),
            Self::Aggregate(a) => a.return_type.to_data_type(),
            Self::ParameterRef(_, dt) => dt.clone(),
            Self::SessionVariable(_, _) => DataType::Unknown,
            Self::Subquery { .. } => DataType::Unknown,
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
            Self::CountSubquery { .. } => DataType::Int,
            Self::Lambda { .. } => DataType::Unknown,
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

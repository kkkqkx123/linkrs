//! Expression Type Definition
//!
//! This module defines the `Expression` enumeration of uniform expression types used in the query engine.

pub use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
pub use crate::core::types::DataType;
use crate::core::Value;
use serde::{Deserialize, Serialize};

/// Unified Expression Type
///
/// An enumeration of expressions containing location information (`span` fields) for:
/// - Parser layer: error localization and reporting
/// - Core Layer: Type Checking and Enforcement
/// - Serialization: storage and transmission
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expression {
    /// literal value
    Literal(Value),

    /// variable reference
    Variable(String),

    /// Property Access
    Property {
        object: Box<Expression>,
        property: String,
    },

    /// binary operation
    Binary {
        left: Box<Expression>,
        op: BinaryOperator,
        right: Box<Expression>,
    },

    /// one-dimensional operation
    Unary {
        op: UnaryOperator,
        operand: Box<Expression>,
    },

    /// function call
    Function { name: String, args: Vec<Expression> },

    /// aggregate function (math.)
    Aggregate {
        func: AggregateFunction,
        args: Vec<Expression>,
        distinct: bool,
        filter: Option<Box<Expression>>,
    },

    /// list literal
    List(Vec<Expression>),

    /// mapping literal
    Map(Vec<(String, Expression)>),

    /// conditional expression
    Case {
        test_expr: Option<Box<Expression>>,
        conditions: Vec<(Expression, Expression)>,
        default: Option<Box<Expression>>,
    },

    /// type conversion
    TypeCast {
        expression: Box<Expression>,
        target_type: DataType,
    },

    /// subscript access
    Subscript {
        collection: Box<Expression>,
        index: Box<Expression>,
    },

    /// range expression
    Range {
        collection: Box<Expression>,
        start: Option<Box<Expression>>,
        end: Option<Box<Expression>>,
    },

    /// path expression
    Path(Vec<Expression>),

    /// tag expression
    Label(String),

    /// List Derivative Expressions
    ListComprehension {
        variable: String,
        source: Box<Expression>,
        filter: Option<Box<Expression>>,
        map: Option<Box<Expression>>,
    },

    /// Dynamic access to tag attributes
    ///
    /// Used to access tag properties dynamically, e.g. `tagName.propertyName`
    /// where tagName is a variable or tag expression
    LabelTagProperty {
        tag: Box<Expression>,
        property: String,
    },

    /// Tag Attribute Access
    ///
    /// Used to access properties on the vertex tag, e.g. `tagName.propertyName`
    TagProperty { tag_name: String, property: String },

    /// Edge Attribute Access
    ///
    /// Used to access attributes on an edge type
    EdgeProperty { edge_name: String, property: String },

    /// predicate expression (math.)
    ///
    /// Used to implement predicate functions such as FILTER, ALL, ANY, EXISTS, etc.
    Predicate { func: String, args: Vec<Expression> },

    /// Reduce expression
    ///
    /// Used to implement the REDUCE function
    Reduce {
        accumulator: String,
        initial: Box<Expression>,
        variable: String,
        source: Box<Expression>,
        mapping: Box<Expression>,
    },

    /// path construction expression
    ///
    /// Used for building paths, such as `path(v1, e1, v2)`
    PathBuild(Vec<Expression>),

    /// Query parameter expression
    ///
    /// Used to represent query parameters, e.g. `$param`.
    Parameter(String),

/// Vector literal expression
///
/// Represents vector literals like VECTOR[0.1, 0.2, 0.3] or [0.1, 0.2]::VECTOR
Vector(Vec<f32>),

    /// Window function expression with OVER clause
    WindowFunction {
        /// Window function name (e.g. "row_number", "rank", "lead")
        name: String,
        /// Arguments to the window function
        args: Vec<Expression>,
        /// PARTITION BY expressions
        over_partition_by: Vec<Expression>,
        /// ORDER BY expressions
        over_order_by: Vec<Expression>,
        /// Whether each ORDER BY expression is descending
        over_order_desc: Vec<bool>,
    },

    /// EXISTS subquery expression
    ///
    /// Evaluates to true if the subquery pattern has at least one match.
    /// Syntax: `EXISTS { pattern }` or `EXISTS(pattern)`
    Exists {
        /// The subquery body (WHERE clause condition or pattern reference)
        body: Box<SubqueryBody>,
    },

    /// IN subquery expression
    ///
    /// Evaluates to true if `expr` is in the result set of the subquery.
    /// Syntax: `expr IN { MATCH ... RETURN ... }`
    In {
        /// The expression to test
        expr: Box<Expression>,
        /// The subquery body
        subquery: Box<SubqueryBody>,
        /// Whether this is NOT IN
        negated: bool,
    },
}

/// A subquery body used in EXISTS and IN expressions.
///
/// Represents a mini-query: MATCH pattern [WHERE condition] RETURN expression
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubqueryBody {
    /// Pattern elements (triples like `a:Person-[:KNOWS]->b:Person`)
    pub patterns: Vec<String>,
    /// WHERE clause expression
    pub where_clause: Option<Box<Expression>>,
    /// RETURN expression (single expression for scalar subquery)
    pub return_expr: Option<Box<Expression>>,
    /// Whether this is a correlated subquery (references outer variables)
    pub is_correlated: bool,
}

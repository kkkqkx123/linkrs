//! AST Basic Type Definitions
//!
//! This module defines types specific to the query AST (Abstract Syntax Tree), including tags, property references, clause structures, etc.
//! At the same time, the type of the core module is re-exported for easier use.

pub use graphdb_core::types::operators::AggregateFunction as CoreAggregateFunction;
pub use graphdb_core::types::{EdgeDirection, OrderDirection};

pub use graphdb_core::types::Span;

pub type BinaryOp = graphdb_core::types::operators::BinaryOperator;
pub type UnaryOp = graphdb_core::types::operators::UnaryOperator;
pub type DataType = graphdb_core::types::DataType;
pub type AggregateFunction = CoreAggregateFunction;

#[derive(Debug, Clone, PartialEq)]
pub struct LimitClause {
    pub span: Span,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkipClause {
    pub span: Span,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SampleClause {
    pub span: Span,
    pub count: usize,
    pub percentage: Option<f64>,
}

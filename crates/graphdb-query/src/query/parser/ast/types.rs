//! AST Basic Type Definitions
//!
//! This module defines types specific to the query AST (Abstract Syntax Tree), including tags, property references, clause structures, etc.
//! At the same time, the type of the core module is re-exported for easier use.

pub use crate::core::types::operators::AggregateFunction as CoreAggregateFunction;
pub use crate::core::types::{EdgeDirection, OrderDirection};

pub use crate::core::types::Span;

pub type BinaryOp = crate::core::types::operators::BinaryOperator;
pub type UnaryOp = crate::core::types::operators::UnaryOperator;
pub type DataType = crate::core::types::DataType;
pub type AggregateFunction = CoreAggregateFunction;

#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyRef {
    pub name: String,
}

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

//! Unified expression type definitions
//!
//! This module defines the unified expression type `Expression` that is used in the query engine.
//!
//! ## Design Specifications
//!
//! "Expression" is a unified type of expression that combines the characteristics of the following sources:
//! - **AST at the Parser Layer**: Provides `Span` information for error localization.
//! - **Core layer expressions**: Provide serialization support and aggregate functions.
//!
//! ## Type Characteristics
//!
//! – **Location information**: The optional `Span` field is used for error reporting.
//! - **Aggregate functions**: The `Aggregate` variant is supported for aggregate queries.
//! - **Serialization support**: Serialization/deserialization is supported via `serde`.
//!
//! ## Explanation of the Variants
//!
//! | Variant | Purpose |
//! |------|------|
//! | `Literal` | Literal value |
//! | `Variable` | Variable reference |
//! `Property` | Access to properties
//! `Binary` | Binary operations
//! `Unary` | Unary operation
//! | `Function` | Function Call |
//! `Aggregate` | Aggregate function
//! `List` | List literal
//! `Map` | Literal map expression
//! | `Case` | Conditional Expression |
//! `TypeCast` | Type conversion
//! **Subscript** | Access using subscripts
//! | `Range` | Range expression |
//! | `Path` | Path expression |
//! | `Label` | Label Expression |
//!
//! ## Usage Examples
//!
//! ```rust
//! use crate::core::types::expr::Expression;
//! use crate::core::types::operators::{BinaryOperator, AggregateFunction};
//! use crate::core::Value;
//!
// Simple literals
//! let expression = Expression::literal(Value::Int(42));
//!
// Binary operations
//! let sum = Expression::variable("a") + Expression::variable("b");
//!
// Aggregate functions
//! let count = Expression::aggregate(
//!     AggregateFunction::Count,
//!     Expression::variable("col"),
//!     false
//! );
//! ```
//!
//! ## Context Explanation
//!
//! This module defines pure data types, which do not contain any context.
//! The type definitions relevant to the context are defined in the `query` module.
//! - **`query::validator::context::ExpressionAnalysisContext`**: compile-time analysis context for validation, optimizer, type derivation, etc. phases
//! - **`query::executor::expression::evaluation_context::ExpressionContext`**: runtime evaluation context trait for expression evaluation
//!
//! Please select the appropriate context type based on the usage scenario.

// Submodule definition - organized by functionality

// Analysis utilities
pub mod analysis_utils;

// Construction and building
mod construction;

// Context-aware expressions
pub mod contextual;

// Core expression definitions
mod def;

// Display formatting
mod display;

// Expression metadata
pub mod expression;

// Expression utilities
pub mod expression_utils;

// Grouping utilities
pub mod group_utils;

// Inspection utilities
mod inspection;

// Memory estimation
pub mod memory_estimation;

// Serialization support
pub mod serializable;

// Expression traversal
mod traverse;

// Type deduction
mod type_deduce;

// Visitor pattern
pub mod expression_context;
pub mod visitor;
pub mod visitor_checkers;
pub mod visitor_collectors;

// Unified Export - Core types
pub use contextual::ContextualExpression;
pub use def::Expression;
pub use expression::{ExpressionId, ExpressionMeta};
pub use serializable::SerializableExpression;

// Unified Export - Visitor pattern
pub use expression_context::{ExpressionAnalysisContext, OptimizationFlags};
pub use visitor::ExpressionVisitor;
pub use visitor_checkers::{ConstantChecker, PropertyContainsChecker};
pub use visitor_collectors::{
    FunctionCollector, OrConditionCollector, PropertyCollector, PropertyPredicate,
    PropertyPredicateCollector, VariableCollector,
};

// Unified Export - Analysis utilities
pub use analysis_utils::{
    collect_variables, collect_variables_from_contextual, extract_aggregate_functions, find_all,
    has_aggregate_function, is_constant, is_constant_expression, is_evaluable,
};

// Unified Export - Expression utilities
pub use expression_utils::{
    extract_group_info, extract_property_refs, extract_string_from_expr,
    generate_default_alias_from_contextual,
};

// Unified Export - Grouping utilities
pub use group_utils::{extract_group_suite, GroupSuite};

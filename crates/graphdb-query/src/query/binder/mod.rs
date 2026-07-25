//! Binder module: AST → BoundStatement conversion with catalog resolution.
//!
//! The Binder is the replacement for the old Validator.  It produces a
//! fully-resolved [`BoundStatement`] IR that the planner consumes directly.

pub mod binder;
pub mod bound;
pub mod expr_binder;
pub mod expr_converter;
pub mod query_graph;
pub mod scope;
pub mod semantic_checker;
pub mod validation;

pub use binder::Binder;
pub use bound::{
    BoundColumnRef, BoundExpression, BoundMatchStatement, BoundStatement,
};
pub use expr_binder::ExpressionBinder;
pub use query_graph::{BoundEdgePattern, BoundEdgeTypeRef, BoundNodePattern, QueryGraph};
pub use scope::{BinderScope, BinderVariable};
pub use semantic_checker::validate_expression;
pub use validation::{
    AggregateCallInfo, ClauseKind, CypherClauseKind, HintSeverity, IndexHint,
    OptimizationHint, PathAnalysis, SemanticInfo, ValidatedStatement, ValidationInfo,
};

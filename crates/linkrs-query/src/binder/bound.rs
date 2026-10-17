//! Bound IR types: fully resolved intermediate representation.
//!
//! These types are produced by the [`Binder`](super::binder::Binder) and
//! consumed by the [`Planner`](crate::planning::planner::Planner).
//! Every name reference has been resolved against the catalog so that
//! downstream phases never need to re-parse or re-resolve the AST.
//!
//! The module was split from the original single `bound.rs` (70+ types) into
//! focused submodules by responsibility:
//! - `expr`: resolved expression types and their helpers
//! - `clause`: projection / order-by / return / yield / where / with clauses
//! - `stmt`: the [`BoundStatement`] sum type and its accessors
//! - `stmt_dql`: DQL statement structs (Match, Go, Lookup, ...)
//! - `stmt_dml`: DML statement structs (Insert, Update, Delete, Merge, ...)
//! - `stmt_ddl`: DDL / management / transaction statement structs

mod clause;
mod expr;
mod stmt;
mod stmt_ddl;
mod stmt_dml;
mod stmt_dql;

pub use clause::*;
pub use expr::*;
pub use stmt::*;
pub use stmt_ddl::*;
pub use stmt_dml::*;
pub use stmt_dql::*;
